use http_body_util::BodyExt;

pub async fn buffer_body(body: axum::body::Body) -> bytes::Bytes {
    body.collect().await.unwrap().to_bytes()
}
