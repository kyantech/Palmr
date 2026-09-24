use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use minijinja::{AutoEscape, Environment, UndefinedBehavior};
use serde::Serialize;

use super::model::{DisplayText, MailKind, MailParams, MAX_SUBJECT_CHARS};
use crate::domain::locale::LocaleCode;

const FALLBACK_SUBJECT: &str = "Palmr";

macro_rules! locale_sources {
    ($($code:literal),+ $(,)?) => {
        pub const LOCALE_SOURCES: &[(&str, &str)] = &[
            $((
                $code,
                include_str!(concat!(
                    "../../../../web/src/app/i18n/locales/",
                    $code,
                    "/emails.json"
                )),
            )),+
        ];
    };
}

locale_sources! {
    "ar-SA", "de-DE", "el-GR", "en-US", "es-ES", "fa-IR", "fr-FR", "he-IL", "hi-IN",
    "id-ID", "it-IT", "ja-JP", "ko-KR", "nl-NL", "pl-PL", "pt-BR", "ru-RU", "sv-SE",
    "th-TH", "tr-TR", "uk-UA", "vi-VN", "zh-CN",
}

const HTML_TEMPLATES: &[(&str, &str)] = &[
    (
        "layout.html.j2",
        include_str!("../../../templates/email/layout.html.j2"),
    ),
    (
        "password_reset.html.j2",
        include_str!("../../../templates/email/password_reset.html.j2"),
    ),
    (
        "email_verification.html.j2",
        include_str!("../../../templates/email/email_verification.html.j2"),
    ),
    (
        "invite.html.j2",
        include_str!("../../../templates/email/invite.html.j2"),
    ),
    (
        "share_recipient_notify.html.j2",
        include_str!("../../../templates/email/share_recipient_notify.html.j2"),
    ),
    (
        "reverse_share_owner_notify.html.j2",
        include_str!("../../../templates/email/reverse_share_owner_notify.html.j2"),
    ),
    (
        "security_notification.html.j2",
        include_str!("../../../templates/email/security_notification.html.j2"),
    ),
];

const TEXT_TEMPLATES: &[(&str, &str)] = &[
    (
        "password_reset.txt.j2",
        include_str!("../../../templates/email/password_reset.txt.j2"),
    ),
    (
        "email_verification.txt.j2",
        include_str!("../../../templates/email/email_verification.txt.j2"),
    ),
    (
        "invite.txt.j2",
        include_str!("../../../templates/email/invite.txt.j2"),
    ),
    (
        "share_recipient_notify.txt.j2",
        include_str!("../../../templates/email/share_recipient_notify.txt.j2"),
    ),
    (
        "reverse_share_owner_notify.txt.j2",
        include_str!("../../../templates/email/reverse_share_owner_notify.txt.j2"),
    ),
    (
        "security_notification.txt.j2",
        include_str!("../../../templates/email/security_notification.txt.j2"),
    ),
];

#[derive(Clone, Copy)]
pub struct RenderError;

impl RenderError {
    pub const fn code(self) -> &'static str {
        super::error::EMAIL_TEMPLATE_FAILED
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the e-mail template could not be rendered")
    }
}

impl fmt::Debug for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RenderError")
    }
}

impl std::error::Error for RenderError {}

impl From<minijinja::Error> for RenderError {
    fn from(_: minijinja::Error) -> Self {
        Self
    }
}

impl From<serde_json::Error> for RenderError {
    fn from(_: serde_json::Error) -> Self {
        Self
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MailRenderer;

struct Inner {
    html: Environment<'static>,
    text: Environment<'static>,
}

static INNER: OnceLock<Result<Inner, RenderError>> = OnceLock::new();

impl MailRenderer {
    pub const fn new() -> Self {
        Self
    }

    fn inner(&self) -> Result<&'static Inner, RenderError> {
        match INNER.get_or_init(build) {
            Ok(inner) => Ok(inner),
            Err(error) => Err(*error),
        }
    }

    pub fn render(&self, request: &RenderRequest<'_>) -> Result<RenderedMail, RenderError> {
        let inner = self.inner()?;
        let context = Context::build(request);
        let kind = request.kind.as_str();
        let html = inner
            .html
            .get_template(&format!("{kind}.html.j2"))?
            .render(&context)?;
        let text = inner
            .text
            .get_template(&format!("{kind}.txt.j2"))?
            .render(&context)?;
        let subject = inner
            .text
            .get_template(&format!("frag.{}.{kind}.subject", request.locale.as_str()))?
            .render(&context)?;
        Ok(RenderedMail {
            subject: sanitize_subject(&subject),
            html,
            text,
        })
    }
}

fn build() -> Result<Inner, RenderError> {
    Ok(Inner {
        html: environment(AutoEscape::Html, HTML_TEMPLATES)?,
        text: environment(AutoEscape::None, TEXT_TEMPLATES)?,
    })
}

pub struct RenderRequest<'a> {
    pub kind: MailKind,
    pub locale: LocaleCode,
    pub recipient_name: Option<&'a str>,
    pub app_name: &'a str,
    pub logo_url: Option<&'a str>,
    pub action_url: &'a str,
    pub params: &'a MailParams,
}

#[derive(Debug, Clone)]
pub struct RenderedMail {
    pub subject: String,
    pub html: String,
    pub text: String,
}

#[derive(Serialize)]
struct Context {
    locale: String,
    text_direction: &'static str,
    app_name: String,
    logo_url: Option<String>,
    recipient_name: String,
    action_url: String,
    expiry_minutes: u32,
    expiry_hours: u32,
    inviter_name: String,
    sender_name: String,
    share_name: String,
    file_names: String,
    file_count: usize,
    uploader_name: String,
    link_name: String,
    event: String,
}

impl Context {
    fn build(request: &RenderRequest<'_>) -> Self {
        let mut context = Self {
            locale: request.locale.as_str().to_owned(),
            text_direction: text_direction(request.locale),
            app_name: DisplayText::new(request.app_name).as_str().to_owned(),
            logo_url: request.logo_url.map(ToOwned::to_owned),
            recipient_name: request
                .recipient_name
                .map(|name| DisplayText::new(name).as_str().to_owned())
                .unwrap_or_default(),
            action_url: request.action_url.to_owned(),
            expiry_minutes: 0,
            expiry_hours: 0,
            inviter_name: String::new(),
            sender_name: String::new(),
            share_name: String::new(),
            file_names: String::new(),
            file_count: 0,
            uploader_name: String::new(),
            link_name: String::new(),
            event: String::new(),
        };
        match request.params {
            MailParams::PasswordReset { expiry_minutes }
            | MailParams::EmailVerification { expiry_minutes } => {
                context.expiry_minutes = *expiry_minutes;
            }
            MailParams::Invite {
                inviter_name,
                expiry_hours,
            } => {
                context.inviter_name = inviter_name.as_str().to_owned();
                context.expiry_hours = *expiry_hours;
            }
            MailParams::ShareRecipientNotify {
                sender_name,
                share_name,
                file_names,
                ..
            } => {
                context.sender_name = sender_name.as_str().to_owned();
                context.share_name = share_name.as_str().to_owned();
                context.file_names = file_names.joined();
                context.file_count = file_names.len();
            }
            MailParams::ReverseShareOwnerNotify {
                uploader_name,
                link_name,
                file_names,
            } => {
                context.uploader_name = uploader_name.as_str().to_owned();
                context.link_name = link_name.as_str().to_owned();
                context.file_names = file_names.joined();
                context.file_count = file_names.len();
            }
            MailParams::SecurityNotification { event } => {
                context.event = event.as_str().to_owned();
            }
        }
        context
    }
}

fn environment(
    auto_escape: AutoEscape,
    templates: &'static [(&'static str, &'static str)],
) -> Result<Environment<'static>, RenderError> {
    let mut environment = Environment::empty();
    environment.set_undefined_behavior(UndefinedBehavior::Strict);
    environment.set_auto_escape_callback(move |_| auto_escape);
    for (name, source) in templates {
        environment.add_template_owned(*name, *source)?;
    }
    for (locale, json) in LOCALE_SOURCES {
        let fragments: BTreeMap<String, String> = serde_json::from_str(json)?;
        for (key, source) in fragments {
            environment.add_template_owned(fragment_name(locale, &key), source)?;
        }
    }
    Ok(environment)
}

fn fragment_name(locale: &str, key: &str) -> String {
    format!("frag.{locale}.{key}")
}

fn text_direction(locale: LocaleCode) -> &'static str {
    match locale {
        LocaleCode::ArSa | LocaleCode::FaIr | LocaleCode::HeIl => "rtl",
        _ => "ltr",
    }
}

pub fn sanitize_subject(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len().min(MAX_SUBJECT_CHARS));
    let mut count = 0;
    for ch in raw.chars() {
        if ch.is_control() {
            continue;
        }
        if count == MAX_SUBJECT_CHARS {
            break;
        }
        cleaned.push(ch);
        count += 1;
    }
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        FALLBACK_SUBJECT.to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use tl::{Node, ParserOptions};

    use super::*;
    use crate::features::email::model::{FileNames, SecurityEvent};

    const BASE: &str = "https://palmr.example";
    const HOSTILE_IMG: &str = "<img src=x onerror=alert(1)>";
    const HOSTILE_TD: &str = "</td><script>alert(2)</script>";
    const HOSTILE_JS: &str = "<a href=\"javascript:alert(3)\">x</a>";
    const HOSTILE_AMP: &str = "& unpaired <";
    const HOSTILE_RTL: &str = "name\u{202e}evil";

    fn params_for(kind: MailKind) -> MailParams {
        let names = FileNames::new([HOSTILE_TD, HOSTILE_IMG, HOSTILE_AMP, HOSTILE_RTL]);
        match kind {
            MailKind::PasswordReset => MailParams::PasswordReset { expiry_minutes: 60 },
            MailKind::EmailVerification => MailParams::EmailVerification { expiry_minutes: 60 },
            MailKind::Invite => MailParams::Invite {
                inviter_name: DisplayText::new(HOSTILE_IMG),
                expiry_hours: 24,
            },
            MailKind::ShareRecipientNotify => MailParams::ShareRecipientNotify {
                sender_name: DisplayText::new(HOSTILE_IMG),
                share_name: DisplayText::new(HOSTILE_TD),
                share_alias: DisplayText::new("abc123"),
                file_names: names,
            },
            MailKind::ReverseShareOwnerNotify => MailParams::ReverseShareOwnerNotify {
                uploader_name: DisplayText::new(HOSTILE_JS),
                link_name: DisplayText::new(HOSTILE_AMP),
                file_names: names,
            },
            MailKind::SecurityNotification => MailParams::SecurityNotification {
                event: SecurityEvent::PasswordChanged,
            },
        }
    }

    fn render_with(
        renderer: &MailRenderer,
        kind: MailKind,
        locale: LocaleCode,
        app_name: &str,
    ) -> RenderedMail {
        let params = params_for(kind);
        let action_url = format!("{BASE}/s/abc123");
        let request = RenderRequest {
            kind,
            locale,
            recipient_name: Some(HOSTILE_TD),
            app_name,
            logo_url: None,
            action_url: &action_url,
            params: &params,
        };
        renderer.render(&request).unwrap()
    }

    fn tags(html: &str) -> Vec<String> {
        let dom = tl::parse(html, ParserOptions::default()).unwrap();
        dom.nodes()
            .iter()
            .filter_map(Node::as_tag)
            .map(|tag| tag.name().as_utf8_str().to_ascii_lowercase())
            .collect()
    }

    fn anchors(html: &str) -> Vec<String> {
        let dom = tl::parse(html, ParserOptions::default()).unwrap();
        dom.nodes()
            .iter()
            .filter_map(Node::as_tag)
            .filter(|tag| tag.name().as_utf8_str().eq_ignore_ascii_case("a"))
            .filter_map(|tag| {
                tag.attributes()
                    .get("href")
                    .flatten()
                    .map(|href| decode_entities(&href.as_utf8_str()))
            })
            .collect()
    }

    fn decode_entities(text: &str) -> String {
        text.replace("&#x2f;", "/")
            .replace("&#47;", "/")
            .replace("&quot;", "\"")
            .replace("&#x27;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }

    #[test]
    #[expect(
        non_snake_case,
        reason = "regression test names keep the upper-case catalogue identifier"
    )]
    fn regression_R046_email_template_html_escaping() {
        let renderer = MailRenderer::new();
        let ten_k = format!("{}<script>alert(9)</script>", "A".repeat(10_000));

        for kind in MailKind::ALL {
            for locale in LocaleCode::ALL {
                let rendered = render_with(&renderer, kind, *locale, HOSTILE_IMG);

                assert!(!rendered.subject.contains('\r'), "{kind} {locale}");
                assert!(!rendered.subject.contains('\n'), "{kind} {locale}");
                assert!(rendered.subject.chars().count() <= MAX_SUBJECT_CHARS);
                assert!(!rendered.subject.is_empty());

                assert!(rendered.text.contains(BASE));
                assert!(!rendered.text.is_empty());

                let parsed = tags(&rendered.html);
                for forbidden in ["script", "img", "iframe", "object", "embed", "form", "td"] {
                    assert!(
                        !parsed.iter().any(|tag| tag == forbidden),
                        "{forbidden} element rendered for {kind} in {locale}"
                    );
                }

                let links = anchors(&rendered.html);
                assert_eq!(links.len(), 1, "{kind} in {locale}");
                assert_eq!(
                    links[0],
                    format!("{BASE}/s/abc123"),
                    "server-derived link expected for {kind} in {locale}"
                );

                assert!(rendered.html.contains("&lt;img"), "{kind} in {locale}");
                assert!(!rendered.html.contains(HOSTILE_IMG), "{kind} in {locale}");

                let bounded = render_with(&renderer, kind, *locale, &ten_k);
                assert!(!bounded.html.contains(&ten_k), "{kind} in {locale}");
                assert!(
                    bounded.html.contains(&"A".repeat(MAX_SUBJECT_CHARS)),
                    "{kind} in {locale}"
                );
                assert!(!bounded.subject.contains('\n'));
                let bounded_tags = tags(&bounded.html);
                assert!(!bounded_tags.iter().any(|tag| tag == "script"));

                let crlf = render_with(
                    &renderer,
                    kind,
                    *locale,
                    "Header\r\nBcc: injected@example.test",
                );
                assert!(!crlf.subject.contains('\r'), "{kind} in {locale}");
                assert!(!crlf.subject.contains('\n'), "{kind} in {locale}");
                assert!(crlf.subject.chars().count() <= MAX_SUBJECT_CHARS);
            }
        }
    }

    #[test]
    fn unit_sanitize_subject_strips_crlf_and_bounds() {
        assert_eq!(
            sanitize_subject("ok\r\nBcc: evil@example.test"),
            "okBcc: evil@example.test"
        );
        assert!(!sanitize_subject("x\r\ny").contains('\n'));
        assert!(!sanitize_subject("x\u{0}y").contains('\u{0}'));
        assert_eq!(sanitize_subject("   "), FALLBACK_SUBJECT);
        assert_eq!(sanitize_subject(""), FALLBACK_SUBJECT);
        let long = "s".repeat(10_000);
        assert_eq!(sanitize_subject(&long).chars().count(), MAX_SUBJECT_CHARS);
    }

    #[test]
    fn unit_email_locale_key_parity() {
        let mut expected: Option<BTreeSet<String>> = None;
        for (locale, json) in LOCALE_SOURCES {
            let fragments: BTreeMap<String, String> = serde_json::from_str(json).unwrap();
            assert!(!fragments.is_empty(), "{locale}");
            assert!(
                fragments.keys().all(|key| !key.is_empty()),
                "{locale} has an empty key"
            );
            let keys: BTreeSet<String> = fragments.keys().cloned().collect();
            match &expected {
                None => expected = Some(keys),
                Some(expected) => assert_eq!(&keys, expected, "{locale} key set differs"),
            }
        }
        assert_eq!(LOCALE_SOURCES.len(), 23);
        assert_eq!(LocaleCode::ALL.len(), 23);
        let expected = expected.unwrap();
        assert!(expected.contains("password_reset.subject"));
        assert!(expected.contains("security_notification.event.password_changed"));
    }

    fn locale_json(locale: &str) -> &'static str {
        LOCALE_SOURCES
            .iter()
            .find(|(code, _)| *code == locale)
            .map(|(_, json)| *json)
            .unwrap()
    }

    fn placeholders(value: &str) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut rest = value;
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                break;
            };
            found.insert(after[..end].trim().to_owned());
            rest = &after[end + 2..];
        }
        found
    }

    fn without_placeholders(value: &str) -> String {
        let mut plain = String::new();
        let mut rest = value;
        while let Some(start) = rest.find("{{") {
            plain.push_str(&rest[..start]);
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else {
                plain.push_str(&rest[start..]);
                return plain;
            };
            rest = &after[end + 2..];
        }
        plain.push_str(rest);
        plain
    }

    #[test]
    fn unit_email_locale_translations_are_localised() {
        let source = locale_json("en-US");
        let base: BTreeMap<String, String> = serde_json::from_str(source).unwrap();
        for (locale, json) in LOCALE_SOURCES {
            let fragments: BTreeMap<String, String> = serde_json::from_str(json).unwrap();
            let mut differing = 0;
            for (key, value) in &fragments {
                let base_value = &base[key];
                assert_eq!(
                    placeholders(value),
                    placeholders(base_value),
                    "{locale}:{key} placeholder set differs"
                );
                let plain = without_placeholders(value);
                for forbidden in ['<', '>', '&', '{', '}'] {
                    assert!(
                        !plain.contains(forbidden),
                        "{locale}:{key} contains raw {forbidden:?}"
                    );
                }
                assert!(
                    !value.contains('\r') && !value.contains('\n'),
                    "{locale}:{key} contains a line break"
                );
                if value != base_value {
                    differing += 1;
                }
            }
            if *locale == "en-US" {
                assert_eq!(differing, 0, "en-US is the canonical source locale");
            } else {
                assert_ne!(*json, source, "{locale} must not reuse the en-US file");
                assert!(
                    differing >= 20,
                    "{locale} only differs from en-US in {differing} of {} keys",
                    base.len()
                );
            }
        }
    }

    #[test]
    fn unit_every_kind_and_locale_renders() {
        let renderer = MailRenderer::new();
        let logo_url = format!("{BASE}/logo.webp");
        let action_url = format!("{BASE}/x");
        for kind in MailKind::ALL {
            for locale in LocaleCode::ALL {
                let params = params_for(kind);
                let request = RenderRequest {
                    kind,
                    locale: *locale,
                    recipient_name: Some("Ada"),
                    app_name: "Palmr",
                    logo_url: Some(&logo_url),
                    action_url: &action_url,
                    params: &params,
                };
                let rendered = renderer.render(&request).unwrap();
                assert!(!rendered.subject.is_empty());
                assert!(rendered.html.contains("<!doctype html>"));
                assert!(rendered.html.contains("logo.webp"));
                assert!(rendered.html.contains("alt=\"Palmr\""));
                assert!(!rendered.text.is_empty());
            }
        }
    }
}
