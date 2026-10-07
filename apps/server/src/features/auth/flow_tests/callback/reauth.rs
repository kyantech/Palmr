use super::link::{assert_link_redirect, Member};
use super::*;
use crate::features::identity_providers::model::AuthorizePurpose;

const REAUTHENTICATE: &str = "/api/v1/auth/reauthenticate";
const REAUTH_COMPLETE: &str = "https://files.example.test/auth/reauth-complete?status=success";
const REAUTH_TARGET: &str = "/auth/reauth-complete?channel=";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub(super) struct SessionFacts {
    pub(super) id: String,
    pub(super) token_hash: String,
    pub(super) csrf_token_hash: String,
    pub(super) state: String,
    pub(super) auth_method: String,
    pub(super) identity_link_id: Option<String>,
    pub(super) created_at: String,
    pub(super) last_auth_at: String,
}

impl Federation {
    pub(super) async fn external_oidc_member(
        &self,
        slug: &str,
        subject: &str,
        email: &str,
    ) -> Member {
        self.external_member(slug, subject, email).await
    }

    async fn request_reauth(&self, creds: &Credentials, body: &Value) -> Fetched {
        self.stack
            .call(
                Call::new(Method::POST, REAUTHENTICATE, creds).json(body),
                self.next_peer(),
            )
            .await
    }

    pub(super) async fn begin_reauth(&self, creds: &Credentials) -> Begun {
        self.begin_reauth_channel(creds).await.0
    }

    async fn begin_reauth_channel(&self, creds: &Credentials) -> (Begun, String) {
        let fetched = self.request_reauth(creds, &json!({})).await;
        assert_eq!(fetched.status, StatusCode::ACCEPTED, "{}", fetched.text());
        let body = fetched.json();
        assert_eq!(body["accepted"], true);
        let channel = body["externalReauthChannel"].as_str().unwrap().to_owned();
        assert_channel_shape(&channel);
        let url = Url::parse(body["externalReauthUrl"].as_str().unwrap()).unwrap();
        let state = url
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let nonce: String =
            sqlx::query_scalar("SELECT nonce FROM oauth_auth_requests WHERE state_hash = ?1")
                .bind(digest(&state))
                .fetch_one(self.stack.pools.reader().executor())
                .await
                .unwrap();
        let begun = Begun {
            binding: fetched.cookie("palmr_oauth"),
            state,
            nonce,
        };
        (begun, channel)
    }

    pub(super) async fn session_facts(&self, raw: &str) -> SessionFacts {
        sqlx::query_as(
            "SELECT id, token_hash, csrf_token_hash, state, auth_method, identity_link_id,
                    created_at, last_auth_at
               FROM sessions WHERE token_hash = ?1",
        )
        .bind(digest(raw))
        .fetch_one(self.stack.pools.reader().executor())
        .await
        .unwrap()
    }

    async fn authorization_query(&self, creds: &Credentials) -> Vec<(String, String)> {
        let fetched = self.request_reauth(creds, &json!({})).await;
        assert_eq!(fetched.status, StatusCode::ACCEPTED, "{}", fetched.text());
        let url = Url::parse(fetched.json()["externalReauthUrl"].as_str().unwrap()).unwrap();
        url.query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect()
    }

    async fn reauth_oidc(
        &self,
        slug: &str,
        member: &Member,
        set: Value,
        remove: &[&str],
    ) -> Fetched {
        let begun = self.begin_reauth(&member.creds).await;
        self.arm_oidc(&begun, set, remove);
        self.finish_as(slug, &begun, Some(&member.creds)).await
    }

    async fn assert_nothing_gained(&self, users: i64, sessions: i64, links: usize) {
        assert_eq!(self.count("SELECT COUNT(*) FROM users").await, users);
        assert_eq!(self.session_count().await, sessions);
        assert_eq!(self.links().await.len(), links);
    }
}

pub(super) fn assert_reauth_redirect(fetched: &Fetched) -> String {
    assert_eq!(
        fetched.status,
        StatusCode::SEE_OTHER,
        "{}",
        location(fetched)
    );
    let target = location(fetched);
    let channel = target
        .strip_prefix(&format!("{REAUTH_COMPLETE}&channel="))
        .unwrap_or_else(|| panic!("{target}"));
    assert_channel_shape(channel);
    assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(fetched.set_cookies(), vec![OAUTH_CLEARED.to_owned()]);
    channel.to_owned()
}

#[tokio::test]
async fn it_sso_reauth_sets_last_auth_at() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    let second = f
        .login_oidc(
            "corp",
            json!({ "sub": "sso-subject", "email": "sso@example.test", "email_verified": true }),
            &[],
        )
        .await;
    let second = assert_signed_in(&second, "/overview");
    let sessions = f.session_count().await;

    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let stale = f.request_link("corp", &member.creds).await;
    assert_eq!(stale.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(stale.json()["error"]["details"]["method"], "external");

    let before = f.session_facts(&member.creds.session).await;
    let untouched = f.session_facts(&second.session).await;
    let (begun, channel) = f.begin_reauth_channel(&member.creds).await;
    assert_eq!(
        f.session_facts(&member.creds.session).await,
        before,
        "starting re-authentication changes nothing"
    );
    f.arm_oidc(&begun, json!({ "sub": "sso-subject" }), &[]);
    let fetched = f.finish_as("corp", &begun, Some(&member.creds)).await;
    assert_eq!(assert_reauth_redirect(&fetched), channel);
    assert_eq!(
        location(&fetched),
        format!("{REAUTH_COMPLETE}&channel={channel}")
    );

    let after = f.session_facts(&member.creds.session).await;
    assert_eq!(after.id, before.id);
    assert_eq!(after.token_hash, before.token_hash);
    assert_eq!(after.csrf_token_hash, before.csrf_token_hash);
    assert_eq!(after.state, "active");
    assert_eq!(after.identity_link_id, before.identity_link_id);
    assert_eq!(after.created_at, before.created_at);
    assert!(after.last_auth_at > before.last_auth_at);
    assert_eq!(
        after.last_auth_at,
        Timestamp::try_from(f.stack.clock.now())
            .unwrap()
            .to_string()
    );
    assert_eq!(f.session_facts(&second.session).await, untouched);
    assert_eq!(f.session_count().await, sessions, "no session is created");

    let me = f
        .stack
        .get("/api/v1/auth/me", Some(&member.creds.session), 70)
        .await;
    assert_eq!(me.status, StatusCode::OK, "the same token stays valid");
    let reopened = f.request_link("corp", &member.creds).await;
    assert_eq!(
        reopened.error_code(),
        "PROVIDER_IDENTITY_ALREADY_LINKED",
        "the recent-auth window is open again"
    );

    assert_eq!(
        f.count("SELECT COUNT(*) FROM login_attempts").await,
        1,
        "no Palmr credential check is counted (the operator's own login only)"
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_reauth_start_response_and_request_row() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    f.oauth2("plain", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;

    let fetched = f.request_reauth(&member.creds, &json!({})).await;
    assert_eq!(fetched.status, StatusCode::ACCEPTED, "{}", fetched.text());
    assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
    let body = fetched.json();
    assert_eq!(body["accepted"], true);
    assert_eq!(body.as_object().unwrap().len(), 3);
    let channel = body["externalReauthChannel"].as_str().unwrap();
    assert_channel_shape(channel);
    let url = Url::parse(body["externalReauthUrl"].as_str().unwrap()).unwrap();
    assert!(!url.path().starts_with("/api/"), "{url}");
    let query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(form_value(&query, "prompt"), Some("login"));
    assert_eq!(form_value(&query, "max_age"), Some("0"));
    assert_eq!(form_value(&query, "code_challenge_method"), Some("S256"));
    assert!(form_value(&query, "nonce").is_some());
    assert!(!url.as_str().contains(channel), "{url}");
    assert!(query.iter().all(|(_, value)| !value.contains(channel)));
    assert_eq!(
        form_value(&query, "redirect_uri"),
        Some("https://files.example.test/api/v1/auth/providers/corp/callback")
    );
    let cookie = fetched
        .set_cookies()
        .into_iter()
        .find(|cookie| cookie.starts_with("palmr_oauth="))
        .unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));

    let row: (String, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT purpose, link_user_id, post_auth_path, provider_id FROM oauth_auth_requests
          WHERE purpose = 'reauth'",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(row.0, "reauth");
    assert_eq!(row.1, Some(member.id.to_string()));
    assert_eq!(row.2, Some(format!("{REAUTH_TARGET}{channel}")));
    assert_eq!(row.3, f.provider_id("corp").await);

    let oauth2 = f
        .external_member_oauth2("plain", "4242", "plain@example.test")
        .await;
    let query = f.authorization_query(&oauth2.creds).await;
    assert_eq!(form_value(&query, "prompt"), Some("login"));
    assert!(form_value(&query, "max_age").is_none());
    assert!(form_value(&query, "nonce").is_none());
    assert_eq!(form_value(&query, "code_challenge_method"), Some("S256"));
    f.stack.stop().await;
}

impl Federation {
    async fn external_member_oauth2(&self, slug: &str, subject: &str, email: &str) -> Member {
        let begun = self.begin(slug, None).await;
        self.idp.set_token_response(json!({
            "access_token": "oauth-access-token",
            "token_type": "bearer",
        }));
        self.idp.set_userinfo(json!({
            "sub": subject, "email": email, "email_verified": true,
        }));
        let fetched = self.finish(slug, &begun).await;
        let creds = assert_signed_in(&fetched, "/overview");
        let id = self.user_by_email(email).await;
        Member { id, creds }
    }
}

#[tokio::test]
async fn it_sso_reauth_provider_comes_from_the_session_identity() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    f.oidc("other", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    f.link("other", member.id, "other-subject", "active").await;

    let query = f.authorization_query(&member.creds).await;
    assert_eq!(
        form_value(&query, "redirect_uri"),
        Some("https://files.example.test/api/v1/auth/providers/corp/callback")
    );
    let provider: String =
        sqlx::query_scalar("SELECT provider_id FROM oauth_auth_requests WHERE purpose = 'reauth'")
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(provider, f.provider_id("corp").await);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_reauth_start_refusals_create_no_request() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;

    let requests_before = f.auth_requests().await;
    f.stack
        .execute("UPDATE identity_providers SET is_enabled = 0 WHERE key = 'corp'")
        .await;
    let disabled = f.request_reauth(&member.creds, &json!({})).await;
    assert_eq!(disabled.status, StatusCode::FORBIDDEN);
    assert_eq!(disabled.error_code(), "PROVIDER_DISABLED");
    f.stack
        .execute("UPDATE identity_providers SET is_enabled = 1 WHERE key = 'corp'")
        .await;

    f.stack
        .setting("auth_providers_enabled", "boolean", "false")
        .await;
    let globally_off = f.request_reauth(&member.creds, &json!({})).await;
    assert_eq!(globally_off.error_code(), "PROVIDER_DISABLED");
    f.stack
        .setting("auth_providers_enabled", "boolean", "true")
        .await;

    f.stack
        .execute("UPDATE identity_links SET state = 'suspended', suspended_at = '2026-09-25T12:00:00.000Z'")
        .await;
    let suspended = f.request_reauth(&member.creds, &json!({})).await;
    assert_eq!(suspended.status, StatusCode::NOT_FOUND);
    assert_eq!(suspended.error_code(), "PROVIDER_LINK_NOT_FOUND");
    f.stack
        .execute("UPDATE identity_links SET state = 'active', suspended_at = NULL")
        .await;

    f.stack
        .execute("UPDATE sessions SET identity_link_id = NULL WHERE auth_method = 'external'")
        .await;
    let unbound = f.request_reauth(&member.creds, &json!({})).await;
    assert_eq!(unbound.error_code(), "PROVIDER_LINK_NOT_FOUND");

    let body = f
        .request_reauth(&member.creds, &json!({ "password": PASSWORD }))
        .await;
    assert_eq!(body.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body.error_code(), "VALIDATION_ERROR");

    for refused in [&disabled, &globally_off, &suspended, &unbound, &body] {
        assert!(refused.set_cookies().is_empty());
    }
    assert_eq!(f.auth_requests().await, requests_before);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_reauth_keeps_the_local_branch_for_hybrid_accounts() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;
    f.link("corp", member.id, "hybrid-subject", "active").await;
    f.stack
        .execute(&format!(
            "UPDATE sessions SET identity_link_id = (SELECT id FROM identity_links LIMIT 1)
              WHERE user_id = '{}'",
            member.id
        ))
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));

    let empty = f.request_reauth(&member.creds, &json!({})).await;
    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(empty.error_code(), "VALIDATION_ERROR");
    assert_eq!(f.auth_requests().await, 0);

    let wrong = f
        .request_reauth(&member.creds, &json!({ "password": WRONG }))
        .await;
    assert_eq!(wrong.error_code(), "AUTH_INVALID_CREDENTIALS");
    let right = f
        .request_reauth(&member.creds, &json!({ "password": PASSWORD }))
        .await;
    assert_eq!(right.status, StatusCode::NO_CONTENT);
    assert!(right.set_cookies().is_empty());
    f.stack.stop().await;
}

#[tokio::test]
async fn it_reauth_callback_requires_the_same_live_session_and_user() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    let stranger = f
        .external_oidc_member("corp", "stranger-subject", "stranger@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;
    let stranger_before = f.session_facts(&stranger.creds.session).await;
    let users = f.count("SELECT COUNT(*) FROM users").await;
    let sessions = f.session_count().await;
    let links = f.links().await.len();

    let anonymous = f.begin_reauth(&member.creds).await;
    f.arm_oidc(&anonymous, json!({ "sub": "sso-subject" }), &[]);
    assert_reauth_failure(
        &f.finish_as("corp", &anonymous, None).await,
        "PROVIDER_STATE_INVALID",
    );

    let foreign = f.begin_reauth(&member.creds).await;
    f.arm_oidc(&foreign, json!({ "sub": "sso-subject" }), &[]);
    assert_reauth_failure(
        &f.finish_as("corp", &foreign, Some(&stranger.creds)).await,
        "PROVIDER_STATE_INVALID",
    );

    assert_eq!(f.session_facts(&member.creds.session).await, before);
    assert_eq!(
        f.session_facts(&stranger.creds.session).await,
        stranger_before
    );
    f.assert_nothing_gained(users, sessions, links).await;

    let replay_source = f.begin_reauth(&member.creds).await;
    f.arm_oidc(&replay_source, json!({ "sub": "sso-subject" }), &[]);
    assert_reauth_redirect(
        &f.finish_as("corp", &replay_source, Some(&member.creds))
            .await,
    );
    assert_failure(
        &f.finish_as("corp", &replay_source, Some(&member.creds))
            .await,
        "PROVIDER_STATE_INVALID",
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_reauth_callback_must_prove_the_same_subject_and_provider() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    f.oidc("other", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    let intruder = f
        .external_oidc_member("other", "intruder-subject", "intruder@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;
    let users = f.count("SELECT COUNT(*) FROM users").await;
    let sessions = f.session_count().await;
    let links = f.links().await.len();

    let wrong_subject = f
        .reauth_oidc("corp", &member, json!({ "sub": "someone-else" }), &[])
        .await;
    assert_reauth_failure(&wrong_subject, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(f.session_facts(&member.creds.session).await, before);

    let begun = f.begin_reauth(&member.creds).await;
    let other = f.provider_id("other").await;
    f.stack
        .execute(&format!(
            "UPDATE oauth_auth_requests
                SET provider_id = '{other}',
                    redirect_uri = 'https://files.example.test/api/v1/auth/providers/other/callback'
              WHERE state_hash = '{}'",
            digest(&begun.state)
        ))
        .await;
    f.arm_oidc(&begun, json!({ "sub": "sso-subject" }), &[]);
    let wrong_provider = f.finish_as("other", &begun, Some(&member.creds)).await;
    assert_reauth_failure(&wrong_provider, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(f.session_facts(&member.creds.session).await, before);

    let intruding = f.begin_reauth(&intruder.creds).await;
    f.arm_oidc(&intruding, json!({ "sub": "sso-subject" }), &[]);
    let swapped_user = f
        .finish_as("other", &intruding, Some(&intruder.creds))
        .await;
    assert_reauth_failure(&swapped_user, "AUTH_RECENT_AUTH_REQUIRED");

    f.assert_nothing_gained(users, sessions, links).await;
    f.stack.stop().await;
}

#[tokio::test]
async fn it_oidc_reauth_enforces_auth_time() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;
    let now = f.now();

    let missing = f
        .reauth_oidc(
            "corp",
            &member,
            json!({ "sub": "sso-subject" }),
            &["auth_time"],
        )
        .await;
    assert_reauth_failure(&missing, "AUTH_RECENT_AUTH_REQUIRED");
    let stale = f
        .reauth_oidc(
            "corp",
            &member,
            json!({ "sub": "sso-subject", "auth_time": now - 7 * 60 }),
            &[],
        )
        .await;
    assert_reauth_failure(&stale, "AUTH_RECENT_AUTH_REQUIRED");
    let future = f
        .reauth_oidc(
            "corp",
            &member,
            json!({ "sub": "sso-subject", "auth_time": now + 120 }),
            &[],
        )
        .await;
    assert_reauth_failure(&future, "AUTH_RECENT_AUTH_REQUIRED");
    let unparsable = f
        .reauth_oidc(
            "corp",
            &member,
            json!({ "sub": "sso-subject", "auth_time": "yesterday" }),
            &[],
        )
        .await;
    assert_reauth_failure(&unparsable, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(f.session_facts(&member.creds.session).await, before);

    let forged = f
        .reauth_oidc(
            "corp",
            &member,
            json!({ "sub": "sso-subject", "nonce": "forged" }),
            &[],
        )
        .await;
    assert_reauth_failure(&forged, "PROVIDER_ID_TOKEN_INVALID");

    let recent = f
        .reauth_oidc(
            "corp",
            &member,
            json!({ "sub": "sso-subject", "auth_time": f.now() - 60 }),
            &[],
        )
        .await;
    assert_reauth_redirect(&recent);
    assert!(f.session_facts(&member.creds.session).await.last_auth_at > before.last_auth_at);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_oauth2_reauth_requires_a_fresh_round_trip_and_the_same_subject() {
    let f = Federation::start().await;
    f.oauth2("plain", json!({ "autoProvision": true })).await;
    let member = f
        .external_member_oauth2("plain", "4242", "plain@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;
    let arm = |subject: &str| {
        f.idp.set_token_response(json!({
            "access_token": "oauth-access-token",
            "token_type": "bearer",
        }));
        f.idp.set_userinfo(json!({
            "sub": subject, "email": "plain@example.test", "email_verified": true,
        }));
    };

    let mismatched = f.begin_reauth(&member.creds).await;
    arm("9999");
    assert_reauth_failure(
        &f.finish_as("plain", &mismatched, Some(&member.creds)).await,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_eq!(f.session_facts(&member.creds.session).await, before);

    let slow = f.begin_reauth(&member.creds).await;
    f.stack.clock.advance(Duration::from_secs(5 * 60 + 1));
    arm("4242");
    let requests = f.idp.token_requests().await.len();
    assert_reauth_failure(
        &f.finish_as("plain", &slow, Some(&member.creds)).await,
        "AUTH_RECENT_AUTH_REQUIRED",
    );
    assert_eq!(
        f.idp.token_requests().await.len(),
        requests,
        "an old request never reaches the provider"
    );
    assert_eq!(f.session_facts(&member.creds.session).await, before);

    f.stack
        .execute(&format!(
            "UPDATE users SET totp_enabled = 1 WHERE id = '{}'",
            member.id
        ))
        .await;
    let attempts = f.count("SELECT COUNT(*) FROM login_attempts").await;
    let fresh = f.begin_reauth(&member.creds).await;
    arm("4242");
    let fetched = f.finish_as("plain", &fresh, Some(&member.creds)).await;
    assert_reauth_redirect(&fetched);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM login_attempts").await,
        attempts,
        "no Palmr credential or second factor is counted"
    );
    assert_eq!(
        f.count("SELECT COUNT(*) FROM sessions WHERE state = 'mfa_pending'")
            .await,
        0
    );
    let after = f.session_facts(&member.creds.session).await;
    assert!(after.last_auth_at > before.last_auth_at);
    assert_eq!(after.token_hash, before.token_hash);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_reauth_and_link_hold_no_write_transaction_across_provider_calls() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    f.oidc("second", json!({})).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    let probe = "UPDATE app_settings SET updated_at = updated_at
                  WHERE key = 'auth_providers_enabled'";

    let reauth = f.begin_reauth(&member.creds).await;
    f.arm_oidc(&reauth, json!({ "sub": "sso-subject" }), &[]);
    f.idp.set_token_delay(Some(Duration::from_millis(1500)));
    let started = Instant::now();
    let (fetched, waited) =
        tokio::join!(f.finish_as("corp", &reauth, Some(&member.creds)), async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            f.stack.execute(probe).await;
            started.elapsed()
        });
    assert_reauth_redirect(&fetched);
    assert!(
        waited < Duration::from_millis(1200),
        "a write waited {waited:?} for the token exchange of a reauth"
    );

    let link = f.begin_link("second", &member.creds).await;
    f.arm_oidc(&link, json!({ "sub": "second-subject" }), &[]);
    let started = Instant::now();
    let (fetched, waited) =
        tokio::join!(f.finish_as("second", &link, Some(&member.creds)), async {
            tokio::time::sleep(Duration::from_millis(300)).await;
            f.stack.execute(probe).await;
            started.elapsed()
        });
    assert_link_redirect(&fetched);
    assert!(
        waited < Duration::from_millis(1200),
        "a write waited {waited:?} for the token exchange of a link"
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_reauth_channels_are_independent_per_challenge() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;

    let (first, first_channel) = f.begin_reauth_channel(&member.creds).await;
    let (second, second_channel) = f.begin_reauth_channel(&member.creds).await;
    assert_ne!(first_channel, second_channel);
    let stored: Vec<String> = sqlx::query_scalar(
        "SELECT post_auth_path FROM oauth_auth_requests WHERE purpose = 'reauth'
          ORDER BY post_auth_path",
    )
    .fetch_all(f.stack.pools.reader().executor())
    .await
    .unwrap();
    let mut expected = vec![
        format!("{REAUTH_TARGET}{first_channel}"),
        format!("{REAUTH_TARGET}{second_channel}"),
    ];
    expected.sort();
    assert_eq!(stored, expected);

    f.arm_oidc(&second, json!({ "sub": "sso-subject" }), &[]);
    let fetched = f.finish_as("corp", &second, Some(&member.creds)).await;
    assert_eq!(assert_reauth_redirect(&fetched), second_channel);

    f.arm_oidc(&first, json!({ "sub": "wrong-subject" }), &[]);
    let fetched = f.finish_as("corp", &first, Some(&member.creds)).await;
    let target = Url::parse(&location(&fetched)).unwrap();
    let query: Vec<(String, String)> = target
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(form_value(&query, "channel"), Some(first_channel.as_str()));
    assert_eq!(
        form_value(&query, "error"),
        Some("AUTH_RECENT_AUTH_REQUIRED")
    );
    assert_ne!(form_value(&query, "channel"), Some(second_channel.as_str()));

    let after = f.session_facts(&member.creds.session).await;
    assert_eq!(after.token_hash, before.token_hash);
    assert!(after.last_auth_at > before.last_auth_at);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_reauth_failure_landing_carries_the_stored_channel() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;

    let (denied, channel) = f.begin_reauth_channel(&member.creds).await;
    let fetched = f
        .callback(
            "corp",
            &format!(
                "error=access_denied&error_description=Upstream%20prose&state={}",
                denied.state
            ),
            Some(&denied.binding),
        )
        .await;
    assert_reauth_failure(&fetched, "PROVIDER_AUTH_DENIED");
    let landing = location(&fetched);
    assert!(
        landing.ends_with(&format!("&channel={channel}")),
        "{landing}"
    );
    for secret in [
        denied.state.as_str(),
        denied.nonce.as_str(),
        denied.binding.as_str(),
        "Upstream",
    ] {
        assert!(!landing.contains(secret), "{secret}");
    }

    let (exchange, channel) = f.begin_reauth_channel(&member.creds).await;
    f.idp
        .set_token_status(400, json!({ "error": "invalid_grant" }));
    let fetched = f.finish_as("corp", &exchange, Some(&member.creds)).await;
    assert_reauth_failure(&fetched, "PROVIDER_CODE_EXCHANGE_FAILED");
    assert!(location(&fetched).ends_with(&format!("&channel={channel}")));

    assert_eq!(f.session_facts(&member.creds.session).await, before);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_reauth_channel_is_never_taken_from_callback_input() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    let forged = mint_channel().unwrap();

    let (begun, channel) = f.begin_reauth_channel(&member.creds).await;
    let fetched = f
        .callback(
            "corp",
            &format!("error=access_denied&channel={forged}&state={}", begun.state),
            Some(&begun.binding),
        )
        .await;
    assert_reauth_failure(&fetched, "PROVIDER_AUTH_DENIED");
    let landing = location(&fetched);
    assert!(
        landing.ends_with(&format!("&channel={channel}")),
        "{landing}"
    );
    assert!(!landing.contains(&forged));

    for query in [
        format!("error=access_denied&channel={forged}"),
        format!("code=auth-code&channel={forged}&state=unknown"),
    ] {
        let fetched = f.callback("corp", &query, Some(&begun.binding)).await;
        let landing = location(&fetched);
        assert!(
            landing.starts_with("https://files.example.test/login?"),
            "{landing}"
        );
        assert!(
            !landing.contains("channel") && !landing.contains(&forged),
            "{landing}"
        );
    }
    let fetched = f.callback("corp", &format!("channel={forged}"), None).await;
    assert!(!location(&fetched).contains("channel"));
    f.stack.stop().await;
}

#[tokio::test]
async fn it_sso_reauth_without_a_stored_channel_fails_closed() {
    let f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f
        .external_oidc_member("corp", "sso-subject", "sso@example.test")
        .await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let before = f.session_facts(&member.creds.session).await;

    let begun = f.begin_reauth(&member.creds).await;
    f.stack
        .execute(&format!(
            "UPDATE oauth_auth_requests SET post_auth_path = '/overview' WHERE state_hash = '{}'",
            digest(&begun.state)
        ))
        .await;
    f.arm_oidc(&begun, json!({ "sub": "sso-subject" }), &[]);
    let exchanges = f.idp.token_requests().await.len();
    let fetched = f.finish_as("corp", &begun, Some(&member.creds)).await;
    assert_eq!(fetched.status, StatusCode::SEE_OTHER);
    let landing = location(&fetched);
    assert!(
        landing.starts_with(
            "https://files.example.test/auth/reauth-complete?status=error&error=PROVIDER_STATE_INVALID&requestId="
        ),
        "{landing}"
    );
    assert!(!landing.contains("channel"), "{landing}");
    assert_eq!(f.idp.token_requests().await.len(), exchanges);
    assert_eq!(f.session_facts(&member.creds.session).await, before);

    let requests = f.auth_requests().await;
    for return_to in [None, Some("/overview"), Some("/auth/reauth-complete")] {
        let context = AuthorizeContext {
            purpose: AuthorizePurpose::Reauth,
            bound_user_id: Some(member.id),
            return_to: return_to.map(str::to_owned),
        };
        assert!(
            f.stack.providers.authorize("corp", context).await.is_err(),
            "{return_to:?}"
        );
    }
    assert_eq!(
        f.auth_requests().await,
        requests,
        "no request row is minted"
    );
    f.stack.stop().await;
}

#[test]
fn unit_non_login_purposes_never_reach_account_resolution() {
    let callback = include_str!("../../../identity_providers/callback.rs");
    let link = include_str!("../../../identity_providers/link.rs");
    let reauth = include_str!("../../../identity_providers/reauth.rs");
    assert_eq!(callback.matches("resolve::resolve(").count(), 1);
    let resolver = callback.find("resolve::resolve(").unwrap();
    let sign_in = callback.find("async fn sign_in").unwrap();
    let after_sign_in = callback.find("async fn refused_actor").unwrap();
    assert!((sign_in..after_sign_in).contains(&resolver));
    for module in [link, reauth] {
        for forbidden in [
            "resolve::resolve(",
            "provision::",
            "issue_session",
            "users::insert",
            "AuthMethod::External",
            "LoginSucceeded",
            "login_succeeded",
        ] {
            assert!(!module.contains(forbidden), "{forbidden}");
        }
    }
    assert!(reauth.contains("mark_reauthenticated_in_tx"));
    assert!(!reauth.contains("rotate_in_tx") && !reauth.contains(".rotate("));
    assert!(link.contains("LinkMethod::Manual"));
    let _ = assert_link_redirect;
}
