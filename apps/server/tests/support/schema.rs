use std::path::Path;

use anyhow::{ensure, Context, Result};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};

use super::TestApplication;

const DATABASE_FILE: &str = "palmr.db";
const MIGRATIONS_TABLE: &str = "_sqlx_migrations";
const STORAGE_OBJECTS: &str = "storage_objects";
const STORAGE_OBJECT_COLUMN: &str = "storage_object_id";
const RESTRICT: &str = "RESTRICT";
const CASCADE: &str = "CASCADE";
const EXPLICIT_DELETE_ACTIONS: [&str; 3] = [RESTRICT, CASCADE, "SET NULL"];

pub const BOOLEAN_COLUMNS: [(&str, &str); 16] = [
    ("users", "must_change_password"),
    ("users", "is_active"),
    ("users", "totp_enabled"),
    ("identity_providers", "is_enabled"),
    ("identity_providers", "auto_provision"),
    ("identity_links", "email_verified_at_link"),
    ("shares", "is_active"),
    ("shares", "notify_recipients"),
    ("shares", "show_owner"),
    ("reverse_shares", "is_active"),
    ("reverse_shares", "notify_owner"),
    ("reverse_share_upload_sessions", "send_confirmation"),
    ("transfer_sessions", "cancel_requested"),
    ("tus_uploads", "upload_defer_length"),
    ("app_settings", "is_secret"),
    ("branding_assets", "is_current"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub declared_type: String,
    pub not_null: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub columns: Vec<String>,
    pub parent: String,
    pub on_delete: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexColumn {
    pub name: Option<String>,
    pub collation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    pub name: String,
    pub unique: bool,
    pub partial: bool,
    pub columns: Vec<IndexColumn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub sql: String,
    pub columns: Vec<Column>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<Index>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaObject {
    pub kind: String,
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub tables: Vec<Table>,
    pub objects: Vec<SchemaObject>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Violation {
    ByteOwnerCascade {
        table: String,
        columns: Vec<String>,
        parent: String,
    },
    StorageObjectNotRestrict {
        table: String,
        columns: Vec<String>,
        on_delete: String,
    },
    ImplicitDeleteAction {
        table: String,
        columns: Vec<String>,
        parent: String,
        on_delete: String,
    },
    NocaseCollation {
        object: String,
    },
    TimestampNotText {
        table: String,
        column: String,
        declared_type: String,
    },
    BooleanNotCanonical {
        table: String,
        column: String,
    },
    BooleanUnregistered {
        table: String,
        column: String,
    },
}

impl Table {
    pub fn column(&self, name: &str) -> Option<&Column> {
        self.columns.iter().find(|column| column.name == name)
    }

    pub fn owns_bytes(&self) -> bool {
        self.name.eq_ignore_ascii_case(STORAGE_OBJECTS)
            || self.columns.iter().any(|column| {
                let name = column.name.to_ascii_lowercase();
                name == STORAGE_OBJECT_COLUMN
                    || name.ends_with(&format!("_{STORAGE_OBJECT_COLUMN}"))
            })
            || self
                .foreign_keys
                .iter()
                .any(|foreign_key| foreign_key.parent.eq_ignore_ascii_case(STORAGE_OBJECTS))
    }
}

impl Schema {
    pub async fn introspect(connection: &mut SqliteConnection) -> Result<Self> {
        let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT type, name, tbl_name, sql FROM sqlite_schema ORDER BY type, name",
        )
        .fetch_all(&mut *connection)
        .await
        .context("read sqlite_schema")?;

        let mut tables = Vec::new();
        let mut objects = Vec::new();
        for (kind, name, owner, sql) in rows {
            if is_internal(&name) || is_internal(&owner) {
                continue;
            }
            if kind == "table" {
                tables.push(
                    introspect_table(connection, &name, sql.clone().unwrap_or_default()).await?,
                );
            }
            if let Some(sql) = sql {
                objects.push(SchemaObject { kind, name, sql });
            }
        }
        Ok(Self { tables, objects })
    }

    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|table| table.name == name)
    }

    pub fn table_names(&self) -> Vec<&str> {
        self.tables
            .iter()
            .map(|table| table.name.as_str())
            .collect()
    }
}

pub async fn migrated_schema(test_name: &str) -> Result<Schema> {
    let application = TestApplication::start(test_name).await?;
    let schema = read_migrated(&application.data_dir().join(DATABASE_FILE)).await;
    application.shutdown().await;
    schema
}

pub async fn scratch_schema(ddl: &str) -> Result<Schema> {
    let mut connection = SqliteConnection::connect("sqlite::memory:")
        .await
        .context("open scratch database")?;
    sqlx::raw_sql(ddl)
        .execute(&mut connection)
        .await
        .context("apply scratch DDL")?;
    let schema = Schema::introspect(&mut connection).await?;
    connection.close().await?;
    Ok(schema)
}

pub fn all_violations(schema: &Schema) -> Vec<Violation> {
    [
        byte_owner_cascades(schema),
        implicit_delete_actions(schema),
        nocase_collations(schema),
        non_text_timestamps(schema),
        non_canonical_booleans(schema),
    ]
    .concat()
}

pub fn byte_owner_cascades(schema: &Schema) -> Vec<Violation> {
    let mut violations = Vec::new();
    for table in schema.tables.iter().filter(|table| table.owns_bytes()) {
        for foreign_key in &table.foreign_keys {
            if foreign_key.on_delete == CASCADE {
                violations.push(Violation::ByteOwnerCascade {
                    table: table.name.clone(),
                    columns: foreign_key.columns.clone(),
                    parent: foreign_key.parent.clone(),
                });
            } else if foreign_key.parent.eq_ignore_ascii_case(STORAGE_OBJECTS)
                && foreign_key.on_delete != RESTRICT
            {
                violations.push(Violation::StorageObjectNotRestrict {
                    table: table.name.clone(),
                    columns: foreign_key.columns.clone(),
                    on_delete: foreign_key.on_delete.clone(),
                });
            }
        }
    }
    violations
}

pub fn implicit_delete_actions(schema: &Schema) -> Vec<Violation> {
    schema
        .tables
        .iter()
        .flat_map(|table| {
            table
                .foreign_keys
                .iter()
                .filter(|foreign_key| {
                    !EXPLICIT_DELETE_ACTIONS.contains(&foreign_key.on_delete.as_str())
                })
                .map(|foreign_key| Violation::ImplicitDeleteAction {
                    table: table.name.clone(),
                    columns: foreign_key.columns.clone(),
                    parent: foreign_key.parent.clone(),
                    on_delete: foreign_key.on_delete.clone(),
                })
        })
        .collect()
}

pub fn nocase_collations(schema: &Schema) -> Vec<Violation> {
    let mut objects: Vec<String> = schema
        .objects
        .iter()
        .filter(|object| {
            words(&object.sql)
                .windows(2)
                .any(|pair| pair[0] == "collate" && pair[1] == "nocase")
        })
        .map(|object| object.name.clone())
        .collect();
    for table in &schema.tables {
        for index in &table.indexes {
            let nocase = index
                .columns
                .iter()
                .any(|column| column.collation.eq_ignore_ascii_case("NOCASE"));
            if nocase && !objects.contains(&index.name) {
                objects.push(index.name.clone());
            }
        }
    }
    objects
        .into_iter()
        .map(|object| Violation::NocaseCollation { object })
        .collect()
}

pub fn non_text_timestamps(schema: &Schema) -> Vec<Violation> {
    schema
        .tables
        .iter()
        .flat_map(|table| {
            table
                .columns
                .iter()
                .filter(|column| {
                    column.name.to_ascii_lowercase().ends_with("_at")
                        && !column.declared_type.eq_ignore_ascii_case("TEXT")
                })
                .map(|column| Violation::TimestampNotText {
                    table: table.name.clone(),
                    column: column.name.clone(),
                    declared_type: column.declared_type.clone(),
                })
        })
        .collect()
}

pub fn non_canonical_booleans(schema: &Schema) -> Vec<Violation> {
    let mut violations = Vec::new();
    for table in &schema.tables {
        let registered: Vec<&str> = BOOLEAN_COLUMNS
            .iter()
            .filter(|(owner, _)| *owner == table.name)
            .map(|(_, column)| *column)
            .collect();
        let checked = boolean_checks(&table.sql);

        for name in &registered {
            let canonical = table.column(name).is_some_and(|column| {
                column.declared_type.eq_ignore_ascii_case("INTEGER")
                    && column.not_null
                    && checked.iter().any(|checked| checked == name)
            });
            if !canonical {
                violations.push(Violation::BooleanNotCanonical {
                    table: table.name.clone(),
                    column: (*name).to_owned(),
                });
            }
        }

        for column in &table.columns {
            if registered.contains(&column.name.as_str()) {
                continue;
            }
            if column.declared_type.to_ascii_uppercase().contains("BOOL") {
                violations.push(Violation::BooleanNotCanonical {
                    table: table.name.clone(),
                    column: column.name.clone(),
                });
            } else if checked.contains(&column.name.to_ascii_lowercase()) {
                violations.push(Violation::BooleanUnregistered {
                    table: table.name.clone(),
                    column: column.name.clone(),
                });
            }
        }
    }
    violations
}

async fn read_migrated(path: &Path) -> Result<Schema> {
    let mut connection =
        SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path).read_only(true))
            .await
            .with_context(|| format!("open migrated database {}", path.display()))?;
    let applied: i64 = sqlx::query_scalar(&format!(
        "SELECT count(*) FROM {MIGRATIONS_TABLE} WHERE success = 1"
    ))
    .fetch_one(&mut connection)
    .await
    .context("read applied migrations")?;
    ensure!(applied > 0, "the test database has no applied migrations");
    let schema = Schema::introspect(&mut connection).await?;
    connection.close().await?;
    Ok(schema)
}

async fn introspect_table(
    connection: &mut SqliteConnection,
    name: &str,
    sql: String,
) -> Result<Table> {
    let columns: Vec<(String, String, i64)> = sqlx::query_as(
        r#"SELECT name, type, "notnull" FROM pragma_table_xinfo(?1) WHERE hidden <> 1 ORDER BY cid"#,
    )
    .bind(name)
    .fetch_all(&mut *connection)
    .await
    .with_context(|| format!("read columns of {name}"))?;

    let references: Vec<(i64, String, String, String)> = sqlx::query_as(
        r#"SELECT id, "from", "table", on_delete FROM pragma_foreign_key_list(?1) ORDER BY id, seq"#,
    )
    .bind(name)
    .fetch_all(&mut *connection)
    .await
    .with_context(|| format!("read foreign keys of {name}"))?;

    let mut foreign_keys: Vec<(i64, ForeignKey)> = Vec::new();
    for (id, column, parent, on_delete) in references {
        match foreign_keys.last_mut() {
            Some((last, foreign_key)) if *last == id => foreign_key.columns.push(column),
            _ => foreign_keys.push((
                id,
                ForeignKey {
                    columns: vec![column],
                    parent,
                    on_delete: on_delete.to_ascii_uppercase(),
                },
            )),
        }
    }

    let listed: Vec<(String, i64, i64)> = sqlx::query_as(
        r#"SELECT name, "unique", partial FROM pragma_index_list(?1) ORDER BY name"#,
    )
    .bind(name)
    .fetch_all(&mut *connection)
    .await
    .with_context(|| format!("read indexes of {name}"))?;

    let mut indexes = Vec::with_capacity(listed.len());
    for (index, unique, partial) in listed {
        let keys: Vec<(Option<String>, String)> = sqlx::query_as(
            "SELECT name, coll FROM pragma_index_xinfo(?1) WHERE key = 1 ORDER BY seqno",
        )
        .bind(&index)
        .fetch_all(&mut *connection)
        .await
        .with_context(|| format!("read columns of index {index}"))?;
        indexes.push(Index {
            name: index,
            unique: unique != 0,
            partial: partial != 0,
            columns: keys
                .into_iter()
                .map(|(name, collation)| IndexColumn { name, collation })
                .collect(),
        });
    }

    Ok(Table {
        name: name.to_owned(),
        sql,
        columns: columns
            .into_iter()
            .map(|(name, declared_type, not_null)| Column {
                name,
                declared_type,
                not_null: not_null != 0,
            })
            .collect(),
        foreign_keys: foreign_keys
            .into_iter()
            .map(|(_, foreign_key)| foreign_key)
            .collect(),
        indexes,
    })
}

fn is_internal(name: &str) -> bool {
    name.starts_with("sqlite_") || name == MIGRATIONS_TABLE
}

fn without_comments(sql: &str) -> String {
    let mut output = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(current) = chars.next() {
        match current {
            '\'' | '"' | '`' | '[' => {
                let close = if current == '[' { ']' } else { current };
                output.push(current);
                for next in chars.by_ref() {
                    output.push(next);
                    if next == close {
                        break;
                    }
                }
            }
            '-' if chars.peek() == Some(&'-') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        break;
                    }
                }
                output.push(' ');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
                output.push(' ');
            }
            _ => output.push(current),
        }
    }
    output
}

fn words(sql: &str) -> Vec<String> {
    let lowered = without_comments(sql).to_ascii_lowercase();
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = lowered.chars();
    while let Some(character) = chars.next() {
        if character.is_ascii_alphanumeric() || character == '_' {
            current.push(character);
            continue;
        }
        if !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        if matches!(character, '\'' | '"' | '`' | '[') {
            let close = if character == '[' { ']' } else { character };
            words.push(chars.by_ref().take_while(|next| *next != close).collect());
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn boolean_checks(sql: &str) -> Vec<String> {
    const OPEN: &str = "check(";
    const RANGE: &str = "in(0,1))";
    let compact: String = without_comments(sql)
        .to_ascii_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    compact
        .match_indices(OPEN)
        .filter_map(|(start, _)| {
            let rest = &compact[start + OPEN.len()..];
            let candidate = rest.find(RANGE).map(|end| &rest[..end])?;
            (!candidate.is_empty()
                && candidate
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_'))
            .then(|| candidate.to_owned())
        })
        .collect()
}
