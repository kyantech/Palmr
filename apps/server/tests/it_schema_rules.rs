pub mod support;

use support::schema::{
    all_violations, byte_owner_cascades, implicit_delete_actions, migrated_schema,
    nocase_collations, non_canonical_booleans, non_text_timestamps, scratch_schema, IndexColumn,
    Violation,
};

const CONTENT_GRAPH: &str = "
CREATE TABLE users (id TEXT NOT NULL PRIMARY KEY);
CREATE TABLE storage_objects (id TEXT NOT NULL PRIMARY KEY);
CREATE TABLE files (
    id                TEXT NOT NULL PRIMARY KEY,
    owner_id          TEXT NOT NULL,
    storage_object_id TEXT NOT NULL,
    FOREIGN KEY (owner_id)          REFERENCES users(id)           ON DELETE RESTRICT,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
);
CREATE TABLE received_files (
    id                TEXT NOT NULL PRIMARY KEY,
    owner_id          TEXT NOT NULL,
    storage_object_id TEXT NOT NULL,
    FOREIGN KEY (owner_id)          REFERENCES users(id)           ON DELETE RESTRICT,
    FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
);
CREATE TABLE transfer_sessions (
    id      TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE TABLE transfer_session_files (
    id                         TEXT NOT NULL PRIMARY KEY,
    transfer_session_id        TEXT NOT NULL,
    resulting_file_id          TEXT NULL,
    resulting_received_file_id TEXT NULL,
    FOREIGN KEY (transfer_session_id)        REFERENCES transfer_sessions(id) ON DELETE CASCADE,
    FOREIGN KEY (resulting_file_id)          REFERENCES files(id)             ON DELETE SET NULL,
    FOREIGN KEY (resulting_received_file_id) REFERENCES received_files(id)    ON DELETE SET NULL
);
";

const EMBED_GRANTS: &str = "
CREATE TABLE embed_grants (
    id       TEXT NOT NULL PRIMARY KEY,
    file_id  TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    FOREIGN KEY (file_id)  REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE RESTRICT
);
";

const PARENT: &str = "CREATE TABLE parents (id TEXT NOT NULL PRIMARY KEY);";

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn cascade(table: &str, column: &str, parent: &str) -> Violation {
    Violation::ByteOwnerCascade {
        table: table.to_owned(),
        columns: strings(&[column]),
        parent: parent.to_owned(),
    }
}

fn implicit(table: &str, column: &str, on_delete: &str) -> Violation {
    Violation::ImplicitDeleteAction {
        table: table.to_owned(),
        columns: strings(&[column]),
        parent: "parents".to_owned(),
        on_delete: on_delete.to_owned(),
    }
}

fn nocase(object: &str) -> Violation {
    Violation::NocaseCollation {
        object: object.to_owned(),
    }
}

fn timestamp(column: &str, declared_type: &str) -> Violation {
    Violation::TimestampNotText {
        table: "events".to_owned(),
        column: column.to_owned(),
        declared_type: declared_type.to_owned(),
    }
}

fn boolean(table: &str, column: &str) -> Violation {
    Violation::BooleanNotCanonical {
        table: table.to_owned(),
        column: column.to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_migrated_database_passes_every_rule() -> anyhow::Result<()> {
    let schema = migrated_schema("it_schema_migrated_database_passes_every_rule").await?;
    assert_eq!(all_violations(&schema), []);
    assert!(schema
        .table_names()
        .iter()
        .all(|name| !name.starts_with("sqlite_") && *name != "_sqlx_migrations"));
    Ok(())
}

#[tokio::test]
async fn it_schema_kit_introspects_structure() -> anyhow::Result<()> {
    let schema = scratch_schema(&format!(
        "{PARENT}
        CREATE TABLE children (
            id        TEXT    NOT NULL PRIMARY KEY,
            parent_id TEXT    NULL,
            label     TEXT    NOT NULL,
            weight    INTEGER,
            FOREIGN KEY (parent_id) REFERENCES parents(id) ON DELETE SET NULL
        );
        CREATE TABLE pairs (
            left_id  TEXT NOT NULL,
            right_id TEXT NOT NULL,
            PRIMARY KEY (left_id, right_id),
            FOREIGN KEY (left_id, right_id) REFERENCES children(id, label) ON DELETE CASCADE
        ) WITHOUT ROWID;
        CREATE UNIQUE INDEX ux_children_label ON children(label COLLATE BINARY) WHERE parent_id IS NOT NULL;"
    ))
    .await?;

    assert_eq!(schema.table_names(), ["children", "pairs", "parents"]);
    let children = schema.table("children").expect("children introspected");
    let columns: Vec<(&str, &str, bool)> = children
        .columns
        .iter()
        .map(|column| {
            (
                column.name.as_str(),
                column.declared_type.as_str(),
                column.not_null,
            )
        })
        .collect();
    assert_eq!(
        columns,
        [
            ("id", "TEXT", true),
            ("parent_id", "TEXT", false),
            ("label", "TEXT", true),
            ("weight", "INTEGER", false),
        ]
    );
    assert_eq!(children.foreign_keys.len(), 1);
    assert_eq!(children.foreign_keys[0].columns, ["parent_id"]);
    assert_eq!(children.foreign_keys[0].parent, "parents");
    assert_eq!(children.foreign_keys[0].on_delete, "SET NULL");

    let index = children
        .indexes
        .iter()
        .find(|index| index.name == "ux_children_label")
        .expect("partial unique index introspected");
    assert!(index.unique && index.partial);
    assert_eq!(
        index.columns,
        [IndexColumn {
            name: Some("label".to_owned()),
            collation: "BINARY".to_owned(),
        }]
    );

    let pairs = schema.table("pairs").expect("pairs introspected");
    assert_eq!(pairs.foreign_keys.len(), 1);
    assert_eq!(pairs.foreign_keys[0].columns, ["left_id", "right_id"]);
    assert_eq!(pairs.foreign_keys[0].on_delete, "CASCADE");
    Ok(())
}

#[tokio::test]
async fn it_schema_kit_ignores_internal_tables() -> anyhow::Result<()> {
    let schema = scratch_schema(
        "CREATE TABLE _sqlx_migrations (
            version        BIGINT PRIMARY KEY,
            description    TEXT NOT NULL,
            installed_on   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success        BOOLEAN NOT NULL,
            checksum       BLOB NOT NULL,
            execution_time BIGINT NOT NULL
        );
        CREATE TABLE counters (id INTEGER PRIMARY KEY AUTOINCREMENT, label TEXT NOT NULL);
        INSERT INTO counters (label) VALUES ('first');",
    )
    .await?;

    assert_eq!(schema.table_names(), ["counters"]);
    assert_eq!(all_violations(&schema), []);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_no_cascade_on_byte_owners() -> anyhow::Result<()> {
    let migrated = migrated_schema("it_schema_no_cascade_on_byte_owners").await?;
    assert_eq!(byte_owner_cascades(&migrated), []);

    let accepted = scratch_schema(&format!("{CONTENT_GRAPH}{EMBED_GRANTS}")).await?;
    assert_eq!(byte_owner_cascades(&accepted), []);
    assert_eq!(implicit_delete_actions(&accepted), []);
    assert!(!accepted
        .table("embed_grants")
        .expect("embed_grants")
        .owns_bytes());
    assert!(!accepted
        .table("transfer_session_files")
        .expect("transfer_session_files")
        .owns_bytes());
    assert!(accepted.table("files").expect("files").owns_bytes());
    assert!(accepted
        .table("storage_objects")
        .expect("storage_objects")
        .owns_bytes());

    let owner_cascade = scratch_schema(&format!(
        "{CONTENT_GRAPH}
        CREATE TABLE avatars (
            id                TEXT NOT NULL PRIMARY KEY,
            user_id           TEXT NOT NULL,
            storage_object_id TEXT NOT NULL,
            FOREIGN KEY (user_id)           REFERENCES users(id)           ON DELETE CASCADE,
            FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
        );"
    ))
    .await?;
    assert_eq!(
        byte_owner_cascades(&owner_cascade),
        [cascade("avatars", "user_id", "users")]
    );

    let byte_holding_embed = scratch_schema(&format!(
        "{CONTENT_GRAPH}
        CREATE TABLE embed_grants (
            id                TEXT NOT NULL PRIMARY KEY,
            file_id           TEXT NOT NULL,
            storage_object_id TEXT NOT NULL,
            FOREIGN KEY (file_id)           REFERENCES files(id)           ON DELETE CASCADE,
            FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE RESTRICT
        );"
    ))
    .await?;
    assert_eq!(
        byte_owner_cascades(&byte_holding_embed),
        [cascade("embed_grants", "file_id", "files")]
    );

    let prefixed_column = scratch_schema(&format!(
        "{CONTENT_GRAPH}
        CREATE TABLE profiles (
            id                       TEXT NOT NULL PRIMARY KEY,
            user_id                  TEXT NOT NULL,
            avatar_storage_object_id TEXT NULL,
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
        );"
    ))
    .await?;
    assert_eq!(
        byte_owner_cascades(&prefixed_column),
        [cascade("profiles", "user_id", "users")]
    );

    let object_cascade = scratch_schema(&format!(
        "{CONTENT_GRAPH}
        CREATE TABLE blobs (
            id        TEXT NOT NULL PRIMARY KEY,
            object_id TEXT NOT NULL,
            FOREIGN KEY (object_id) REFERENCES storage_objects(id) ON DELETE CASCADE
        );"
    ))
    .await?;
    assert_eq!(
        byte_owner_cascades(&object_cascade),
        [cascade("blobs", "object_id", "storage_objects")]
    );

    let object_set_null = scratch_schema(&format!(
        "{CONTENT_GRAPH}
        CREATE TABLE covers (
            id                TEXT NOT NULL PRIMARY KEY,
            storage_object_id TEXT NULL,
            FOREIGN KEY (storage_object_id) REFERENCES storage_objects(id) ON DELETE SET NULL
        );"
    ))
    .await?;
    assert_eq!(
        byte_owner_cascades(&object_set_null),
        [Violation::StorageObjectNotRestrict {
            table: "covers".to_owned(),
            columns: strings(&["storage_object_id"]),
            on_delete: "SET NULL".to_owned(),
        }]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_fk_actions_explicit() -> anyhow::Result<()> {
    let migrated = migrated_schema("it_schema_fk_actions_explicit").await?;
    assert_eq!(implicit_delete_actions(&migrated), []);

    let explicit = scratch_schema(&format!(
        "{PARENT}
        CREATE TABLE explicit_children (
            id          TEXT NOT NULL PRIMARY KEY,
            restrict_id TEXT NOT NULL REFERENCES parents(id) ON DELETE RESTRICT,
            cascade_id  TEXT NOT NULL,
            set_null_id TEXT NULL,
            FOREIGN KEY (cascade_id)  REFERENCES parents(id) ON DELETE CASCADE,
            FOREIGN KEY (set_null_id) REFERENCES parents(id) ON DELETE SET NULL
        );"
    ))
    .await?;
    assert_eq!(implicit_delete_actions(&explicit), []);

    let violating = scratch_schema(&format!(
        "{PARENT}
        CREATE TABLE table_level (
            id        TEXT NOT NULL PRIMARY KEY,
            parent_id TEXT NOT NULL,
            FOREIGN KEY (parent_id) REFERENCES parents(id)
        );
        CREATE TABLE column_level (
            id        TEXT NOT NULL PRIMARY KEY,
            parent_id TEXT NOT NULL REFERENCES parents(id)
        );
        CREATE TABLE update_only (
            id        TEXT NOT NULL PRIMARY KEY,
            parent_id TEXT NOT NULL REFERENCES parents(id) ON UPDATE CASCADE
        );
        CREATE TABLE spelled_no_action (
            id        TEXT NOT NULL PRIMARY KEY,
            parent_id TEXT NOT NULL REFERENCES parents(id) ON DELETE NO ACTION
        );
        CREATE TABLE set_default (
            id        TEXT NOT NULL PRIMARY KEY,
            parent_id TEXT NULL DEFAULT NULL REFERENCES parents(id) ON DELETE SET DEFAULT
        );"
    ))
    .await?;
    let mut found = implicit_delete_actions(&violating);
    found.sort_by_key(|violation| format!("{violation:?}"));
    let mut expected = vec![
        implicit("table_level", "parent_id", "NO ACTION"),
        implicit("column_level", "parent_id", "NO ACTION"),
        implicit("update_only", "parent_id", "NO ACTION"),
        implicit("spelled_no_action", "parent_id", "NO ACTION"),
        implicit("set_default", "parent_id", "SET DEFAULT"),
    ];
    expected.sort_by_key(|violation| format!("{violation:?}"));
    assert_eq!(found, expected);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_no_nocase_collation() -> anyhow::Result<()> {
    let migrated = migrated_schema("it_schema_no_nocase_collation").await?;
    assert_eq!(nocase_collations(&migrated), []);

    let accepted = scratch_schema(
        "CREATE TABLE aliases (
            id    TEXT NOT NULL PRIMARY KEY,
            alias TEXT NOT NULL COLLATE BINARY CHECK (alias NOT LIKE '%--%'),
            note  TEXT NULL CHECK (note IS NULL OR note <> 'collate nocase')
        );
        CREATE UNIQUE INDEX ux_aliases_alias ON aliases(alias);",
    )
    .await?;
    assert_eq!(nocase_collations(&accepted), []);

    let column = scratch_schema(
        "CREATE TABLE people (
            id    TEXT NOT NULL PRIMARY KEY,
            email TEXT NOT NULL COLLATE NOCASE UNIQUE
        );",
    )
    .await?;
    assert_eq!(
        nocase_collations(&column),
        [nocase("people"), nocase("sqlite_autoindex_people_2")]
    );

    let index = scratch_schema(
        "CREATE TABLE people (id TEXT NOT NULL PRIMARY KEY, email TEXT NOT NULL);
        CREATE UNIQUE INDEX ux_people_email ON people(email COLLATE nocase);",
    )
    .await?;
    assert_eq!(nocase_collations(&index), [nocase("ux_people_email")]);

    let view = scratch_schema(
        "CREATE TABLE people (id TEXT NOT NULL PRIMARY KEY, email TEXT NOT NULL);
        CREATE VIEW people_sorted AS SELECT id FROM people ORDER BY email COLLATE \"NOCASE\";",
    )
    .await?;
    assert_eq!(nocase_collations(&view), [nocase("people_sorted")]);

    let hidden_after_comment = scratch_schema(
        "CREATE TABLE people (
            id    TEXT NOT NULL PRIMARY KEY, -- identifier
            email TEXT NOT NULL /* compared case-insensitively */ COLLATE NoCase
        );",
    )
    .await?;
    assert_eq!(nocase_collations(&hidden_after_comment), [nocase("people")]);

    let quoted_name = scratch_schema(
        "CREATE TABLE people (id TEXT NOT NULL PRIMARY KEY, email TEXT NOT NULL COLLATE 'NOCASE');",
    )
    .await?;
    assert_eq!(nocase_collations(&quoted_name), [nocase("people")]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_timestamps_are_text() -> anyhow::Result<()> {
    let migrated = migrated_schema("it_schema_timestamps_are_text").await?;
    assert_eq!(non_text_timestamps(&migrated), []);

    let accepted = scratch_schema(
        "CREATE TABLE events (
            id                     TEXT    NOT NULL PRIMARY KEY,
            created_at             TEXT    NOT NULL,
            revoked_at             TEXT    NULL,
            observed_at            text,
            email_verified_at_link INTEGER NOT NULL DEFAULT 0 CHECK (email_verified_at_link IN (0,1))
        );",
    )
    .await?;
    assert_eq!(non_text_timestamps(&accepted), []);

    let violating = scratch_schema(
        "CREATE TABLE events (
            id         TEXT     NOT NULL PRIMARY KEY,
            created_at INTEGER  NOT NULL,
            expires_at DATETIME NULL,
            deleted_at,
            seen_at    TIMESTAMP
        );",
    )
    .await?;
    assert_eq!(
        non_text_timestamps(&violating),
        [
            timestamp("created_at", "INTEGER"),
            timestamp("expires_at", "DATETIME"),
            timestamp("deleted_at", ""),
            timestamp("seen_at", "TIMESTAMP"),
        ]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn it_schema_booleans_canonical() -> anyhow::Result<()> {
    let migrated = migrated_schema("it_schema_booleans_canonical").await?;
    assert_eq!(non_canonical_booleans(&migrated), []);

    let accepted = scratch_schema(
        "CREATE TABLE users (
            id                   TEXT    NOT NULL PRIMARY KEY,
            must_change_password INTEGER NOT NULL DEFAULT 0 CHECK (must_change_password IN (0,1)),
            is_active            INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
            totp_enabled         INTEGER NOT NULL DEFAULT 0
                                 CHECK ( totp_enabled IN ( 0 , 1 ) ),
            is_verified_count    INTEGER NOT NULL DEFAULT 0 CHECK (is_verified_count >= 0)
        );",
    )
    .await?;
    assert_eq!(non_canonical_booleans(&accepted), []);

    let violating = scratch_schema(
        "CREATE TABLE users (
            id                   TEXT    NOT NULL PRIMARY KEY,
            must_change_password INTEGER NOT NULL DEFAULT 0,
            is_active            INTEGER NULL CHECK (is_active IN (0,1)),
            totp_enabled         BOOLEAN NOT NULL DEFAULT 0 CHECK (totp_enabled IN (0,1))
        );
        CREATE TABLE shares (
            id                TEXT    NOT NULL PRIMARY KEY,
            is_active         INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0,1,2)),
            notify_recipients INTEGER NOT NULL DEFAULT 1 CHECK (notify_recipients BETWEEN 0 AND 1),
            show_owner        INTEGER NOT NULL DEFAULT 0 CHECK (show_owner IN (0,1)),
            is_pinned         BOOL    NOT NULL DEFAULT 0,
            is_featured       INTEGER NOT NULL DEFAULT 0 CHECK (is_featured IN (0,1))
        );",
    )
    .await?;
    assert_eq!(
        non_canonical_booleans(&violating),
        [
            boolean("shares", "is_active"),
            boolean("shares", "notify_recipients"),
            boolean("shares", "is_pinned"),
            Violation::BooleanUnregistered {
                table: "shares".to_owned(),
                column: "is_featured".to_owned(),
            },
            boolean("users", "must_change_password"),
            boolean("users", "is_active"),
            boolean("users", "totp_enabled"),
        ]
    );

    let missing =
        scratch_schema("CREATE TABLE app_settings (key TEXT NOT NULL PRIMARY KEY);").await?;
    assert_eq!(
        non_canonical_booleans(&missing),
        [boolean("app_settings", "is_secret")]
    );
    Ok(())
}
