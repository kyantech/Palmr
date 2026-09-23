use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use http::uri::{PathAndQuery, Uri};

pub async fn normalize_path(mut request: Request, next: Next) -> Response {
    if let Some(uri) = normalized_uri(request.uri()) {
        *request.uri_mut() = uri;
    }
    next.run(request).await
}

fn normalized_uri(uri: &Uri) -> Option<Uri> {
    let path = normalized_path(uri.path())?;
    let path_and_query = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(PathAndQuery::try_from(path_and_query).ok()?);
    Uri::from_parts(parts).ok()
}

fn normalized_path(path: &str) -> Option<String> {
    if !path.starts_with('/') {
        return None;
    }
    let mut normalized = String::with_capacity(path.len());
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        normalized.push('/');
        normalized.push_str(segment);
    }
    if normalized.is_empty() {
        normalized.push('/');
    }
    (normalized != path).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use http::Uri;
    use rstest::rstest;

    use super::{normalized_path, normalized_uri};

    #[rstest]
    #[case::root("/", None)]
    #[case::already_normal("/api/v1/files", None)]
    #[case::trailing_slash("/api/v1/files/", Some("/api/v1/files"))]
    #[case::leading_duplicate("//api/v1/files", Some("/api/v1/files"))]
    #[case::leading_and_trailing("//api/v1/files/", Some("/api/v1/files"))]
    #[case::interior_duplicate("/api//v1///files", Some("/api/v1/files"))]
    #[case::only_slashes("///", Some("/"))]
    #[case::case_preserved("/API/V1/Files/", Some("/API/V1/Files"))]
    #[case::encoded_slash_untouched("/s/a%2Fb/", Some("/s/a%2Fb"))]
    #[case::encoded_bytes_untouched("/s/%41%2f%2F", None)]
    #[case::dot_segments_untouched("/a/./b/../c", None)]
    #[case::asterisk_form("*", None)]
    fn unit_normalized_path(#[case] input: &str, #[case] expected: Option<&str>) {
        assert_eq!(normalized_path(input).as_deref(), expected);
    }

    #[rstest]
    #[case::query_preserved("/api//v1/files/?b=2&a=1", "/api/v1/files?b=2&a=1")]
    #[case::query_slashes_untouched("/api/v1/?next=//x//", "/api/v1?next=//x//")]
    #[case::empty_query_kept("/api/v1/?", "/api/v1?")]
    #[case::absolute_form("http://palmr.test//api/v1/", "http://palmr.test/api/v1")]
    fn unit_normalized_uri_keeps_query(#[case] input: &str, #[case] expected: &str) {
        let uri: Uri = input.parse().unwrap();
        assert_eq!(normalized_uri(&uri).unwrap().to_string(), expected);
    }
}
