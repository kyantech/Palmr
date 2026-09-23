use axum::body::{to_bytes, Body};

pub async fn upload(body: Body) -> Result<(), axum::Error> {
    let bytes = axum::body::to_bytes(body, usize::MAX).await?;
    drop(bytes);
    Ok(())
}
