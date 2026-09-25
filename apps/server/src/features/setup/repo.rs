use crate::infra::db::WriteTx;

use super::error::SetupError;

const SELECT_SETUP_COMPLETED: &str =
    "SELECT value_json FROM app_settings WHERE key = 'setup_completed'";

pub async fn setup_completed(tx: &mut WriteTx<'_>) -> Result<bool, SetupError> {
    let stored: Option<Option<String>> = sqlx::query_scalar(SELECT_SETUP_COMPLETED)
        .fetch_optional(tx.executor())
        .await?;
    match stored {
        None => Ok(false),
        Some(Some(json)) => match serde_json::from_str::<serde_json::Value>(&json) {
            Ok(serde_json::Value::Bool(flag)) => Ok(flag),
            _ => Err(SetupError::MalformedSetupFlag),
        },
        Some(None) => Err(SetupError::MalformedSetupFlag),
    }
}
