use std::time::Duration;

use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection};
use tempfile::TempDir;

use super::{DbError, DbErrorKind, DATABASE_FILE};
use crate::domain::error_code::ErrorCode;
use crate::infra::http::error::ApiError;

async fn connect(root: &TempDir) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(root.path().join(DATABASE_FILE))
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(Duration::ZERO),
    )
    .await
    .unwrap()
}

async fn failure(connection: &mut SqliteConnection, sql: &str) -> DbError {
    DbError::from(
        sqlx::raw_sql(sql)
            .execute(&mut *connection)
            .await
            .unwrap_err(),
    )
}

fn assert_internal(error: DbError, kind: DbErrorKind) {
    assert_eq!(error.kind(), kind, "{error}");
    let api = ApiError::from(error);
    assert_eq!(api.code(), ErrorCode::InternalError);
}

#[tokio::test]
async fn unit_sqlite_error_mapping() {
    let root = TempDir::new().unwrap();
    let mut connection = connect(&root).await;
    sqlx::raw_sql(
        "CREATE TABLE parent(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
         CREATE TABLE child(
             id INTEGER PRIMARY KEY,
             parent_id INTEGER NOT NULL REFERENCES parent(id),
             size INTEGER NOT NULL CHECK (size >= 0)
         );
         INSERT INTO parent(id, name) VALUES (1, 'a');",
    )
    .execute(&mut connection)
    .await
    .unwrap();

    let unique = failure(
        &mut connection,
        "INSERT INTO parent(id, name) VALUES (2, 'a')",
    )
    .await;
    assert!(matches!(unique, DbError::UniqueViolation(_)));
    assert_internal(unique, DbErrorKind::UniqueViolation);

    let primary_key = failure(
        &mut connection,
        "INSERT INTO parent(id, name) VALUES (1, 'b')",
    )
    .await;
    assert_internal(primary_key, DbErrorKind::UniqueViolation);

    let check = failure(
        &mut connection,
        "INSERT INTO child(parent_id, size) VALUES (1, -1)",
    )
    .await;
    assert!(matches!(check, DbError::CheckViolation(_)));
    assert_internal(check, DbErrorKind::CheckViolation);

    let foreign_key = failure(
        &mut connection,
        "INSERT INTO child(parent_id, size) VALUES (42, 0)",
    )
    .await;
    assert!(matches!(foreign_key, DbError::ForeignKeyViolation(_)));
    assert_internal(foreign_key, DbErrorKind::ForeignKeyViolation);

    let not_null = failure(
        &mut connection,
        "INSERT INTO parent(id, name) VALUES (3, NULL)",
    )
    .await;
    assert_internal(not_null, DbErrorKind::Other);
    let missing_table = failure(&mut connection, "SELECT * FROM absent").await;
    assert!(matches!(missing_table, DbError::Other(_)));
    assert_internal(missing_table, DbErrorKind::Other);
    assert_internal(DbError::from(sqlx::Error::RowNotFound), DbErrorKind::Other);

    let mut holder = connect(&root).await;
    sqlx::raw_sql("BEGIN IMMEDIATE")
        .execute(&mut holder)
        .await
        .unwrap();
    let busy = failure(&mut connection, "BEGIN IMMEDIATE").await;
    assert!(matches!(busy, DbError::Busy(_)));
    let code = busy
        .source_error()
        .as_database_error()
        .and_then(|database| database.code())
        .unwrap();
    assert_eq!(code.parse::<i32>().unwrap() & 0xff, 5);
    let api = ApiError::from(busy);
    assert_eq!(api.code(), ErrorCode::DatabaseBusy);
    assert_eq!(api.status().as_u16(), 503);
    assert!(api.retryable());

    let queue_timeout = ApiError::from(DbError::from(sqlx::Error::PoolTimedOut));
    assert_eq!(queue_timeout.code(), ErrorCode::DatabaseBusy);

    sqlx::raw_sql("ROLLBACK")
        .execute(&mut holder)
        .await
        .unwrap();
    sqlx::raw_sql("INSERT INTO parent(id, name) VALUES (2, 'b')")
        .execute(&mut connection)
        .await
        .unwrap();
    holder.close().await.unwrap();
    connection.close().await.unwrap();
}
