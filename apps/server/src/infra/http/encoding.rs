use axum::body::HttpBody;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::header::{CONTENT_ENCODING, CONTENT_TYPE};
use http::HeaderMap;
use tower_http::compression::predicate::{Predicate, SizeAbove};
use tower_http::compression::CompressionLayer;
use tower_http::decompression::RequestDecompressionLayer;

use super::error::ApiError;
use super::request_id::{tag_error, RequestId};
use crate::domain::error_code::ErrorCode;

const COMPRESSIBLE_TYPES: [&str; 5] = [
    "application/json",
    "text/html",
    "text/css",
    "text/javascript",
    "application/javascript",
];

const DECODABLE_ENCODINGS: [&[u8]; 3] = [b"identity", b"gzip", b"br"];

pub type ResponseCompression = CompressionLayer<CompressiblePredicate>;

#[derive(Debug, Clone, Copy)]
pub struct CompressiblePredicate(SizeAbove);

impl Predicate for CompressiblePredicate {
    fn should_compress<B>(&self, response: &http::Response<B>) -> bool
    where
        B: HttpBody,
    {
        self.0.should_compress(response) && has_compressible_type(response.headers())
    }
}

fn has_compressible_type(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .is_some_and(|essence| {
            COMPRESSIBLE_TYPES
                .iter()
                .any(|compressible| essence.eq_ignore_ascii_case(compressible))
        })
}

pub fn response_compression() -> ResponseCompression {
    CompressionLayer::new().compress_when(CompressiblePredicate(SizeAbove::default()))
}

pub fn request_decompression() -> RequestDecompressionLayer {
    RequestDecompressionLayer::new()
}

pub async fn reject_undecodable_body(request: Request, next: Next) -> Response {
    if is_decodable(request.headers()) {
        return next.run(request).await;
    }
    let error = ApiError::new(ErrorCode::UnsupportedMediaType)
        .with_message("The request content encoding is not supported");
    tag_error(error, RequestId::of(&request).as_ref()).into_response()
}

fn is_decodable(headers: &HeaderMap) -> bool {
    let mut encodings = headers.get_all(CONTENT_ENCODING).iter();
    match (encodings.next(), encodings.next()) {
        (None, _) => true,
        (Some(encoding), None) => DECODABLE_ENCODINGS.contains(&encoding.as_bytes()),
        (Some(_), Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use http::header::{HeaderMap, HeaderValue, CONTENT_ENCODING, CONTENT_TYPE};
    use rstest::rstest;

    use super::{has_compressible_type, is_decodable};

    #[rstest]
    #[case::json("application/json", true)]
    #[case::json_with_charset("application/json; charset=utf-8", true)]
    #[case::html("text/html; charset=utf-8", true)]
    #[case::css("text/css", true)]
    #[case::javascript("text/javascript", true)]
    #[case::legacy_javascript("application/javascript", true)]
    #[case::case_insensitive("Application/JSON", true)]
    #[case::octet_stream("application/octet-stream", false)]
    #[case::zip("application/zip", false)]
    #[case::image("image/png", false)]
    #[case::video("video/mp4", false)]
    #[case::plain_text("text/plain", false)]
    #[case::json_suffix_is_not_json("application/jsonx", false)]
    fn unit_compressible_content_types(#[case] content_type: &str, #[case] expected: bool) {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_str(content_type).unwrap());
        assert_eq!(has_compressible_type(&headers), expected);
    }

    #[test]
    fn unit_missing_content_type_is_not_compressible() {
        assert!(!has_compressible_type(&HeaderMap::new()));
    }

    #[rstest]
    #[case::absent(&[], true)]
    #[case::identity(&["identity"], true)]
    #[case::gzip(&["gzip"], true)]
    #[case::brotli(&["br"], true)]
    #[case::deflate(&["deflate"], false)]
    #[case::zstd(&["zstd"], false)]
    #[case::stacked_list(&["gzip, br"], false)]
    #[case::repeated_header(&["gzip", "gzip"], false)]
    #[case::uppercase(&["GZIP"], false)]
    fn unit_decodable_request_encodings(#[case] encodings: &[&str], #[case] expected: bool) {
        let mut headers = HeaderMap::new();
        for encoding in encodings {
            headers.append(CONTENT_ENCODING, HeaderValue::from_str(encoding).unwrap());
        }
        assert_eq!(is_decodable(&headers), expected);
    }
}
