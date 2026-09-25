use sqlx::Row;

use super::model::BrandingAsset;
use crate::infra::db::{DbError, ReadPool};

const SELECT_CURRENT: &str = "SELECT b.mime_type, o.id AS storage_object_id, o.object_key \
     FROM branding_assets b \
     JOIN storage_objects o ON o.id = b.storage_object_id \
     WHERE b.kind = ?1 AND b.is_current = 1 AND o.state = 'active'";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentAsset {
    pub mime_type: String,
    pub storage_object_id: String,
    pub object_key: String,
}

pub async fn current(
    reader: &ReadPool,
    asset: BrandingAsset,
) -> Result<Option<CurrentAsset>, DbError> {
    let row = sqlx::query(SELECT_CURRENT)
        .bind(asset.kind())
        .fetch_optional(reader.executor())
        .await?;
    row.map(|row| -> Result<CurrentAsset, sqlx::Error> {
        Ok(CurrentAsset {
            mime_type: row.try_get("mime_type")?,
            storage_object_id: row.try_get("storage_object_id")?,
            object_key: row.try_get("object_key")?,
        })
    })
    .transpose()
    .map_err(DbError::from)
}
