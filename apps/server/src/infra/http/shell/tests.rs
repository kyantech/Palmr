use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use rstest::rstest;

use super::{
    head_environment, ShellContext, ShellInitError, ShellLayoutDefect, ShellMetadata, ShellPart,
    ShellRenderer, HEAD_MARKER, HEAD_TEMPLATE_NAME, HEAD_TEMPLATE_SOURCE,
};
use crate::config::{EnvironmentSource, OperatorConfig, PublicBaseUrl};

pub(crate) const VITE_INDEX: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <base href="/" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Palmr</title>
    <!--palmr:head-->
    <script type="module" crossorigin src="./assets/index-C0iiOcF1.js"></script>
    <link rel="modulepreload" crossorigin href="./assets/vendor-Dx1a2B3c.js">
    <link rel="stylesheet" crossorigin href="./assets/index-B5BXDqMa.css">
  </head>
  <body>
    <div id="root"></div>
  </body>
</html>
"#;

const NONCE: &str = "0123456789abcdef0123456789abcdef";

const HOSTILE_FRAGMENTS: &[&str] = &[
    "\"",
    "'",
    "&",
    "<",
    ">",
    "/",
    "=",
    "`",
    "</title>",
    "<title>",
    "<script>alert(1)</script>",
    "\"><meta http-equiv=\"refresh\" content=\"0;url=https://attacker.test\">",
    "'><meta http-equiv='refresh'>",
    "</meta><script>",
    "<base href=\"https://attacker.test/\">",
    "</head><body onload=alert(1)>",
    "<!--",
    "-->",
    "<![CDATA[",
    "]]>",
    "javascript:alert(1)",
    "&amp;",
    "&lt;script&gt;",
    "&#x27;",
    "&#34;",
    "&quot",
    "{{ csp_nonce }}",
    "{% raw %}",
    "{# #}",
    "\u{202E}",
    "\u{202D}",
    "\u{2066}",
    "\u{2067}",
    "\u{2068}",
    "\u{2069}",
    "\u{200E}",
    "\u{200F}",
    "\u{061C}",
    "\u{200B}",
    "\u{FEFF}",
    "\u{2028}",
    "\u{2029}",
    "\u{0}",
    "\u{1B}",
    "\u{7F}",
    "\u{85}",
    "\r\n",
    "\t",
    "\u{FFFD}",
    "\u{10FFFF}",
    "😀",
    "👩‍👩‍👧‍👦",
    "🏳️‍🌈",
    "e\u{301}\u{302}\u{303}",
    "Z\u{36B}\u{33F}\u{344}\u{317}",
    "ﷺ",
    "مرحبا",
    "שלום",
    "日本語",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Node {
    Doctype,
    Comment(String),
    Open {
        name: String,
        attributes: Vec<(String, String)>,
    },
    Close(String),
    Text(String),
}

impl Node {
    fn attribute(&self, wanted: &str) -> Option<&str> {
        match self {
            Self::Open { attributes, .. } => attributes
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.as_str()),
            _ => None,
        }
    }

    fn is_open(&self, wanted: &str) -> bool {
        matches!(self, Self::Open { name, .. } if name == wanted)
    }

    fn shape(&self) -> Option<Self> {
        match self {
            Self::Open { name, attributes } => Some(Self::Open {
                name: name.clone(),
                attributes: attributes
                    .iter()
                    .map(|(name, _)| (name.clone(), String::new()))
                    .collect(),
            }),
            Self::Text(_) => None,
            other => Some(other.clone()),
        }
    }
}

pub(crate) fn scan(html: &str) -> Vec<Node> {
    let mut nodes = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after.find("-->").expect("unterminated comment");
            nodes.push(Node::Comment(after[..end].to_owned()));
            rest = &after[end + 3..];
        } else if rest.starts_with("<!") {
            let end = rest.find('>').expect("unterminated doctype");
            nodes.push(Node::Doctype);
            rest = &rest[end + 1..];
        } else if let Some(after) = rest.strip_prefix("</") {
            let end = after.find('>').expect("unterminated closing tag");
            nodes.push(Node::Close(after[..end].trim().to_ascii_lowercase()));
            rest = &after[end + 1..];
        } else if let Some(after) = rest.strip_prefix('<') {
            let (node, remaining) = open_tag(after);
            nodes.push(node);
            rest = remaining;
        } else {
            let end = rest.find('<').unwrap_or(rest.len());
            nodes.push(Node::Text(decode(&rest[..end])));
            rest = &rest[end..];
        }
    }
    nodes
}

fn open_tag(input: &str) -> (Node, &str) {
    let is_delimiter = |c: char| c.is_ascii_whitespace() || c == '>' || c == '/';
    let name_end = input.find(is_delimiter).expect("unterminated tag");
    let name = input[..name_end].to_ascii_lowercase();
    let mut rest = &input[name_end..];
    let mut attributes = Vec::new();
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '/');
        if let Some(after) = rest.strip_prefix('>') {
            return (Node::Open { name, attributes }, after);
        }
        let attribute_end = rest
            .find(|c: char| is_delimiter(c) || c == '=')
            .expect("unterminated attribute");
        let attribute = rest[..attribute_end].to_ascii_lowercase();
        rest = &rest[attribute_end..];
        let value = match rest.strip_prefix('=') {
            Some(after) => {
                let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''));
                let (raw, remaining) = match quote {
                    Some(quote) => {
                        let body = &after[1..];
                        let end = body.find(quote).expect("unterminated attribute value");
                        (&body[..end], &body[end + 1..])
                    }
                    None => {
                        let end = after
                            .find(|c: char| c.is_ascii_whitespace() || c == '>')
                            .expect("unterminated attribute value");
                        (&after[..end], &after[end..])
                    }
                };
                rest = remaining;
                decode(raw)
            }
            None => String::new(),
        };
        attributes.push((attribute, value));
    }
}

const REFERENCES: [(&str, char); 7] = [
    ("&amp;", '&'),
    ("&lt;", '<'),
    ("&gt;", '>'),
    ("&quot;", '"'),
    ("&#x27;", '\''),
    ("&#39;", '\''),
    ("&#x2f;", '/'),
];

pub(crate) fn decode(text: &str) -> String {
    let mut decoded = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(ampersand) = rest.find('&') {
        decoded.push_str(&rest[..ampersand]);
        let reference = &rest[ampersand..];
        let (entity, character) = REFERENCES
            .iter()
            .find(|(entity, _)| reference.starts_with(entity))
            .unwrap_or_else(|| panic!("ambiguous character reference in {text:?}"));
        decoded.push(*character);
        rest = &reference[entity.len()..];
    }
    decoded.push_str(rest);
    decoded
}

pub(crate) fn meta(nodes: &[Node], key: &str, value: &str) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.is_open("meta") && node.attribute(key) == Some(value))
        .filter_map(|node| node.attribute("content").map(str::to_owned))
        .collect()
}

pub(crate) fn base_hrefs(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.is_open("base"))
        .filter_map(|node| node.attribute("href").map(str::to_owned))
        .collect()
}

pub(crate) fn titles(nodes: &[Node]) -> Vec<String> {
    nodes
        .windows(3)
        .filter(|window| window[0].is_open("title"))
        .map(|window| match (&window[1], &window[2]) {
            (Node::Text(text), Node::Close(close)) if close == "title" => text.clone(),
            (Node::Close(close), _) if close == "title" => String::new(),
            other => panic!("title does not contain plain text: {other:?}"),
        })
        .collect()
}

fn base_url(value: &str) -> PublicBaseUrl {
    OperatorConfig::load(&EnvironmentSource::from_vars([("PALMR_BASE_URL", value)]))
        .unwrap()
        .config
        .base_url
}

fn renderer() -> ShellRenderer {
    ShellRenderer::new(
        VITE_INDEX.as_bytes(),
        &base_url("https://files.example.test/"),
    )
    .unwrap()
}

struct Hostile<'a> {
    app_name: &'a str,
    app_description: &'a str,
    base_href: &'a str,
    csp_nonce: &'a str,
}

fn render(renderer: &ShellRenderer, values: &Hostile<'_>) -> String {
    renderer
        .render_head(&ShellContext {
            app_name: values.app_name,
            app_description: values.app_description,
            base_href: values.base_href,
            csp_nonce: values.csp_nonce,
        })
        .unwrap()
}

fn assert_inert(renderer: &ShellRenderer, reference: &[Node], values: &Hostile<'_>) {
    let head = render(renderer, values);
    let nodes = scan(&head);

    let shape: Vec<Node> = nodes.iter().filter_map(Node::shape).collect();
    let expected: Vec<Node> = reference.iter().filter_map(Node::shape).collect();
    assert_eq!(shape, expected, "markup structure changed: {head:?}");

    let reference_head = render(
        renderer,
        &Hostile {
            app_name: "x",
            app_description: "x",
            base_href: "x",
            csp_nonce: "x",
        },
    );
    for delimiter in ['<', '>', '"', '\''] {
        assert_eq!(
            head.matches(delimiter).count(),
            reference_head.matches(delimiter).count(),
            "raw {delimiter:?} leaked into {head:?}"
        );
    }

    assert_eq!(titles(&nodes), [values.app_name]);
    assert_eq!(base_hrefs(&nodes), [values.base_href]);
    assert_eq!(meta(&nodes, "name", "csp-nonce"), [values.csp_nonce]);
    assert_eq!(
        meta(&nodes, "name", "description"),
        [values.app_description]
    );
    assert_eq!(meta(&nodes, "property", "og:title"), [values.app_name]);
    assert_eq!(meta(&nodes, "property", "og:site_name"), [values.app_name]);
    assert_eq!(
        meta(&nodes, "property", "og:description"),
        [values.app_description]
    );
    assert!(
        !nodes
            .iter()
            .any(|node| node.is_open("script") || node.is_open("style")),
        "{head:?}"
    );
}

fn hostile_text() -> impl Strategy<Value = String> {
    let fragment = prop::sample::select(HOSTILE_FRAGMENTS).prop_map(str::to_owned);
    let piece = prop_oneof![
        fragment.clone(),
        any::<char>().prop_map(String::from),
        prop::char::range('\u{0}', '\u{7F}').prop_map(String::from),
    ];
    prop_oneof![
        any::<String>(),
        prop::collection::vec(fragment, 0..16).prop_map(|parts| parts.concat()),
        prop::collection::vec(piece, 0..96).prop_map(|parts| parts.concat()),
    ]
}

#[test]
fn prop_shell_render_escapes_hostile_values() {
    let renderer = renderer();
    let reference = scan(&render(
        &renderer,
        &Hostile {
            app_name: "Palmr",
            app_description: "Self-hosted file transfer",
            base_href: "/",
            csp_nonce: NONCE,
        },
    ));

    let long = HOSTILE_FRAGMENTS.concat().repeat(64);
    let lossy_surrogate = String::from_utf8_lossy(b"name-\xED\xA0\x80-\xED\xBF\xBF").into_owned();
    let mut fixed: Vec<String> = HOSTILE_FRAGMENTS.iter().map(|s| (*s).to_owned()).collect();
    fixed.extend([
        String::new(),
        HOSTILE_FRAGMENTS.concat(),
        long,
        lossy_surrogate.clone(),
    ]);
    for value in &fixed {
        assert_inert(
            &renderer,
            &reference,
            &Hostile {
                app_name: value,
                app_description: value,
                base_href: value,
                csp_nonce: value,
            },
        );
    }

    let encoded_surrogate = vec![0xED, 0xA0, 0x80];
    assert!(String::from_utf8(encoded_surrogate).is_err());
    assert!(serde_json::from_str::<String>(r#""\ud800""#).is_err());
    assert!(serde_json::from_str::<String>(r#""\udfff tail""#).is_err());
    assert!(char::from_u32(0xD800).is_none());
    assert_eq!(lossy_surrogate.matches('\u{FFFD}').count(), 6);

    let mut runner = TestRunner::new(Config::with_cases(512));
    runner
        .run(
            &(
                hostile_text(),
                hostile_text(),
                hostile_text(),
                hostile_text(),
            ),
            |(app_name, app_description, base_href, csp_nonce)| {
                assert_inert(
                    &renderer,
                    &reference,
                    &Hostile {
                        app_name: &app_name,
                        app_description: &app_description,
                        base_href: &base_href,
                        csp_nonce: &csp_nonce,
                    },
                );
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn unit_shell_splits_the_built_index_once_around_the_head_placeholder() {
    let renderer = renderer();
    assert!(renderer.prefix.ends_with("<head>\n    <meta charset=\"UTF-8\" />\n    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" />\n    "));
    assert!(renderer.suffix.starts_with(
        "\n    <script type=\"module\" crossorigin src=\"./assets/index-C0iiOcF1.js\">"
    ));

    let head = render(
        &renderer,
        &Hostile {
            app_name: "Palmr",
            app_description: "Self-hosted file transfer",
            base_href: "/",
            csp_nonce: NONCE,
        },
    );
    let document = format!("{}{head}{}", renderer.prefix, renderer.suffix);
    assert!(!document.contains(HEAD_MARKER));
    assert!(!document.contains("<base href=\"/\" />"));
    let nodes = scan(&document);
    assert_eq!(base_hrefs(&nodes), ["/"]);
    assert_eq!(titles(&nodes), ["Palmr"]);

    let first_url = nodes
        .iter()
        .position(|node| node.attribute("src").is_some() || node.attribute("href").is_some())
        .unwrap();
    assert!(nodes[first_url].is_open("base"));

    for preserved in [
        "<meta charset=\"UTF-8\" />",
        "<script type=\"module\" crossorigin src=\"./assets/index-C0iiOcF1.js\"></script>",
        "<link rel=\"modulepreload\" crossorigin href=\"./assets/vendor-Dx1a2B3c.js\">",
        "<link rel=\"stylesheet\" crossorigin href=\"./assets/index-B5BXDqMa.css\">",
        "<div id=\"root\"></div>",
        "</body>\n</html>\n",
    ] {
        assert_eq!(document.matches(preserved).count(), 1, "{preserved}");
    }
}

#[test]
fn unit_frontend_index_source_satisfies_the_shell_layout() {
    let source = include_str!("../../../../../web/index.html");
    let renderer =
        ShellRenderer::new(source.as_bytes(), &base_url("http://localhost:5487")).unwrap();
    assert!(renderer.suffix.contains("<div id=\"root\"></div>"));
}

#[rstest]
#[case::no_marker(VITE_INDEX.replace(HEAD_MARKER, ""), ShellLayoutDefect::Missing(ShellPart::HeadMarker))]
#[case::duplicate_marker(
    VITE_INDEX.replace(HEAD_MARKER, "<!--palmr:head--><!--palmr:head-->"),
    ShellLayoutDefect::Duplicated(ShellPart::HeadMarker)
)]
#[case::marker_in_body(
    VITE_INDEX.replace(HEAD_MARKER, "").replace("<div id=\"root\">", "<!--palmr:head--><div id=\"root\">"),
    ShellLayoutDefect::OutsideHead(ShellPart::HeadMarker)
)]
#[case::no_head(VITE_INDEX.replace("<head>", "<head data-x>"), ShellLayoutDefect::Missing(ShellPart::Head))]
#[case::no_base(VITE_INDEX.replace("<base href=\"/\" />", ""), ShellLayoutDefect::Missing(ShellPart::Base))]
#[case::duplicate_base(
    VITE_INDEX.replace("<base href=\"/\" />", "<base href=\"/\" /><BASE href=\"/\">"),
    ShellLayoutDefect::Duplicated(ShellPart::Base)
)]
#[case::base_after_marker(
    VITE_INDEX.replace("<base href=\"/\" />", "").replace("</head>", "<base href=\"/\"></head>"),
    ShellLayoutDefect::OutsideHead(ShellPart::Base)
)]
#[case::no_title(VITE_INDEX.replace("<title>Palmr</title>", ""), ShellLayoutDefect::Missing(ShellPart::Title))]
#[case::unterminated_title(
    VITE_INDEX.replace("<title>Palmr</title>", "<title>Palmr"),
    ShellLayoutDefect::Unterminated(ShellPart::Title)
)]
#[case::url_before_marker(
    VITE_INDEX.replace("<title>", "<link rel=\"icon\" href=\"./favicon.ico\"><title>"),
    ShellLayoutDefect::UrlBeforeHeadMarker
)]
fn unit_malformed_shell_layout_fails_initialization(
    #[case] index: String,
    #[case] defect: ShellLayoutDefect,
) {
    match ShellRenderer::new(index.as_bytes(), &base_url("https://files.example.test")) {
        Err(ShellInitError::Layout(found)) => assert_eq!(found, defect),
        Err(other) => panic!("unexpected error {other}"),
        Ok(_) => panic!("a malformed shell initialized"),
    }
}

#[test]
fn unit_non_utf8_index_fails_initialization() {
    let mut index = VITE_INDEX.as_bytes().to_vec();
    index.extend_from_slice(b"\xff\xfe");
    assert!(matches!(
        ShellRenderer::new(&index, &base_url("https://files.example.test")),
        Err(ShellInitError::IndexNotUtf8)
    ));
}

#[rstest]
#[case::origin("https://example.test", "/")]
#[case::origin_slash("https://example.test/", "/")]
#[case::subpath("https://example.test/palmr", "/palmr/")]
#[case::subpath_slash("https://example.test/palmr/", "/palmr/")]
#[case::nested("https://example.test/apps/palmr", "/apps/palmr/")]
#[case::port("http://10.0.0.5:8080/palmr/", "/palmr/")]
#[case::encoded("https://example.test/my%20files", "/my%20files/")]
#[case::defaulted("", "/")]
fn unit_base_href_is_the_base_url_path(#[case] configured: &str, #[case] expected: &str) {
    let vars: Vec<(&str, &str)> = if configured.is_empty() {
        Vec::new()
    } else {
        vec![("PALMR_BASE_URL", configured)]
    };
    let base_url = OperatorConfig::load(&EnvironmentSource::from_vars(vars))
        .unwrap()
        .config
        .base_url;
    let renderer = ShellRenderer::new(VITE_INDEX.as_bytes(), &base_url).unwrap();
    assert_eq!(renderer.base_href, expected);
}

#[test]
fn unit_authority_relative_base_path_fails_initialization() {
    assert!(matches!(
        ShellRenderer::new(
            VITE_INDEX.as_bytes(),
            &base_url("https://example.test//attacker.test")
        ),
        Err(ShellInitError::AuthorityRelativeBasePath)
    ));
}

#[test]
fn unit_shell_template_has_no_raw_escape_hatch() {
    for forbidden in [
        "|",
        "safe",
        "autoescape",
        "raw",
        "markup",
        "include",
        "import",
    ] {
        assert!(
            !HEAD_TEMPLATE_SOURCE
                .to_ascii_lowercase()
                .contains(forbidden),
            "{forbidden}"
        );
    }

    for template in [
        "{{ value|safe }}",
        "{{ value|escape }}",
        "{{ value|e }}",
        "{{ value|string }}",
    ] {
        let environment = head_environment(template).unwrap();
        let rendered = environment
            .get_template(HEAD_TEMPLATE_NAME)
            .unwrap()
            .render(minijinja::context! { value => "<script>" });
        assert!(rendered.is_err(), "{template} rendered: {rendered:?}");
    }

    let environment = head_environment("{{ value }}").unwrap();
    assert_eq!(
        environment
            .get_template(HEAD_TEMPLATE_NAME)
            .unwrap()
            .render(minijinja::context! { value => "<script>" })
            .unwrap(),
        "&lt;script&gt;"
    );
}

#[test]
fn unit_invalid_head_template_fails_initialization() {
    assert!(matches!(
        head_environment("{{ app_name"),
        Err(ShellInitError::Template(_))
    ));
    let environment = head_environment("{{ unknown_value }}").unwrap();
    assert!(environment
        .get_template(HEAD_TEMPLATE_NAME)
        .unwrap()
        .render(minijinja::context! {})
        .is_err());
}

#[test]
fn unit_fresh_install_metadata_defaults() {
    assert_eq!(
        ShellMetadata::fresh_install(),
        ShellMetadata::new("Palmr", "Self-hosted file transfer")
    );
}
