use super::object::{malformed, measured_size, object_stat};
use super::{classify, Operation, S3Provider};
use crate::storage::error::StorageError;
use crate::storage::provider::{ListCursor, ListEntry, ListPage, MAX_LIST_PAGE_SIZE};

impl S3Provider {
    pub(crate) async fn list_objects_page(
        &self,
        prefix: &str,
        cursor: Option<ListCursor>,
        page_size: u32,
    ) -> Result<ListPage, StorageError> {
        let Some(max_keys) = max_keys(page_size) else {
            return Ok(ListPage {
                entries: Vec::new(),
                next: None,
            });
        };
        let output = self
            .internal()
            .list_objects_v2()
            .bucket(self.bucket())
            .prefix(prefix)
            .max_keys(max_keys)
            .set_continuation_token(cursor.map(|cursor| cursor.as_str().to_owned()))
            .send()
            .await
            .map_err(|error| classify(Operation::ListObjectsV2, error))?;

        let entries = output
            .contents()
            .iter()
            .map(|object| {
                let key = object
                    .key()
                    .ok_or_else(|| malformed(Operation::ListObjectsV2, "an entry has no key"))?;
                let size = measured_size(Operation::ListObjectsV2, object.size())?;
                let stat =
                    object_stat(Operation::ListObjectsV2, size, object.last_modified(), None)?;
                Ok(ListEntry {
                    key: key.to_owned(),
                    size: stat.size,
                    modified_at: stat.modified_at,
                })
            })
            .collect::<Result<Vec<_>, StorageError>>()?;

        let next = match (output.is_truncated(), output.next_continuation_token()) {
            (Some(true), Some(token)) if !token.is_empty() => Some(ListCursor::new(token)),
            (Some(true), _) => {
                return Err(malformed(
                    Operation::ListObjectsV2,
                    "a truncated page has no continuation token",
                ))
            }
            _ => None,
        };
        Ok(ListPage { entries, next })
    }
}

pub(super) fn max_keys(page_size: u32) -> Option<i32> {
    i32::try_from(page_size.min(MAX_LIST_PAGE_SIZE))
        .ok()
        .filter(|keys| *keys > 0)
}
