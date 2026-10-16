use std::sync::Arc;

use axum::body::Body;
use http::header::{IF_NONE_MATCH, IF_RANGE, RANGE};
use http::{HeaderMap, HeaderValue};
use sqlx::Row;
use time::OffsetDateTime;

use super::error::DownloadError;
use super::model::{
    effective_disposition, served_content_type, EntityTag, StoredFile, PRESIGN_TTL,
};
use super::stream::{ContentStream, DownloadPermits};
use crate::domain::content_disposition::{ContentDisposition, DispositionType};
use crate::domain::http_range::{self, ByteRange, CurrentValidators, RangeDecision};
use crate::features::files::FileId;
use crate::features::folders::visibility::folder_hidden_sql;
use crate::features::users::model::UserId;
use crate::infra::db::DbPools;
use crate::infra::http::etag::matches_validator;
use crate::storage::key::ObjectKey;
use crate::storage::provider::{GrantContext, ObjectStat, StorageProvider};

const RESOLVE: &str = concat!(
    "SELECT f.name, f.mime_type, o.size_bytes, o.checksum, o.object_key, o.provider \
       FROM files f JOIN storage_objects o ON o.id = f.storage_object_id \
      WHERE f.id = ?1 AND f.owner_id = ?2 AND o.state = 'active' AND NOT ",
    folder_hidden_sql!("f.folder_id", "f.owner_id")
);

#[derive(Clone)]
pub struct DownloadService {
    pools: DbPools,
    storage: Arc<dyn StorageProvider>,
    permits: DownloadPermits,
}

pub struct DownloadRequest<'a> {
    pub owner: UserId,
    pub file: FileId,
    pub disposition: DispositionType,
    pub headers: &'a HeaderMap,
}

pub struct Representation {
    pub content_type: String,
    pub disposition: ContentDisposition,
    pub etag: EntityTag,
    pub modified_at: OffsetDateTime,
}

pub struct Content {
    pub representation: Representation,
    pub size: u64,
    pub range: Option<ByteRange>,
    pub stream: ContentStream,
}

impl Content {
    pub const fn length(&self) -> u64 {
        match self.range {
            Some(range) => range.length(),
            None => self.size,
        }
    }

    pub fn into_body(self, request_id: Option<&str>) -> Body {
        self.stream.with_request_id(request_id).into_body()
    }
}

pub enum Delivery {
    Redirect(HeaderValue),
    NotModified(Representation),
    Content(Content),
}

impl DownloadService {
    pub fn new(pools: DbPools, storage: Arc<dyn StorageProvider>) -> Self {
        Self {
            pools,
            storage,
            permits: DownloadPermits::production(),
        }
    }

    pub async fn deliver(&self, request: DownloadRequest<'_>) -> Result<Delivery, DownloadError> {
        let stored = self.resolve(request.owner, request.file).await?;
        let configured = self.storage.describe().provider.as_str();
        if stored.provider != configured {
            return Err(DownloadError::ProviderMismatch {
                recorded: stored.provider.clone(),
                configured,
            });
        }
        let key =
            ObjectKey::parse(&stored.object_key).map_err(|_| DownloadError::CorruptRecord {
                column: "object_key",
            })?;
        let content_type = served_content_type(&stored.mime_type).to_owned();
        let disposition = ContentDisposition::new(
            effective_disposition(request.disposition, &content_type),
            &stored.name,
        );

        let headers = request.headers;
        let can_redirect = self.storage.as_presign().is_some();
        let has_validator_request = headers.contains_key(IF_NONE_MATCH);
        let provider_cannot_decide = headers.contains_key(RANGE) && headers.contains_key(IF_RANGE);

        if can_redirect && !has_validator_request && !provider_cannot_decide {
            if let RangeDecision::Unsatisfiable(unsatisfied) = http_range::decide(
                single_header(headers, &RANGE),
                None,
                CurrentValidators::default(),
                stored.size_bytes,
            ) {
                return Err(DownloadError::RangeNotSatisfiable {
                    size: unsatisfied.size(),
                });
            }
            return self.redirect(&key, &disposition, &content_type).await;
        }

        let stat = match self.storage.stat(&key).await {
            Ok(stat) => stat,
            Err(error) => return Err(self.read_failure(request.owner, request.file, error).await),
        };
        if stat.size != stored.size_bytes {
            return Err(DownloadError::SizeMismatch {
                recorded: stored.size_bytes,
                actual: stat.size,
            });
        }
        let etag = EntityTag::of(stored.checksum.as_deref(), &stat);
        let provider_tag = provider_tag(&stat);
        let if_range = if_range_header(headers);
        let current_tag = match (if_range, provider_tag.as_deref()) {
            (Some(held), Some(provider)) if trim_ows(held) == provider.as_bytes() => {
                provider.to_owned()
            }
            _ => etag.as_str().to_owned(),
        };
        let representation = Representation {
            content_type,
            disposition,
            etag,
            modified_at: stat.modified_at,
        };
        let tags = [Some(representation.etag.as_str()), provider_tag.as_deref()];
        if tags
            .into_iter()
            .flatten()
            .filter_map(|tag| HeaderValue::from_str(tag).ok())
            .any(|tag| matches_validator(headers, &tag))
        {
            return Ok(Delivery::NotModified(representation));
        }

        let decision = http_range::decide(
            single_header(headers, &RANGE),
            if_range,
            CurrentValidators {
                etag: Some(current_tag.as_str()),
                last_modified: Some(stat.modified_at),
            },
            stat.size,
        );
        let range = match decision {
            RangeDecision::Full => None,
            RangeDecision::Partial(range) => Some(range),
            RangeDecision::Unsatisfiable(unsatisfied) => {
                return Err(DownloadError::RangeNotSatisfiable {
                    size: unsatisfied.size(),
                })
            }
        };
        if can_redirect && !provider_cannot_decide {
            return self
                .redirect(
                    &key,
                    &representation.disposition,
                    &representation.content_type,
                )
                .await;
        }
        let length = range.map_or(stat.size, ByteRange::length);
        if length == 0 {
            return Ok(Delivery::Content(Content {
                representation,
                size: stat.size,
                range,
                stream: ContentStream::empty(),
            }));
        }

        let permit = self.permits.acquire().await?;
        let opening = match range {
            Some(range) => {
                self.storage
                    .open_range(&key, range.start(), range.length())
                    .await
            }
            None => self.storage.open_read(&key).await,
        };
        let (opened, body) = match opening {
            Ok(opened) => opened,
            Err(error) => return Err(self.read_failure(request.owner, request.file, error).await),
        };
        if opened.size != stat.size {
            return Err(DownloadError::SizeMismatch {
                recorded: stat.size,
                actual: opened.size,
            });
        }
        Ok(Delivery::Content(Content {
            representation,
            size: stat.size,
            range,
            stream: ContentStream::new(body, length, permit),
        }))
    }

    async fn redirect(
        &self,
        key: &ObjectKey,
        disposition: &ContentDisposition,
        content_type: &str,
    ) -> Result<Delivery, DownloadError> {
        let presign = self
            .storage
            .as_presign()
            .ok_or(DownloadError::CorruptRecord { column: "presign" })?;
        let signed = presign
            .presign_get(
                key,
                PRESIGN_TTL,
                &disposition.to_header_value(),
                content_type,
                &GrantContext::after_authorization(),
            )
            .await
            .map_err(DownloadError::from_storage)?;
        let mut location = HeaderValue::from_str(signed.url().as_str())
            .map_err(|_| DownloadError::CorruptRecord { column: "location" })?;
        location.set_sensitive(true);
        Ok(Delivery::Redirect(location))
    }

    async fn resolve(&self, owner: UserId, file: FileId) -> Result<StoredFile, DownloadError> {
        let row = sqlx::query(RESOLVE)
            .bind(file.to_string())
            .bind(owner.to_string())
            .fetch_optional(self.pools.reader().executor())
            .await?
            .ok_or(DownloadError::NotFound)?;
        let invalid = |column: &'static str| DownloadError::CorruptRecord { column };
        let size: i64 = row
            .try_get("size_bytes")
            .map_err(|_| invalid("size_bytes"))?;
        Ok(StoredFile {
            name: row.try_get("name").map_err(|_| invalid("name"))?,
            mime_type: row.try_get("mime_type").map_err(|_| invalid("mime_type"))?,
            size_bytes: u64::try_from(size).map_err(|_| invalid("size_bytes"))?,
            checksum: row.try_get("checksum").map_err(|_| invalid("checksum"))?,
            object_key: row
                .try_get("object_key")
                .map_err(|_| invalid("object_key"))?,
            provider: row.try_get("provider").map_err(|_| invalid("provider"))?,
        })
    }

    async fn read_failure(
        &self,
        owner: UserId,
        file: FileId,
        error: crate::storage::error::StorageError,
    ) -> DownloadError {
        let failure = DownloadError::from_storage(error);
        if matches!(failure, DownloadError::ObjectMissing) {
            if let Err(DownloadError::NotFound) = self.resolve(owner, file).await {
                return DownloadError::NotFound;
            }
        }
        failure
    }
}

fn single_header<'a>(headers: &'a HeaderMap, name: &http::HeaderName) -> Option<&'a [u8]> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?;
    values.next().is_none().then(|| first.as_bytes())
}

fn if_range_header(headers: &HeaderMap) -> Option<&[u8]> {
    let mut values = headers.get_all(IF_RANGE).iter();
    let first = values.next()?;
    Some(if values.next().is_none() {
        first.as_bytes()
    } else {
        &[]
    })
}

fn provider_tag(stat: &ObjectStat) -> Option<String> {
    let tag = stat.etag.as_ref()?.as_str().trim();
    if tag.is_empty() {
        None
    } else if tag.starts_with('"') || tag.starts_with("W/") {
        Some(tag.to_owned())
    } else {
        Some(format!("\"{tag}\""))
    }
}

fn trim_ows(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}
