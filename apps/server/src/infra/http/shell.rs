use std::error::Error;
use std::fmt;
use std::io;
use std::ops::Range;

use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;

use super::headers::CspNonce;
use crate::config::PublicBaseUrl;

const HEAD_TEMPLATE_NAME: &str = "shell/head.html.j2";
const HEAD_TEMPLATE_SOURCE: &str = include_str!("../../../templates/shell/head.html.j2");
const HEAD_MARKER: &str = "<!--palmr:head-->";
const HEAD_OPEN: &str = "<head>";
const HEAD_CLOSE: &str = "</head>";
const URL_ATTRIBUTES: [&str; 3] = ["href", "src", "srcset"];

pub(crate) const FRESH_INSTALL_APP_NAME: &str = "Palmr";
const FRESH_INSTALL_APP_DESCRIPTION: &str = "Self-hosted file transfer";
const PROBE_NONCE: &str = "00000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellMetadata {
    app_name: String,
    app_description: String,
}

impl ShellMetadata {
    pub fn new(app_name: impl Into<String>, app_description: impl Into<String>) -> Self {
        Self {
            app_name: app_name.into(),
            app_description: app_description.into(),
        }
    }

    pub fn fresh_install() -> Self {
        Self::new(FRESH_INSTALL_APP_NAME, FRESH_INSTALL_APP_DESCRIPTION)
    }
}

#[derive(Serialize)]
struct ShellContext<'a> {
    app_name: &'a str,
    app_description: &'a str,
    base_href: &'a str,
    csp_nonce: &'a str,
}

pub struct ShellRenderer {
    environment: Environment<'static>,
    prefix: String,
    suffix: String,
    base_href: String,
}

impl ShellRenderer {
    pub fn new(index_html: &[u8], base_url: &PublicBaseUrl) -> Result<Self, ShellInitError> {
        let index = std::str::from_utf8(index_html).map_err(|_| ShellInitError::IndexNotUtf8)?;
        let layout = ShellLayout::split(index).map_err(ShellInitError::Layout)?;
        let renderer = Self {
            environment: head_environment(HEAD_TEMPLATE_SOURCE)?,
            prefix: layout.prefix,
            suffix: layout.suffix,
            base_href: base_href(base_url)?,
        };
        let probe = ShellMetadata::fresh_install();
        renderer
            .render_head(&renderer.context(&probe, PROBE_NONCE))
            .map_err(ShellInitError::Template)?;
        Ok(renderer)
    }

    pub fn render_default(
        &self,
        metadata: &ShellMetadata,
        nonce: &CspNonce,
    ) -> Result<String, ShellRenderError> {
        let head = self
            .render_head(&self.context(metadata, nonce.as_str()))
            .map_err(ShellRenderError)?;
        let mut html = String::with_capacity(self.prefix.len() + head.len() + self.suffix.len());
        html.push_str(&self.prefix);
        html.push_str(&head);
        html.push_str(&self.suffix);
        Ok(html)
    }

    fn context<'a>(&'a self, metadata: &'a ShellMetadata, nonce: &'a str) -> ShellContext<'a> {
        ShellContext {
            app_name: &metadata.app_name,
            app_description: &metadata.app_description,
            base_href: &self.base_href,
            csp_nonce: nonce,
        }
    }

    fn render_head(&self, context: &ShellContext<'_>) -> Result<String, minijinja::Error> {
        self.environment
            .get_template(HEAD_TEMPLATE_NAME)?
            .render(context)
    }
}

// `Environment::empty` registers no filters, so `safe`, `escape` and every
// other built-in are unreachable from the template: an autoescaped variable is
// the only way a value can reach the markup.
fn head_environment(source: &str) -> Result<Environment<'_>, ShellInitError> {
    let mut environment = Environment::empty();
    environment.set_auto_escape_callback(|_| AutoEscape::Html);
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment
        .add_template(HEAD_TEMPLATE_NAME, source)
        .map_err(ShellInitError::Template)?;
    Ok(environment)
}

fn base_href(base_url: &PublicBaseUrl) -> Result<String, ShellInitError> {
    let path = base_url.url().path();
    if !path.starts_with('/') || path.starts_with("//") {
        return Err(ShellInitError::AuthorityRelativeBasePath);
    }
    let mut href = path.to_owned();
    if !href.ends_with('/') {
        href.push('/');
    }
    Ok(href)
}

struct ShellLayout {
    prefix: String,
    suffix: String,
}

impl ShellLayout {
    // Offsets found in the ASCII-folded copy are valid in the original because
    // ASCII folding never changes a byte's length or position.
    fn split(index: &str) -> Result<Self, ShellLayoutDefect> {
        let folded = index.to_ascii_lowercase();
        let head_start = only(&folded, HEAD_OPEN, ShellPart::Head)? + HEAD_OPEN.len();
        let head_end = only(&folded, HEAD_CLOSE, ShellPart::Head)?;
        let marker = only(&folded, HEAD_MARKER, ShellPart::HeadMarker)?;
        let before_marker = head_start..marker;
        if !(head_start <= marker && marker + HEAD_MARKER.len() <= head_end) {
            return Err(ShellLayoutDefect::OutsideHead(ShellPart::HeadMarker));
        }

        let base = only_element(&folded, "base", None, ShellPart::Base)?;
        let title = only_element(&folded, "title", Some("</title"), ShellPart::Title)?;
        let mut removed = [base, title];
        for (range, part) in removed.iter().zip([ShellPart::Base, ShellPart::Title]) {
            if !(before_marker.contains(&range.start) && range.end <= marker) {
                return Err(ShellLayoutDefect::OutsideHead(part));
            }
        }
        removed.sort_by_key(|range| range.start);

        let mut prefix = String::with_capacity(marker);
        let mut cursor = 0;
        for range in removed {
            let range = whole_line(index, range);
            prefix.push_str(&index[cursor..range.start]);
            cursor = range.end;
        }
        prefix.push_str(&index[cursor..marker]);

        // The injected <base> must precede every relative URL in the document,
        // otherwise those URLs resolve against the request path and a sub-path
        // deployment loses its prefix.
        if references_url(&prefix.to_ascii_lowercase()) {
            return Err(ShellLayoutDefect::UrlBeforeHeadMarker);
        }
        Ok(Self {
            prefix,
            suffix: index[marker + HEAD_MARKER.len()..].to_owned(),
        })
    }
}

fn only(folded: &str, needle: &str, part: ShellPart) -> Result<usize, ShellLayoutDefect> {
    let mut found = folded.match_indices(needle).map(|(start, _)| start);
    match (found.next(), found.next()) {
        (Some(start), None) => Ok(start),
        (None, _) => Err(ShellLayoutDefect::Missing(part)),
        (Some(_), Some(_)) => Err(ShellLayoutDefect::Duplicated(part)),
    }
}

fn only_element(
    folded: &str,
    name: &str,
    closing: Option<&str>,
    part: ShellPart,
) -> Result<Range<usize>, ShellLayoutDefect> {
    let mut found = tag_starts(folded, name);
    let start = match (found.next(), found.next()) {
        (Some(start), None) => start,
        (None, _) => return Err(ShellLayoutDefect::Missing(part)),
        (Some(_), Some(_)) => return Err(ShellLayoutDefect::Duplicated(part)),
    };
    let rest = &folded[start..];
    let close_from = match closing {
        Some(closing) => rest
            .find(closing)
            .ok_or(ShellLayoutDefect::Unterminated(part))?,
        None => 0,
    };
    let end = rest[close_from..]
        .find('>')
        .ok_or(ShellLayoutDefect::Unterminated(part))?;
    Ok(start..start + close_from + end + 1)
}

fn tag_starts<'a>(folded: &'a str, name: &'a str) -> impl Iterator<Item = usize> + 'a {
    folded.match_indices('<').filter_map(move |(start, _)| {
        let after = folded[start + 1..].strip_prefix(name)?;
        matches!(
            after.bytes().next(),
            Some(b'>' | b'/') | Some(b' ' | b'\t' | b'\n' | b'\r')
        )
        .then_some(start)
    })
}

fn whole_line(text: &str, element: Range<usize>) -> Range<usize> {
    let bytes = text.as_bytes();
    let indent = bytes[..element.start]
        .iter()
        .rev()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .count();
    let trailing = bytes[element.end..]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
        .count();
    let start = element.start - indent;
    let end = element.end + trailing;
    let starts_line = start == 0 || bytes.get(start - 1) == Some(&b'\n');
    if starts_line && bytes.get(end) == Some(&b'\n') {
        start..end + 1
    } else {
        element
    }
}

fn references_url(folded: &str) -> bool {
    URL_ATTRIBUTES.iter().any(|attribute| {
        folded.match_indices(attribute).any(|(start, _)| {
            let preceded_by_space = folded[..start]
                .bytes()
                .next_back()
                .is_some_and(|byte| byte.is_ascii_whitespace());
            let followed_by_equals = folded[start + attribute.len()..]
                .trim_start()
                .starts_with('=');
            preceded_by_space && followed_by_equals
        })
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPart {
    Head,
    HeadMarker,
    Base,
    Title,
}

impl fmt::Display for ShellPart {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Head => "the <head> element",
            Self::HeadMarker => "the <!--palmr:head--> placeholder",
            Self::Base => "the <base> placeholder",
            Self::Title => "the <title> element",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellLayoutDefect {
    Missing(ShellPart),
    Duplicated(ShellPart),
    Unterminated(ShellPart),
    OutsideHead(ShellPart),
    UrlBeforeHeadMarker,
}

impl fmt::Display for ShellLayoutDefect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(part) => write!(f, "{part} is missing"),
            Self::Duplicated(part) => write!(f, "{part} appears more than once"),
            Self::Unterminated(part) => write!(f, "{part} is not terminated"),
            Self::OutsideHead(part) => {
                write!(f, "{part} is not inside <head> before the head placeholder")
            }
            Self::UrlBeforeHeadMarker => f.write_str(
                "an element before the head placeholder references a URL the injected <base> would not govern",
            ),
        }
    }
}

#[derive(Debug)]
pub enum ShellInitError {
    MissingIndex,
    UnreadableIndex(io::Error),
    IndexNotUtf8,
    Layout(ShellLayoutDefect),
    AuthorityRelativeBasePath,
    Template(minijinja::Error),
}

impl fmt::Display for ShellInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIndex => f.write_str("the built SPA has no index.html"),
            Self::UnreadableIndex(error) => {
                write!(f, "the built SPA index.html could not be read: {error}")
            }
            Self::IndexNotUtf8 => f.write_str("the built SPA index.html is not valid UTF-8"),
            Self::Layout(defect) => {
                write!(f, "the built SPA index.html cannot be used as the shell: {defect}")
            }
            Self::AuthorityRelativeBasePath => f.write_str(
                "the PALMR_BASE_URL path starts with '//', which a browser would resolve as a different host",
            ),
            Self::Template(error) => write!(f, "the shell head template is invalid: {error}"),
        }
    }
}

impl Error for ShellInitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::UnreadableIndex(error) => Some(error),
            Self::Template(error) => Some(error),
            Self::MissingIndex
            | Self::IndexNotUtf8
            | Self::Layout(_)
            | Self::AuthorityRelativeBasePath => None,
        }
    }
}

#[derive(Debug)]
pub struct ShellRenderError(minijinja::Error);

impl fmt::Display for ShellRenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the SPA shell could not be rendered: {}", self.0)
    }
}

impl Error for ShellRenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.0)
    }
}

#[cfg(test)]
pub(crate) mod tests;
