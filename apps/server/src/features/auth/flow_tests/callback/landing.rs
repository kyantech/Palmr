use super::link::Member;
use super::reauth::assert_reauth_redirect;
use super::*;

const UNCONSUMED: &str = "SELECT COUNT(*) FROM oauth_auth_requests WHERE consumed_at IS NULL";
const PROVIDER_PROSE: &str = "Upstream%20said%20your%20cat%20is%20unwell";

#[derive(Clone, Copy)]
enum Flow {
    Login,
    Link,
    Reauth,
}

impl Flow {
    const ALL: [Self; 3] = [Self::Login, Self::Link, Self::Reauth];

    const fn landing(self) -> Landing {
        match self {
            Self::Login => Landing::Login,
            Self::Link => Landing::Link,
            Self::Reauth => Landing::Reauth,
        }
    }
}

struct Actors {
    local: Member,
    external: Member,
}

impl Federation {
    async fn actors(&self) -> Actors {
        self.oidc("corp", json!({ "autoProvision": true })).await;
        Actors {
            local: self.local_member("ada").await,
            external: self
                .external_oidc_member("corp", "sso-subject", "sso@example.test")
                .await,
        }
    }

    async fn begin_flow(&self, flow: Flow, actors: &Actors) -> Begun {
        match flow {
            Flow::Login => self.begin("corp", None).await,
            Flow::Link => self.begin_link("corp", &actors.local.creds).await,
            Flow::Reauth => self.begin_reauth(&actors.external.creds).await,
        }
    }

    async fn finish_flow(&self, flow: Flow, begun: &Begun, actors: &Actors) -> Fetched {
        match flow {
            Flow::Login => self.finish("corp", begun).await,
            Flow::Link => {
                self.finish_as("corp", begun, Some(&actors.local.creds))
                    .await
            }
            Flow::Reauth => {
                self.finish_as("corp", begun, Some(&actors.external.creds))
                    .await
            }
        }
    }

    async fn deny(&self, slug: &str, begun: &Begun, cookie: Option<&str>) -> Fetched {
        self.callback(
            slug,
            &format!(
                "error=access_denied&error_description={PROVIDER_PROSE}&state={}",
                begun.state
            ),
            cookie,
        )
        .await
    }
}

fn assert_nothing_forwarded(fetched: &Fetched, begun: &Begun) {
    let target = location(fetched);
    for secret in [
        begun.state.as_str(),
        begun.binding.as_str(),
        begun.nonce.as_str(),
        "auth-code",
        "access-token",
        "Upstream",
        "unwell",
        "access_denied",
        "sso-subject",
        "example.test/",
        "@",
    ] {
        assert!(
            !target
                .replacen("https://files.example.test/", "", 1)
                .contains(secret),
            "{secret} leaked into {target}"
        );
    }
}

#[tokio::test]
async fn it_callback_failure_redirect_carries_the_callback_request_id() {
    let f = Federation::start().await;
    let actors = f.actors().await;

    let denied = f.begin_flow(Flow::Login, &actors).await;
    let fetched = f.deny("corp", &denied, Some(&denied.binding)).await;
    let request_id = assert_failure(&fetched, "PROVIDER_AUTH_DENIED");
    assert_nothing_forwarded(&fetched, &denied);

    let failed = f.begin_flow(Flow::Login, &actors).await;
    f.idp.set_token_status(
        400,
        json!({ "error": "invalid_grant", "error_description": "Upstream" }),
    );
    let fetched = f.finish("corp", &failed).await;
    let second = assert_failure(&fetched, "PROVIDER_CODE_EXCHANGE_FAILED");
    assert_nothing_forwarded(&fetched, &failed);
    assert_ne!(request_id, second, "each callback request has its own id");

    let unknown = f
        .callback("corp", "code=auth-code&state=not-a-state", None)
        .await;
    let third = assert_failure(&unknown, "PROVIDER_STATE_INVALID");
    assert_ne!(third, second);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_failure_landing_follows_the_stored_purpose() {
    let f = Federation::start().await;
    let actors = f.actors().await;
    let before = f.session_facts(&actors.external.creds.session).await;

    for flow in Flow::ALL {
        let landing = flow.landing();

        let begun = f.begin_flow(flow, &actors).await;
        let fetched = f.deny("corp", &begun, Some(&begun.binding)).await;
        assert_landing(&fetched, landing, "PROVIDER_AUTH_DENIED");
        assert_nothing_forwarded(&fetched, &begun);

        let begun = f.begin_flow(flow, &actors).await;
        f.idp
            .set_token_status(400, json!({ "error": "invalid_grant" }));
        let fetched = f.finish_flow(flow, &begun, &actors).await;
        assert_landing(&fetched, landing, "PROVIDER_CODE_EXCHANGE_FAILED");
        assert_nothing_forwarded(&fetched, &begun);

        let begun = f.begin_flow(flow, &actors).await;
        f.arm_oidc(&begun, json!({ "nonce": "forged-nonce-value" }), &[]);
        let fetched = f.finish_flow(flow, &begun, &actors).await;
        assert_landing(&fetched, landing, "PROVIDER_ID_TOKEN_INVALID");
        assert_nothing_forwarded(&fetched, &begun);

        let begun = f.begin_flow(flow, &actors).await;
        f.arm_oidc(&begun, json!({}), &["sub"]);
        let fetched = f.finish_flow(flow, &begun, &actors).await;
        assert_landing(&fetched, landing, "PROVIDER_SUBJECT_MISSING");
    }

    assert_eq!(
        f.session_facts(&actors.external.creds.session).await,
        before
    );
    assert_eq!(f.links().await.len(), 1);
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_purpose_is_never_taken_from_the_request() {
    let f = Federation::start().await;
    let actors = f.actors().await;

    let login = f.begin_flow(Flow::Login, &actors).await;
    let fetched = f
        .callback(
            "corp",
            &format!("error=access_denied&purpose=reauth&state={}", login.state),
            Some(&login.binding),
        )
        .await;
    assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

    let reauth = f.begin_flow(Flow::Reauth, &actors).await;
    let fetched = f
        .callback(
            "corp",
            &format!(
                "error=access_denied&purpose=login&return_to=%2Foverview&status=success&state={}",
                reauth.state
            ),
            Some(&reauth.binding),
        )
        .await;
    assert_reauth_failure(&fetched, "PROVIDER_AUTH_DENIED");

    let link = f.begin_flow(Flow::Link, &actors).await;
    let fetched = f
        .callback(
            "corp",
            &format!("code=auth-code&purpose=login&state={}", link.state),
            Some(&link.binding),
        )
        .await;
    assert!(
        matches!(
            location(&fetched).as_str(),
            target if target.starts_with("https://files.example.test/settings/security?error=")
        ),
        "{}",
        location(&fetched)
    );
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_purpose_falls_back_to_login_when_it_cannot_be_proven() {
    let f = Federation::start().await;
    let actors = f.actors().await;
    f.oidc("other", json!({})).await;

    for flow in [Flow::Link, Flow::Reauth] {
        let begun = f.begin_flow(flow, &actors).await;
        let unconsumed = f.count(UNCONSUMED).await;

        let fetched = f.deny("corp", &begun, None).await;
        assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

        let stranger = Token::mint().unwrap().encode().expose_secret().clone();
        let fetched = f.deny("corp", &begun, Some(&stranger)).await;
        assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

        let fetched = f.deny("other", &begun, Some(&begun.binding)).await;
        assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

        let unknown = Begun {
            state: Token::mint().unwrap().encode().expose_secret().clone(),
            binding: begun.binding.clone(),
            nonce: begun.nonce.clone(),
        };
        let fetched = f.deny("corp", &unknown, Some(&begun.binding)).await;
        assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

        let fetched = f
            .callback(
                "corp",
                "error=access_denied&state=%00%00&state=again",
                Some(&begun.binding),
            )
            .await;
        assert_failure(&fetched, "PROVIDER_AUTH_DENIED");

        assert_eq!(
            f.count(UNCONSUMED).await,
            unconsumed,
            "a denial lookup consumes nothing"
        );
        let fetched = f.deny("corp", &begun, Some(&begun.binding)).await;
        assert_landing(&fetched, flow.landing(), "PROVIDER_AUTH_DENIED");
        assert_eq!(f.count(UNCONSUMED).await, unconsumed);

        let mismatched = f
            .callback(
                "other",
                &format!("code=auth-code&state={}", begun.state),
                Some(&begun.binding),
            )
            .await;
        assert_failure(&mismatched, "PROVIDER_STATE_INVALID");
        let replay = f.finish_flow(flow, &begun, &actors).await;
        assert_failure(&replay, "PROVIDER_STATE_INVALID");
    }

    let expired = f.begin_flow(Flow::Link, &actors).await;
    f.stack.clock.advance(Duration::from_secs(11 * 60));
    let fetched = f.deny("corp", &expired, Some(&expired.binding)).await;
    assert_failure(&fetched, "PROVIDER_AUTH_DENIED");
    f.stack.stop().await;
}

#[tokio::test]
async fn it_callback_success_landing_follows_the_stored_purpose() {
    let f = Federation::start().await;
    let actors = f.actors().await;

    let begun = f.begin("corp", Some("/settings/security")).await;
    f.arm_oidc(
        &begun,
        json!({ "sub": "sso-subject", "email": "sso@example.test", "email_verified": true }),
        &[],
    );
    let fetched = f.finish("corp", &begun).await;
    assert_signed_in(&fetched, "/settings/security");

    let begun = f.begin("corp", None).await;
    f.arm_oidc(
        &begun,
        json!({ "sub": "sso-subject", "email": "sso@example.test", "email_verified": true }),
        &[],
    );
    let fetched = f.finish("corp", &begun).await;
    assert_signed_in(&fetched, "/overview");

    let begun = f.begin_flow(Flow::Link, &actors).await;
    f.arm_oidc(&begun, json!({ "sub": "linked-subject" }), &[]);
    let fetched = f.finish_flow(Flow::Link, &begun, &actors).await;
    link::assert_link_redirect(&fetched);
    assert_eq!(f.links().await.len(), 2);

    let before = f.session_facts(&actors.external.creds.session).await;
    f.stack.clock.advance(Duration::from_secs(90));
    let begun = f.begin_flow(Flow::Reauth, &actors).await;
    f.arm_oidc(&begun, json!({ "sub": "sso-subject" }), &[]);
    let fetched = f.finish_flow(Flow::Reauth, &begun, &actors).await;
    assert_reauth_redirect(&fetched);
    let after = f.session_facts(&actors.external.creds.session).await;
    assert_ne!(after.last_auth_at, before.last_auth_at);
    assert_eq!(after.token_hash, before.token_hash);
    f.stack.stop().await;
}
