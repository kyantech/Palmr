use std::time::Duration;

use super::*;

pub(super) const LINK_PREFIX: &str = "https://files.example.test/settings/security";

pub(super) struct Member {
    pub(super) id: UserId,
    pub(super) creds: Credentials,
}

impl Federation {
    pub(super) async fn local_member(&self, username: &str) -> Member {
        let email = format!("{username}@example.test");
        let id = self
            .stack
            .user(UserSpec::local(username, &email, &password_hash()))
            .await;
        let creds = self.stack.signed_in(username, self.next_peer()).await;
        Member { id, creds }
    }

    pub(super) async fn external_member(&self, slug: &str, subject: &str, email: &str) -> Member {
        let fetched = self
            .login_oidc(
                slug,
                json!({ "sub": subject, "email": email, "email_verified": true }),
                &[],
            )
            .await;
        let creds = assert_signed_in(&fetched, "/overview");
        let id = self.user_by_email(email).await;
        Member { id, creds }
    }

    pub(super) async fn request_link(&self, slug: &str, creds: &Credentials) -> Fetched {
        self.stack
            .call(
                Call::new(Method::POST, &format!("{PROVIDERS}/{slug}/link"), creds),
                self.next_peer(),
            )
            .await
    }

    pub(super) async fn begin_link(&self, slug: &str, creds: &Credentials) -> Begun {
        let fetched = self.request_link(slug, creds).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        self.begun_from(&fetched).await
    }

    pub(super) async fn begun_from(&self, fetched: &Fetched) -> Begun {
        let url = Url::parse(fetched.json()["authorizationUrl"].as_str().unwrap()).unwrap();
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
        Begun {
            binding: fetched.cookie("palmr_oauth"),
            state,
            nonce,
        }
    }

    pub(super) async fn finish_as(
        &self,
        slug: &str,
        begun: &Begun,
        session: Option<&Credentials>,
    ) -> Fetched {
        let mut cookies = vec![format!("palmr_oauth={}", begun.binding)];
        if let Some(session) = session {
            cookies.push(format!("palmr_session={}", session.session));
        }
        let request = Request::builder()
            .method(Method::GET)
            .uri(format!(
                "{PROVIDERS}/{slug}/callback?code=auth-code&state={}",
                begun.state
            ))
            .header(COOKIE, cookies.join("; "))
            .body(Body::empty())
            .unwrap();
        self.stack.send(with_peer(request, self.next_peer())).await
    }

    pub(super) async fn link_oidc(
        &self,
        slug: &str,
        member: &Member,
        set: Value,
        remove: &[&str],
    ) -> Fetched {
        let begun = self.begin_link(slug, &member.creds).await;
        self.arm_oidc(&begun, set, remove);
        self.finish_as(slug, &begun, Some(&member.creds)).await
    }

    pub(super) async fn auth_requests(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM oauth_auth_requests").await
    }

    pub(super) async fn audit_rows(&self, action: &str) -> Vec<Value> {
        let rows: Vec<(Option<String>,)> = sqlx::query_as(
            "SELECT metadata_json FROM audit_events WHERE action = ?1 AND result = 'success'
              ORDER BY id",
        )
        .bind(action)
        .fetch_all(self.stack.pools.reader().executor())
        .await
        .unwrap();
        rows.into_iter()
            .map(|(metadata,)| serde_json::from_str(&metadata.unwrap()).unwrap())
            .collect()
    }

    pub(super) async fn live_sessions(&self) -> i64 {
        self.count("SELECT COUNT(*) FROM sessions WHERE state = 'active'")
            .await
    }
}

pub(super) fn assert_link_redirect(fetched: &Fetched) {
    assert_eq!(
        fetched.status,
        StatusCode::SEE_OTHER,
        "{}",
        location(fetched)
    );
    assert_eq!(location(fetched), LINK_PREFIX);
    assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
    assert!(fetched.body.is_empty());
    assert_eq!(fetched.set_cookies(), vec![OAUTH_CLEARED.to_owned()]);
}

#[tokio::test]
async fn it_manual_link_requires_recent_auth() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;

    let anonymous = f
        .stack
        .call(
            Call {
                session: None,
                ..Call::new(
                    Method::POST,
                    &format!("{PROVIDERS}/corp/link"),
                    &member.creds,
                )
            },
            f.next_peer(),
        )
        .await;
    assert_eq!(
        anonymous.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        anonymous.text()
    );
    assert_eq!(f.auth_requests().await, 0);

    f.stack.clock.advance(Duration::from_secs(6 * 60));
    let stale = f.request_link("corp", &member.creds).await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    assert_eq!(stale.error_code(), "AUTH_RECENT_AUTH_REQUIRED");
    assert!(stale.set_cookies().is_empty());
    assert_eq!(f.auth_requests().await, 0);

    let reauthenticated = f
        .stack
        .call(
            Call::new(Method::POST, "/api/v1/auth/reauthenticate", &member.creds)
                .json(&json!({ "password": PASSWORD })),
            f.next_peer(),
        )
        .await;
    assert_eq!(reauthenticated.status, StatusCode::NO_CONTENT);
    let fresh = f.request_link("corp", &member.creds).await;
    assert_eq!(fresh.status, StatusCode::OK, "{}", fresh.text());
    assert_eq!(f.auth_requests().await, 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_manual_link_request_binds_the_caller_and_reuses_the_authorization_service() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    f.oauth2("plain", json!({})).await;
    let member = f.local_member("ada").await;
    let other = f.local_member("bea").await;

    let fetched = f.request_link("corp", &member.creds).await;
    assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
    assert_eq!(fetched.headers.get("cache-control").unwrap(), "no-store");
    let url = Url::parse(fetched.json()["authorizationUrl"].as_str().unwrap()).unwrap();
    let query: Vec<(String, String)> = url
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    assert_eq!(form_value(&query, "code_challenge_method"), Some("S256"));
    assert_eq!(
        form_value(&query, "redirect_uri"),
        Some("https://files.example.test/api/v1/auth/providers/corp/callback")
    );
    assert!(form_value(&query, "nonce").is_some());
    assert!(form_value(&query, "prompt").is_none());
    assert!(form_value(&query, "max_age").is_none());
    let binding = fetched
        .set_cookies()
        .into_iter()
        .find(|cookie| cookie.starts_with("palmr_oauth="))
        .unwrap();
    assert!(binding.contains("HttpOnly"));
    assert!(binding.contains("Path=/api/v1/auth/providers"));
    assert!(binding.contains("SameSite=Lax"));

    let row: (String, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT purpose, link_user_id, post_auth_path, state_hash FROM oauth_auth_requests",
    )
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(row.0, "link");
    assert_eq!(row.1, Some(member.id.to_string()));
    assert_ne!(row.1, Some(other.id.to_string()));
    assert_eq!(row.2.as_deref(), Some("/settings/security"));
    assert_eq!(row.3, digest(form_value(&query, "state").unwrap()));

    let oauth2 = f.request_link("plain", &member.creds).await;
    assert_eq!(oauth2.status, StatusCode::OK, "{}", oauth2.text());
    let url = Url::parse(oauth2.json()["authorizationUrl"].as_str().unwrap()).unwrap();
    assert!(url.query_pairs().all(|(key, _)| key != "nonce"));
    assert_eq!(f.auth_requests().await, 2);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_manual_link_start_refusals_persist_nothing() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    f.oidc("off", json!({})).await;
    let member = f.local_member("ada").await;

    let unknown = f.request_link("missing", &member.creds).await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    assert_eq!(unknown.error_code(), "PROVIDER_NOT_FOUND");

    f.stack
        .execute("UPDATE identity_providers SET is_enabled = 0 WHERE key = 'off'")
        .await;
    let disabled = f.request_link("off", &member.creds).await;
    assert_eq!(disabled.status, StatusCode::FORBIDDEN);
    assert_eq!(disabled.error_code(), "PROVIDER_DISABLED");

    f.stack
        .setting("auth_providers_enabled", "boolean", "false")
        .await;
    let globally_off = f.request_link("corp", &member.creds).await;
    assert_eq!(globally_off.status, StatusCode::FORBIDDEN);
    assert_eq!(globally_off.error_code(), "PROVIDER_DISABLED");
    f.stack
        .setting("auth_providers_enabled", "boolean", "true")
        .await;

    f.link("corp", member.id, "already-mine", "active").await;
    let linked = f.request_link("corp", &member.creds).await;
    assert_eq!(linked.status, StatusCode::CONFLICT);
    assert_eq!(linked.error_code(), "PROVIDER_IDENTITY_ALREADY_LINKED");

    for refused in [&unknown, &disabled, &globally_off, &linked] {
        assert!(refused.set_cookies().is_empty());
    }
    assert_eq!(f.auth_requests().await, 0);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_callback_requires_the_same_live_palmr_session() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let ada = f.local_member("ada").await;
    let bea = f.local_member("bea").await;

    let anonymous = f.begin_link("corp", &ada.creds).await;
    f.arm_oidc(&anonymous, json!({}), &[]);
    let rejected = f.finish_as("corp", &anonymous, None).await;
    assert_failure(&rejected, "PROVIDER_STATE_INVALID");
    assert_eq!(f.idp.token_requests().await.len(), 0);

    let foreign = f.begin_link("corp", &ada.creds).await;
    f.arm_oidc(&foreign, json!({}), &[]);
    let rejected = f.finish_as("corp", &foreign, Some(&bea.creds)).await;
    assert_failure(&rejected, "PROVIDER_STATE_INVALID");

    let stale_cookie = Credentials {
        session: "not-a-session".to_owned(),
        csrf: String::new(),
    };
    let garbage = f.begin_link("corp", &ada.creds).await;
    f.arm_oidc(&garbage, json!({}), &[]);
    let rejected = f.finish_as("corp", &garbage, Some(&stale_cookie)).await;
    assert_failure(&rejected, "PROVIDER_STATE_INVALID");

    let revoked = f.begin_link("corp", &ada.creds).await;
    f.arm_oidc(&revoked, json!({}), &[]);
    f.stack
        .execute(&format!(
            "UPDATE sessions SET state = 'revoked', revoked_at = '2026-09-25T12:00:30.000Z',
                    revoked_reason = 'logout' WHERE user_id = '{}'",
            ada.id
        ))
        .await;
    let rejected = f.finish_as("corp", &revoked, Some(&ada.creds)).await;
    assert_failure(&rejected, "PROVIDER_STATE_INVALID");

    assert_eq!(f.links().await.len(), 0);
    assert_eq!(f.idp.token_requests().await.len(), 0);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM oauth_auth_requests WHERE consumed_at IS NULL")
            .await,
        0
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_manual_link_succeeds_with_an_unverified_email_and_issues_no_session() {
    let mut f = Federation::start().await;
    f.oidc("corp", json!({ "autoProvision": true })).await;
    let member = f.local_member("ada").await;
    let bystander = f.make_user("bystander", "user@example.com").await;
    let users_before = f.count("SELECT COUNT(*) FROM users").await;
    let sessions_before = f.session_count().await;
    f.stack.flush_audit().await;
    let logins_before = f
        .count("SELECT COUNT(*) FROM audit_events WHERE action = 'LOGIN_SUCCEEDED'")
        .await;
    let before = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, token_hash, last_auth_at FROM sessions WHERE user_id = ?1",
    )
    .bind(member.id.to_string())
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();

    let fetched = f
        .link_oidc(
            "corp",
            &member,
            json!({ "sub": "manual-subject", "email": "user@example.com", "email_verified": false }),
            &[],
        )
        .await;
    assert_link_redirect(&fetched);

    let links: Vec<(String, String, String, Option<String>, i64, String)> = sqlx::query_as(
        "SELECT user_id, subject, link_method, email_at_link, email_verified_at_link, state
           FROM identity_links",
    )
    .fetch_all(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(
        links,
        [(
            member.id.to_string(),
            "manual-subject".to_owned(),
            "manual".to_owned(),
            Some("user@example.com".to_owned()),
            0,
            "active".to_owned(),
        )]
    );
    assert_ne!(links[0].0, bystander.to_string());

    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, users_before);
    assert_eq!(f.session_count().await, sessions_before);
    let after = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, token_hash, last_auth_at FROM sessions WHERE user_id = ?1",
    )
    .bind(member.id.to_string())
    .fetch_one(f.stack.pools.reader().executor())
    .await
    .unwrap();
    assert_eq!(after, before);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM identity_links WHERE last_login_at IS NOT NULL")
            .await,
        0
    );

    f.stack.flush_audit().await;
    let created = f.audit_rows("IDENTITY_LINK_CREATED").await;
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["via"], "manual");
    assert_eq!(created[0]["provider_id"], f.provider_id("corp").await);
    assert_eq!(
        f.count("SELECT COUNT(*) FROM audit_events WHERE action = 'LOGIN_SUCCEEDED'")
            .await,
        logins_before,
        "linking never records a login"
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_manual_link_works_for_oauth2_without_a_verified_email() {
    let f = Federation::start().await;
    f.oauth2("plain", json!({})).await;
    let member = f.local_member("ada").await;
    let begun = f.begin_link("plain", &member.creds).await;
    f.idp.set_token_response(json!({
        "access_token": "oauth-access-token",
        "token_type": "bearer",
    }));
    f.idp.set_userinfo(mock_idp::missing_email_userinfo());
    let fetched = f.finish_as("plain", &begun, Some(&member.creds)).await;
    assert_link_redirect(&fetched);
    let row: (String, Option<String>, i64) =
        sqlx::query_as("SELECT subject, email_at_link, email_verified_at_link FROM identity_links")
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(row, ("external-subject".to_owned(), None, 0));
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_never_enters_login_resolution() {
    let f = Federation::start().await;
    f.oidc(
        "corp",
        json!({ "autoProvision": true, "allowEmailLinking": true }),
    )
    .await;
    let ada = f.local_member("ada").await;
    let victim = f.local_member("victim").await;
    f.stack
        .execute(&format!(
            "UPDATE users SET email = 'shared@example.test', email_normalized = 'shared@example.test'
              WHERE id = '{}'",
            victim.id
        ))
        .await;
    let users_before = f.count("SELECT COUNT(*) FROM users").await;
    let sessions_before = f.session_count().await;

    let fetched = f
        .link_oidc(
            "corp",
            &ada,
            json!({ "sub": "linked-to-ada", "email": "shared@example.test", "email_verified": true }),
            &[],
        )
        .await;
    assert_link_redirect(&fetched);
    let owner: String =
        sqlx::query_scalar("SELECT user_id FROM identity_links WHERE subject = 'linked-to-ada'")
            .fetch_one(f.stack.pools.reader().executor())
            .await
            .unwrap();
    assert_eq!(
        owner,
        ada.id.to_string(),
        "the e-mail never selects the account"
    );
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, users_before);
    assert_eq!(f.session_count().await, sessions_before);

    let begun = f.begin_link("corp", &victim.creds).await;
    f.arm_oidc(
        &begun,
        json!({ "sub": "brand-new", "email": "nobody@example.test" }),
        &[],
    );
    let provisioned = f.finish_as("corp", &begun, Some(&victim.creds)).await;
    assert_link_redirect(&provisioned);
    assert_eq!(f.count("SELECT COUNT(*) FROM users").await, users_before);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_refuses_subject_bound_elsewhere() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let owner = f.make_user("owner", "owner@example.test").await;
    f.link("corp", owner, "external-subject", "active").await;
    let member = f.local_member("ada").await;
    let links_before = f.links().await;
    let sessions_before = f.live_sessions().await;

    let fetched = f
        .link_oidc("corp", &member, json!({ "sub": "external-subject" }), &[])
        .await;
    assert_failure(&fetched, "PROVIDER_IDENTITY_ALREADY_LINKED");
    assert_eq!(f.links().await, links_before);
    assert_eq!(f.audit_rows("IDENTITY_LINK_CREATED").await.len(), 0);
    assert_eq!(f.live_sessions().await, sessions_before);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_same_subject_same_user_is_idempotent_and_a_different_subject_conflicts() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;

    let same = f.begin_link("corp", &member.creds).await;
    f.link("corp", member.id, "external-subject", "active")
        .await;
    f.arm_oidc(&same, json!({ "sub": "external-subject" }), &[]);
    let fetched = f.finish_as("corp", &same, Some(&member.creds)).await;
    assert_link_redirect(&fetched);
    assert_eq!(f.links().await.len(), 1);
    assert_eq!(f.audit_rows("IDENTITY_LINK_CREATED").await.len(), 0);

    f.stack.execute("DELETE FROM identity_links").await;
    let different = f.begin_link("corp", &member.creds).await;
    f.link("corp", member.id, "the-original", "active").await;
    f.arm_oidc(&different, json!({ "sub": "a-new-subject" }), &[]);
    let fetched = f.finish_as("corp", &different, Some(&member.creds)).await;
    assert_failure(&fetched, "PROVIDER_IDENTITY_ALREADY_LINKED");
    let links = f.links().await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].1, "the-original");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_fails_when_recent_auth_expires_during_the_round_trip() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;

    let begun = f.begin_link("corp", &member.creds).await;
    f.stack.clock.advance(Duration::from_secs(6 * 60));
    f.arm_oidc(&begun, json!({}), &[]);
    let before: String = sqlx::query_scalar("SELECT last_auth_at FROM sessions")
        .fetch_one(f.stack.pools.reader().executor())
        .await
        .unwrap();
    let fetched = f.finish_as("corp", &begun, Some(&member.creds)).await;
    assert_failure(&fetched, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(f.links().await.len(), 0);
    assert_eq!(f.audit_rows("IDENTITY_LINK_CREATED").await.len(), 0);
    let after: String = sqlx::query_scalar("SELECT last_auth_at FROM sessions")
        .fetch_one(f.stack.pools.reader().executor())
        .await
        .unwrap();
    assert_eq!(after, before, "the callback never extends the window");

    let replay = f.finish_as("corp", &begun, Some(&member.creds)).await;
    assert_failure(&replay, "PROVIDER_STATE_INVALID");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_audit_failure_rolls_back_the_link() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;
    f.stack
        .execute(
            "CREATE TRIGGER fail_manual_link_audit BEFORE INSERT ON audit_events
             WHEN NEW.action = 'IDENTITY_LINK_CREATED'
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    let fetched = f
        .link_oidc("corp", &member, json!({ "sub": "rolled-back" }), &[])
        .await;
    assert_failure(&fetched, "INTERNAL_ERROR");
    assert_eq!(f.links().await.len(), 0);
    assert_eq!(f.audit_rows("IDENTITY_LINK_CREATED").await.len(), 0);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_surfaces_provider_failures_as_stable_codes_without_linking() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;

    let denied = f.begin_link("corp", &member.creds).await;
    let fetched = f
        .callback(
            "corp",
            &format!("error=access_denied&state={}", denied.state),
            Some(&denied.binding),
        )
        .await;
    assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

    let invalid = f.begin_link("corp", &member.creds).await;
    f.arm_oidc(&invalid, json!({ "nonce": "forged" }), &[]);
    let fetched = f.finish_as("corp", &invalid, Some(&member.creds)).await;
    assert_failure(&fetched, "PROVIDER_ID_TOKEN_INVALID");

    let missing = f.begin_link("corp", &member.creds).await;
    f.arm_oidc(&missing, json!({}), &["sub"]);
    let fetched = f.finish_as("corp", &missing, Some(&member.creds)).await;
    assert_failure(&fetched, "PROVIDER_SUBJECT_MISSING");

    assert_eq!(f.links().await.len(), 0);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_link_callback_clears_the_binding_cookie_on_every_terminal_outcome() {
    let f = Federation::start().await;
    f.oidc("corp", json!({})).await;
    let member = f.local_member("ada").await;

    let wrong_binding = f.begin_link("corp", &member.creds).await;
    let tampered = Begun {
        state: wrong_binding.state.clone(),
        binding: Token::mint().unwrap().encode().expose_secret().clone(),
        nonce: wrong_binding.nonce.clone(),
    };
    let fetched = f.finish_as("corp", &tampered, Some(&member.creds)).await;
    assert_failure(&fetched, "PROVIDER_STATE_INVALID");

    let success = f
        .link_oidc("corp", &member, json!({ "sub": "cookie-subject" }), &[])
        .await;
    assert_link_redirect(&success);
    f.stack.stop().await;
}
