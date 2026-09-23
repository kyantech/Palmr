use std::path::Path;

pub async fn load(path: &Path) -> std::io::Result<Vec<u8>> {
    tokio::fs::read(path).await
}

pub fn load_blocking(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}
