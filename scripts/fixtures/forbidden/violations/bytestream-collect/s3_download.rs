use aws_smithy_types::byte_stream::ByteStream;

pub async fn fetch(client: &aws_sdk_s3::Client, key: &str) -> Vec<u8> {
    let object = client.get_object().key(key).send().await.unwrap();
    let data = object.body.collect().await.unwrap();
    data.into_bytes().to_vec()
}

pub async fn drain(stream: ByteStream) -> bytes::Bytes {
    ByteStream::collect(stream).await.unwrap().into_bytes()
}
