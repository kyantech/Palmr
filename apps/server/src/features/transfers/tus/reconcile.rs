use crate::storage::error::StorageError;
use crate::storage::staging::StagingAppend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reconciled {
    pub authoritative: u64,
    pub recorded: u64,
    pub staged: u64,
}

impl Reconciled {
    pub const fn torn_tail(&self) -> bool {
        self.staged > self.authoritative
    }

    pub const fn phantom_offset(&self) -> bool {
        self.recorded > self.authoritative
    }
}

pub const fn authoritative(recorded: u64, staged: u64) -> u64 {
    if recorded < staged {
        recorded
    } else {
        staged
    }
}

pub async fn truncate_to_authoritative(
    writer: &mut dyn StagingAppend,
    recorded: u64,
) -> Result<Reconciled, StorageError> {
    let staged = writer.len().await?;
    let authoritative = authoritative(recorded, staged);
    writer.truncate(authoritative).await?;
    Ok(Reconciled {
        authoritative,
        recorded,
        staged,
    })
}

#[cfg(test)]
mod tests {
    use super::{authoritative, Reconciled};

    #[test]
    fn unit_authoritative_offset_is_the_minimum_never_the_maximum() {
        for (recorded, staged, expected) in [
            (0, 0, 0),
            (8, 14, 8),
            (14, 8, 8),
            (9, 9, 9),
            (0, 99, 0),
            (99, 0, 0),
            (u64::MAX, 5, 5),
        ] {
            assert_eq!(authoritative(recorded, staged), expected);
            let case = Reconciled {
                authoritative: expected,
                recorded,
                staged,
            };
            assert_eq!(case.torn_tail(), staged > expected);
            assert_eq!(case.phantom_offset(), recorded > expected);
        }
    }
}
