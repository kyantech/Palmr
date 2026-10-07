use sqlx::sqlite::SqliteQueryResult;

use crate::domain::error_code::ErrorCode;
use crate::domain::naming::{
    CandidateError, InvalidName, NameCandidate, NameSeries, MAX_NAME_ATTEMPTS,
};
use crate::infra::db::DbError;
use crate::infra::http::error::ApiError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameNamespace {
    FilesInFolder,
    FilesAtRoot,
    FoldersInFolder,
    FoldersAtRoot,
    ReceivedFiles,
}

impl NameNamespace {
    pub const fn conflict_clause(self) -> &'static str {
        match self {
            Self::FilesInFolder => {
                "ON CONFLICT (owner_id, folder_id, name_normalized) \
                 WHERE folder_id IS NOT NULL DO NOTHING"
            }
            Self::FilesAtRoot => {
                "ON CONFLICT (owner_id, name_normalized) WHERE folder_id IS NULL DO NOTHING"
            }
            Self::FoldersInFolder => {
                "ON CONFLICT (owner_id, parent_id, name_normalized) \
                 WHERE parent_id IS NOT NULL DO NOTHING"
            }
            Self::FoldersAtRoot => {
                "ON CONFLICT (owner_id, name_normalized) WHERE parent_id IS NULL DO NOTHING"
            }
            Self::ReceivedFiles => "ON CONFLICT (reverse_share_id, name_normalized) DO NOTHING",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt<T> {
    Stored(T),
    NameTaken,
}

impl Attempt<()> {
    pub fn from_insert(result: &SqliteQueryResult) -> Self {
        if result.rows_affected() == 0 {
            Self::NameTaken
        } else {
            Self::Stored(())
        }
    }

    pub fn from_name_update(
        result: Result<SqliteQueryResult, sqlx::Error>,
    ) -> Result<Self, DbError> {
        match result {
            Ok(_) => Ok(Self::Stored(())),
            Err(source) => match DbError::from(source) {
                DbError::UniqueViolation(_) => Ok(Self::NameTaken),
                other => Err(other),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored<T> {
    pub name: NameCandidate,
    pub attempt: u32,
    pub value: T,
}

impl<T> Stored<T> {
    pub const fn renamed(&self) -> bool {
        self.attempt > 0
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum NamedInsertError<E> {
    InvalidName(InvalidName),
    Conflict(CandidateError),
    Failed(E),
}

impl<E> From<E> for NamedInsertError<E> {
    fn from(error: E) -> Self {
        Self::Failed(error)
    }
}

impl From<InvalidName> for ErrorCode {
    fn from(_: InvalidName) -> Self {
        Self::NameInvalid
    }
}

impl From<CandidateError> for ErrorCode {
    fn from(_: CandidateError) -> Self {
        Self::FileNameConflict
    }
}

impl From<InvalidName> for ApiError {
    fn from(error: InvalidName) -> Self {
        Self::new(error.into())
    }
}

impl From<CandidateError> for ApiError {
    fn from(error: CandidateError) -> Self {
        Self::new(error.into())
    }
}

impl<E: Into<ApiError>> From<NamedInsertError<E>> for ApiError {
    fn from(error: NamedInsertError<E>) -> Self {
        match error {
            NamedInsertError::InvalidName(invalid) => invalid.into(),
            NamedInsertError::Conflict(conflict) => conflict.into(),
            NamedInsertError::Failed(source) => source.into(),
        }
    }
}

pub async fn insert_with_unique_name<T, E>(
    requested: &str,
    mut attempt: impl AsyncFnMut(NameCandidate) -> Result<Attempt<T>, E>,
) -> Result<Stored<T>, NamedInsertError<E>> {
    let series = NameSeries::parse(requested).map_err(NamedInsertError::InvalidName)?;
    for number in 0..=MAX_NAME_ATTEMPTS {
        let name = series
            .candidate(number)
            .map_err(NamedInsertError::Conflict)?;
        match attempt(name.clone())
            .await
            .map_err(NamedInsertError::Failed)?
        {
            Attempt::Stored(value) => {
                return Ok(Stored {
                    name,
                    attempt: number,
                    value,
                })
            }
            Attempt::NameTaken => {}
        }
    }
    Err(NamedInsertError::Conflict(
        CandidateError::AttemptsExhausted,
    ))
}
