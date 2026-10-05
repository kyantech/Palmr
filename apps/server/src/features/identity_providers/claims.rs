use serde_json::Value;
use url::Url;

use super::http_client::acceptable_url;
use super::model::Preset;

pub fn email_verified(claim: Option<&Value>) -> bool {
    match claim {
        Some(Value::Bool(verified)) => *verified,
        Some(Value::String(text)) => text == "true",
        _ => false,
    }
}

pub fn github_verified_primary_email(emails: &Value) -> Option<String> {
    emails.as_array()?.iter().find_map(|entry| {
        let primary = entry.get("primary") == Some(&Value::Bool(true));
        let verified = entry.get("verified") == Some(&Value::Bool(true));
        let address = entry
            .get("email")?
            .as_str()
            .filter(|text| !text.is_empty())?;
        (primary && verified).then(|| address.to_owned())
    })
}

pub fn github_emails_endpoint(userinfo_endpoint: &str) -> Option<String> {
    let mut url = Url::parse(userinfo_endpoint).ok()?;
    url.path_segments_mut().ok()?.pop_if_empty().push("emails");
    Some(url.into())
}

pub fn avatar_url(preset: Preset, subject: &str, picture: Option<&str>) -> Option<String> {
    let picture = picture?;
    if let Some(url) = acceptable_url(picture) {
        return (url.scheme() == "https").then(|| picture.to_owned());
    }
    let bare_hash = !picture.is_empty()
        && picture
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    let numeric_subject = !subject.is_empty() && subject.bytes().all(|byte| byte.is_ascii_digit());
    (preset == Preset::Discord && bare_hash && numeric_subject)
        .then(|| format!("https://cdn.discordapp.com/avatars/{subject}/{picture}.png"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::model::{ClaimMapping, Protocol};
    use super::super::oauth2::profile_from_claims;
    use super::super::oidc::{check_claims, ClaimContext, JwsAlgorithm, ValidationPurpose};
    use super::super::presets::{spec_for, CATALOGUE};
    use super::*;

    #[test]
    fn unit_only_boolean_true_and_the_string_true_are_verified() {
        assert!(email_verified(Some(&json!(true))));
        assert!(email_verified(Some(&json!("true"))));

        let unverified = [
            json!(false),
            json!(null),
            json!(0),
            json!(1),
            json!(""),
            json!("false"),
            json!("no"),
            json!("yes"),
            json!("TRUE"),
            json!("True"),
            json!(" true"),
            json!("true "),
            json!([true]),
            json!({"verified": true}),
        ];
        for claim in &unverified {
            assert!(!email_verified(Some(claim)), "{claim}");
        }
        assert!(!email_verified(None));
    }

    #[test]
    fn unit_github_only_a_primary_verified_address_is_eligible() {
        let emails = json!([
            {"email": "secondary@example.com", "primary": false, "verified": true},
            {"email": "unverified@example.com", "primary": true, "verified": false},
            {"email": "primary@example.com", "primary": true, "verified": true},
        ]);
        assert_eq!(
            github_verified_primary_email(&emails).as_deref(),
            Some("primary@example.com")
        );

        for rejected in [
            json!([{"email": "a@example.com", "primary": false, "verified": true}]),
            json!([{"email": "a@example.com", "primary": true, "verified": false}]),
            json!([{"email": "a@example.com", "primary": true}]),
            json!([{"email": "a@example.com", "primary": "true", "verified": "true"}]),
            json!([{"email": "", "primary": true, "verified": true}]),
            json!([{"primary": true, "verified": true}]),
            json!([]),
            json!({"email": "a@example.com", "primary": true, "verified": true}),
            json!(null),
        ] {
            assert_eq!(github_verified_primary_email(&rejected), None, "{rejected}");
        }
    }

    #[test]
    fn unit_github_emails_endpoint_extends_the_userinfo_path() {
        assert_eq!(
            github_emails_endpoint("https://api.github.com/user").as_deref(),
            Some("https://api.github.com/user/emails")
        );
        assert_eq!(
            github_emails_endpoint("http://127.0.0.1:8080/user/").as_deref(),
            Some("http://127.0.0.1:8080/user/emails")
        );
        assert_eq!(github_emails_endpoint("not a url"), None);
    }

    #[test]
    fn unit_avatar_url_accepts_only_https_urls_and_discord_hashes() {
        assert_eq!(
            avatar_url(Preset::Google, "1", Some("https://example.com/a.png")).as_deref(),
            Some("https://example.com/a.png")
        );
        for rejected in [
            "http://127.0.0.1/a.png",
            "javascript:alert(1)",
            "data:image/png;base64,AAAA",
            "ftp://example.com/a.png",
            "//example.com/a.png",
            "",
        ] {
            assert_eq!(
                avatar_url(Preset::Google, "1", Some(rejected)),
                None,
                "{rejected}"
            );
        }
        assert_eq!(avatar_url(Preset::Google, "1", None), None);
        assert_eq!(
            avatar_url(Preset::Discord, "80351110224678912", Some("8342729096ea3675442027381ff50dfe"))
                .as_deref(),
            Some("https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png")
        );
        assert_eq!(
            avatar_url(Preset::Discord, "not-numeric", Some("8342729096ea")),
            None
        );
        assert_eq!(
            avatar_url(Preset::Google, "80351110224678912", Some("8342729096ea")),
            None
        );
    }

    const FIXTURES: [(&str, Preset, Protocol, &str); 11] = [
        (
            "google",
            Preset::Google,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/google.json"),
        ),
        (
            "github",
            Preset::Github,
            Protocol::OAuth2,
            include_str!("../../../tests/fixtures/idp/presets/github.json"),
        ),
        (
            "discord",
            Preset::Discord,
            Protocol::OAuth2,
            include_str!("../../../tests/fixtures/idp/presets/discord.json"),
        ),
        (
            "pocket_id",
            Preset::PocketId,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/pocket_id.json"),
        ),
        (
            "authentik",
            Preset::Authentik,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/authentik.json"),
        ),
        (
            "zitadel",
            Preset::Zitadel,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/zitadel.json"),
        ),
        (
            "auth0",
            Preset::Auth0,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/auth0.json"),
        ),
        (
            "kinde",
            Preset::Kinde,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/kinde.json"),
        ),
        (
            "frontegg",
            Preset::Frontegg,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/frontegg.json"),
        ),
        (
            "generic_oidc",
            Preset::Generic,
            Protocol::Oidc,
            include_str!("../../../tests/fixtures/idp/presets/generic_oidc.json"),
        ),
        (
            "generic_oauth2",
            Preset::Generic,
            Protocol::OAuth2,
            include_str!("../../../tests/fixtures/idp/presets/generic_oauth2.json"),
        ),
    ];

    const NOW: i64 = 1_800_000_000;

    fn mapping_of(fixture: &Value, preset: Preset, protocol: Protocol) -> ClaimMapping {
        match fixture.get("mapping") {
            Some(custom) => {
                let field = |name: &str| custom[name].as_str().unwrap().to_owned();
                ClaimMapping {
                    subject: field("subject"),
                    email: field("email"),
                    email_verified: field("emailVerified"),
                    username: field("username"),
                    name: field("name"),
                    picture: field("picture"),
                }
            }
            None => spec_for(preset, protocol).claim_mapping(),
        }
    }

    struct Observed<'a> {
        subject: Option<&'a str>,
        email: Option<&'a str>,
        verified: bool,
        username: Option<&'a str>,
        name: Option<&'a str>,
        picture: Option<&'a str>,
    }

    fn assert_expected(name: &str, expected: &Value, observed: &Observed<'_>) {
        assert_eq!(
            observed.subject,
            expected["subject"].as_str(),
            "{name} subject"
        );
        assert_eq!(observed.email, expected["email"].as_str(), "{name} email");
        assert_eq!(
            observed.verified, expected["emailVerified"],
            "{name} emailVerified"
        );
        assert_eq!(
            observed.username,
            expected["username"].as_str(),
            "{name} username"
        );
        assert_eq!(observed.name, expected["name"].as_str(), "{name} name");
        assert_eq!(
            observed.picture,
            expected["picture"].as_str(),
            "{name} picture"
        );
    }

    #[test]
    fn regression_314_provider_preset_claim_mapping() {
        for spec in &CATALOGUE {
            assert!(
                FIXTURES
                    .iter()
                    .any(|(_, preset, protocol, _)| *preset == spec.preset
                        && *protocol == spec.protocol),
                "no committed claim fixture for {} ({})",
                spec.display_name,
                spec.protocol.as_str()
            );
        }

        for (name, preset, protocol, raw) in FIXTURES {
            let fixture: Value = serde_json::from_str(raw).unwrap();
            let mapping = mapping_of(&fixture, preset, protocol);
            let document = &fixture["document"];
            let expected = &fixture["expected"];

            let profile = profile_from_claims(document, &mapping);
            assert_expected(
                name,
                expected,
                &Observed {
                    subject: profile.subject.as_deref(),
                    email: profile.email.as_deref(),
                    verified: profile.email_verified,
                    username: profile.username.as_deref(),
                    name: profile.name.as_deref(),
                    picture: profile.picture.as_deref(),
                },
            );

            let mut without_subject = document.clone();
            without_subject
                .as_object_mut()
                .unwrap()
                .remove(&mapping.subject);
            assert_eq!(
                profile_from_claims(&without_subject, &mapping).subject,
                None,
                "{name} must not invent a subject"
            );

            if let Some(absent) = fixture["absent"]["remove"].as_str() {
                let mut trimmed = document.clone();
                trimmed.as_object_mut().unwrap().remove(absent);
                let profile = profile_from_claims(&trimmed, &mapping);
                assert_eq!(profile.picture, None, "{name} without {absent}");
                assert_eq!(profile.subject.as_deref(), expected["subject"].as_str());
                assert_eq!(profile.email.as_deref(), expected["email"].as_str());
            }

            if protocol == Protocol::Oidc {
                let mut claims = document.clone();
                let object = claims.as_object_mut().unwrap();
                object.insert("iss".to_owned(), json!("https://issuer.test"));
                object.insert("aud".to_owned(), json!("client"));
                object.insert("exp".to_owned(), json!(NOW + 300));
                object.insert("iat".to_owned(), json!(NOW));
                object.insert("nonce".to_owned(), json!("expected"));
                let context = ClaimContext {
                    issuer: "https://issuer.test",
                    client_id: "client",
                    expected_nonce: "expected",
                    purpose: ValidationPurpose::Login,
                    access_token: None,
                    mapping: &mapping,
                };
                let validated =
                    check_claims(&claims, &context, JwsAlgorithm::Rs256, "k1", NOW).unwrap();
                assert_expected(
                    name,
                    expected,
                    &Observed {
                        subject: Some(&validated.subject),
                        email: validated.email.as_deref(),
                        verified: validated.email_verified,
                        username: validated.username.as_deref(),
                        name: validated.name.as_deref(),
                        picture: validated.picture.as_deref(),
                    },
                );

                claims.as_object_mut().unwrap().remove(&mapping.subject);
                assert_eq!(
                    check_claims(&claims, &context, JwsAlgorithm::Rs256, "k1", NOW)
                        .unwrap_err()
                        .api_code(),
                    crate::domain::error_code::ErrorCode::ProviderSubjectMissing,
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn regression_314_github_and_discord_preset_specifics() {
        let github: Value = serde_json::from_str(FIXTURES[1].3).unwrap();
        assert_eq!(
            github_verified_primary_email(&github["emails"]).as_deref(),
            github["eligibleEmail"].as_str()
        );
        let profile = profile_from_claims(
            &github["document"],
            &spec_for(Preset::Github, Protocol::OAuth2).claim_mapping(),
        );
        assert!(
            !profile.email_verified,
            "GitHub /user never proves an e-mail"
        );

        let discord: Value = serde_json::from_str(FIXTURES[2].3).unwrap();
        let profile = profile_from_claims(
            &discord["document"],
            &spec_for(Preset::Discord, Protocol::OAuth2).claim_mapping(),
        );
        let url = avatar_url(
            Preset::Discord,
            profile.subject.as_deref().unwrap(),
            profile.picture.as_deref(),
        )
        .unwrap();
        assert_eq!(
            url,
            "https://cdn.discordapp.com/avatars/80351110224678912/8342729096ea3675442027381ff50dfe.png"
        );
    }
}
