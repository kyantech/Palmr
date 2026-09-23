use tokio::io::AsyncReadExt;

pub async fn buffer_object(mut file: tokio::fs::File) -> std::io::Result<Vec<u8>> {
    let mut whole = Vec::new();
    file.read_to_end(&mut whole).await?;
    Ok(whole)
}
