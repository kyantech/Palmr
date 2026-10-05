use super::profile::{assert_code, Call};
use super::*;
use crate::features::settings::model::AppSettings;

pub(super) const SETTINGS: &str = "/api/v1/admin/settings";
const EFFECTIVE: &str = "/api/v1/settings/effective";
const PROFILE: &str = "/api/v1/profile";
const REAUTH: &str = "/api/v1/auth/reauthenticate";
const NEW_PASSWORD: &str = "a brand new passphrase";
const MAX_SAFE: i64 = (1 << 53) - 1;
const AUDIT_ACTIONS: &str = "'SETTING_CHANGED', 'SECURITY_POLICY_CHANGED', 'MANDATORY_2FA_POLICY_CHANGED', 'SMTP_SETTINGS_CHANGED'";

pub(super) type AuditRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    String,
);

const STORED_COUNT: i64 = u32::MAX as i64;

const BOUNDS: [(&str, &str, i64, i64); 13] = [
    ("security", "passwordMinLength", 8, STORED_COUNT),
    ("security", "publicLinkPasswordMinLength", 8, STORED_COUNT),
    ("security", "maxLoginAttempts", 3, STORED_COUNT),
    ("security", "loginLockoutMinutes", 1, STORED_COUNT),
    ("security", "sessionIdleDays", 1, 30),
    ("security", "sessionAbsoluteDays", 1, 90),
    ("security", "recentAuthMinutes", 1, 15),
    ("security", "passwordResetValidityMinutes", 5, 1440),
    ("security", "inviteValidityHours", 1, 720),
    ("security", "trustedDeviceDurationDays", 1, 365),
    ("quotas", "defaultUserQuotaBytes", 0, MAX_SAFE),
    ("quotas", "maxFileSizeBytes", 0, MAX_SAFE),
    ("public-links", "maxPublicLinkLifetimeDays", 1, STORED_COUNT),
];

impl Stack {
    pub(super) async fn settings_call(
        &self,
        method: Method,
        group: Option<&str>,
        creds: &Credentials,
        body: Option<&Value>,
        host: u8,
    ) -> Fetched {
        self.clock.advance(Duration::from_secs(1));
        let path = group.map_or_else(
            || SETTINGS.to_owned(),
            |group| format!("{SETTINGS}/{group}"),
        );
        let mut call = Call::new(method, &path, creds);
        if let Some(body) = body {
            call = call.json(body);
        }
        self.call(call, host).await
    }

    pub(super) async fn read_settings(&self, group: &str, creds: &Credentials) -> Value {
        let fetched = self
            .settings_call(Method::GET, Some(group), creds, None, 10)
            .await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    pub(super) async fn patch_settings(
        &self,
        group: &str,
        creds: &Credentials,
        body: &Value,
    ) -> Fetched {
        self.settings_call(Method::PATCH, Some(group), creds, Some(body), 10)
            .await
    }

    pub(super) async fn patch_ok(&self, group: &str, creds: &Credentials, body: &Value) -> Value {
        let fetched = self.patch_settings(group, creds, body).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    pub(super) async fn effective(&self, creds: &Credentials) -> Value {
        let fetched = self.get(EFFECTIVE, Some(&creds.session), 11).await;
        assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        fetched.json()
    }

    pub(super) async fn stored_setting(
        &self,
        key: &str,
    ) -> Option<(String, Option<String>, String)> {
        sqlx::query_as("SELECT value_json, updated_by, updated_at FROM app_settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(self.pools.reader().executor())
            .await
            .unwrap()
    }

    pub(super) async fn all_stored_settings(&self) -> Vec<(String, String, String)> {
        sqlx::query_as(
            "SELECT key, COALESCE(value_json, ''), updated_at FROM app_settings ORDER BY key",
        )
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn settings_audit(&self) -> Vec<AuditRow> {
        sqlx::query_as(&format!(
            "SELECT action, actor_type, actor_user_id, target_type, target_id, target_label,
                    result, metadata_json
               FROM audit_events WHERE action IN ({AUDIT_ACTIONS}) ORDER BY id"
        ))
        .fetch_all(self.pools.reader().executor())
        .await
        .unwrap()
    }

    pub(super) async fn settings_admin(&self) -> (Credentials, String) {
        let creds = self.operator(10).await;
        let id = self.operator_id("root").await;
        (creds, id)
    }

    pub(super) async fn settings_user(&self, username: &str, host: u8) -> Credentials {
        let hash = password_hash();
        self.user(UserSpec::local(
            username,
            &format!("{username}@example.test"),
            &hash,
        ))
        .await;
        self.signed_in(username, host).await
    }

    pub(super) async fn reauth_with_password(&self, creds: &Credentials, host: u8) -> Fetched {
        let body = json!({ "password": PASSWORD, "totpCode": null });
        self.call(Call::new(Method::POST, REAUTH, creds).json(&body), host)
            .await
    }
}

pub(super) fn assert_detail(fetched: &Fetched, key: &str, value: &Value) {
    assert_eq!(
        &fetched.json()["error"]["details"][key],
        value,
        "{}",
        fetched.text()
    );
}

#[tokio::test]
async fn it_settings_admin_get_and_patch_round_trip() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;

    let all = stack
        .settings_call(Method::GET, None, &admin, None, 10)
        .await;
    assert_eq!(all.status, StatusCode::OK, "{}", all.text());
    assert_eq!(
        all.headers.get("cache-control").unwrap().to_str().unwrap(),
        "no-store"
    );
    assert_eq!(
        all.json(),
        json!({
            "general": {
                "appName": "Palmr",
                "appDescription": "Self-hosted file transfer",
                "defaultLocale": "en-US",
                "hideVersion": false,
                "poweredByVisible": true,
                "thumbnailSourceLimit": "64MiB",
            },
            "security": {
                "passwordMinLength": 8,
                "publicLinkPasswordMinLength": 8,
                "maxLoginAttempts": 5,
                "loginLockoutMinutes": 10,
                "sessionIdleDays": 7,
                "sessionAbsoluteDays": 30,
                "recentAuthMinutes": 5,
                "passwordResetValidityMinutes": 60,
                "inviteValidityHours": 24,
                "twoFactorRequired": false,
                "trustedDevicesEnabled": true,
                "trustedDeviceDurationDays": 30,
                "authProvidersEnabled": true,
            },
            "quotas": { "defaultUserQuotaBytes": null, "maxFileSizeBytes": null },
            "public-links": { "maxPublicLinkLifetimeDays": null },
            "smtp": {
                "enabled": false,
                "host": null,
                "port": 587,
                "security": "starttls",
                "username": null,
                "passwordConfigured": false,
                "fromName": null,
                "fromEmail": null,
                "allowSelfSignedCertificate": false,
                "noAuth": false,
            },
        })
    );

    let general = stack
        .patch_ok(
            "general",
            &admin,
            &json!({
                "appName": "  Nova Files  ",
                "appDescription": "",
                "defaultLocale": "pt-BR",
                "hideVersion": true,
                "poweredByVisible": false,
                "thumbnailSourceLimit": "unlimited",
            }),
        )
        .await;
    assert_eq!(
        general,
        json!({
            "appName": "Nova Files",
            "appDescription": "",
            "defaultLocale": "pt-BR",
            "hideVersion": true,
            "poweredByVisible": false,
            "thumbnailSourceLimit": "unlimited",
        })
    );
    assert_eq!(stack.read_settings("general", &admin).await, general);
    let settings = stack.settings.current();
    assert_eq!(settings.app_name(), "Nova Files");
    assert!(!settings.general.show_version);
    assert!(!settings.general.powered_by_visible);

    let security = stack
        .patch_ok(
            "security",
            &admin,
            &json!({
                "passwordMinLength": 12,
                "trustedDevicesEnabled": false,
                "trustedDeviceDurationDays": 90,
            }),
        )
        .await;
    assert_eq!(security["passwordMinLength"], 12);
    assert_eq!(security["trustedDevicesEnabled"], false);
    assert_eq!(security["trustedDeviceDurationDays"], 90);
    assert_eq!(security["maxLoginAttempts"], 5);

    let quotas = stack
        .patch_ok(
            "quotas",
            &admin,
            &json!({ "defaultUserQuotaBytes": 5_368_709_120_i64, "maxFileSizeBytes": 1_048_576 }),
        )
        .await;
    assert_eq!(
        quotas,
        json!({ "defaultUserQuotaBytes": 5_368_709_120_i64, "maxFileSizeBytes": 1_048_576 })
    );
    let links = stack
        .patch_ok(
            "public-links",
            &admin,
            &json!({ "maxPublicLinkLifetimeDays": 30 }),
        )
        .await;
    assert_eq!(links, json!({ "maxPublicLinkLifetimeDays": 30 }));

    let all = stack
        .settings_call(Method::GET, None, &admin, None, 10)
        .await
        .json();
    assert_eq!(all["general"], general);
    assert_eq!(all["security"], security);
    assert_eq!(all["quotas"], quotas);
    assert_eq!(all["public-links"], links);

    let reconstructed = SettingsService::load(
        &stack.pools,
        Arc::new(clock.clone()),
        &InstanceKey::load_or_create(root.path()).unwrap().0,
    )
    .await
    .unwrap();
    assert_eq!(reconstructed.current().app_name(), "Nova Files");
    assert_eq!(reconstructed.current().security.password_min_length, 12);
    assert_eq!(
        reconstructed
            .current()
            .quotas
            .max_file_size_bytes
            .map(|size| size.get()),
        Some(1_048_576)
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_normal_user_is_forbidden() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let ada = stack.settings_user("ada", 11).await;
    let before = stack.all_stored_settings().await;

    for group in [
        None,
        Some("general"),
        Some("security"),
        Some("quotas"),
        Some("public-links"),
    ] {
        let read = stack
            .settings_call(Method::GET, group, &ada, None, 11)
            .await;
        assert_code(&read, StatusCode::FORBIDDEN, "FORBIDDEN");
        if let Some(group) = group {
            let body = json!({});
            let write = stack
                .settings_call(Method::PATCH, Some(group), &ada, Some(&body), 11)
                .await;
            assert_code(&write, StatusCode::FORBIDDEN, "FORBIDDEN");
            let body = json!({ "appName": "Hijack", "passwordMinLength": 8 });
            let write = stack
                .settings_call(Method::PATCH, Some(group), &ada, Some(&body), 11)
                .await;
            assert_code(&write, StatusCode::FORBIDDEN, "FORBIDDEN");
        }
    }
    assert_eq!(stack.all_stored_settings().await, before);
    assert_eq!(stack.scalar_i64("SELECT COUNT(*) FROM audit_events WHERE action LIKE '%POLICY%' OR action = 'SETTING_CHANGED'").await, 0);
    assert_eq!(
        stack.read_settings("general", &admin).await["appName"],
        "Palmr"
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_security_patch_requires_recent_auth() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;

    clock.advance(Duration::from_secs(6 * 60));
    let before = stack.all_stored_settings().await;
    let stale = stack
        .patch_settings("security", &admin, &json!({ "passwordMinLength": 12 }))
        .await;
    assert_code(&stale, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(stack.all_stored_settings().await, before);
    assert_eq!(stack.settings.current().security.password_min_length, 8);
    let read = stack
        .settings_call(Method::GET, Some("security"), &admin, None, 10)
        .await;
    assert_eq!(read.status, StatusCode::OK, "{}", read.text());

    for (group, body) in [
        ("general", json!({ "appName": "Still Fine" })),
        ("quotas", json!({ "maxFileSizeBytes": 10 })),
        ("public-links", json!({ "maxPublicLinkLifetimeDays": 5 })),
    ] {
        stack.patch_ok(group, &admin, &body).await;
    }

    let reauth = stack.reauth_with_password(&admin, 10).await;
    assert_eq!(reauth.status, StatusCode::NO_CONTENT, "{}", reauth.text());
    let fresh = stack
        .patch_ok("security", &admin, &json!({ "passwordMinLength": 12 }))
        .await;
    assert_eq!(fresh["passwordMinLength"], 12);
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_unknown_group_key_and_body_shape() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let before = stack.all_stored_settings().await;

    for group in [
        "nope",
        "branding",
        "retention",
        "audit",
        "storage",
        "public_links",
    ] {
        let read = stack
            .settings_call(Method::GET, Some(group), &admin, None, 10)
            .await;
        assert_eq!(
            read.status,
            StatusCode::NOT_FOUND,
            "{group}: {}",
            read.text()
        );
        let write = stack.patch_settings(group, &admin, &json!({})).await;
        assert_eq!(write.status, StatusCode::NOT_FOUND, "{group}");
    }

    let cases: [(&str, Value); 14] = [
        ("general", json!({ "bogus": 1 })),
        ("general", json!({ "app_name": "snake" })),
        ("general", json!({ "appName": "Valid", "bogus": true })),
        ("general", json!({ "showVersion": false })),
        ("general", json!({ "poweredByText": "Custom" })),
        ("general", json!({ "maxFileSizeBytes": 1 })),
        ("security", json!({ "defaultUserQuotaBytes": 1 })),
        ("quotas", json!({ "passwordMinLength": 12 })),
        (
            "public-links",
            json!({ "maxPublicLinkLifetimeDays": 5, "extra": 1 }),
        ),
        ("general", json!({ "port": 8080 })),
        ("general", json!({ "baseUrl": "https://evil.example" })),
        ("general", json!({ "trustProxy": "off" })),
        ("quotas", json!({ "storageProvider": "s3" })),
        (
            "security",
            json!({ "s3Bucket": "x", "dataDir": "/tmp", "bindAddress": "0.0.0.0" }),
        ),
    ];
    for (group, body) in cases {
        let rejected = stack.patch_settings(group, &admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_UNKNOWN",
        );
        assert_eq!(rejected.json()["error"]["details"], json!({}), "{body}");
    }
    assert_eq!(stack.all_stored_settings().await, before);
    assert_eq!(stack.settings.current().app_name(), "Palmr");

    for body in [json!([]), json!("appName"), json!(7), json!(null)] {
        let rejected = stack.patch_settings("general", &admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "VALIDATION_ERROR",
        );
        assert_detail(&rejected, "fields", &json!(["body"]));
    }
    let call = Call::new(Method::PATCH, "/api/v1/admin/settings/general", &admin);
    let broken = Call {
        body: Some("{\"appName\"".to_owned()),
        ..call
    };
    assert_code(
        &stack.call(broken, 10).await,
        StatusCode::BAD_REQUEST,
        "INVALID_JSON",
    );
    assert_eq!(stack.all_stored_settings().await, before);
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_wrong_json_types_and_invalid_values() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let before = stack.all_stored_settings().await;
    let long_name = "n".repeat(101);
    let long_description = "d".repeat(301);

    let cases: Vec<(&str, &str, Value)> = vec![
        ("general", "appName", json!(7)),
        ("general", "appName", json!(true)),
        ("general", "appName", json!("")),
        ("general", "appName", json!("   ")),
        ("general", "appName", json!(long_name)),
        ("general", "appName", json!("bell\u{7}")),
        ("general", "appDescription", json!(7)),
        ("general", "appDescription", json!(long_description)),
        ("general", "appDescription", json!("line\nbreak")),
        ("general", "defaultLocale", json!("xx-XX")),
        ("general", "defaultLocale", json!("en-us")),
        ("general", "defaultLocale", json!(3)),
        ("general", "hideVersion", json!("yes")),
        ("general", "hideVersion", json!(1)),
        ("general", "poweredByVisible", json!("true")),
        ("general", "thumbnailSourceLimit", json!("32MiB")),
        ("general", "thumbnailSourceLimit", json!(64)),
        ("security", "passwordMinLength", json!("12")),
        ("security", "passwordMinLength", json!(12.5)),
        ("security", "passwordMinLength", json!(12.0)),
        ("security", "passwordMinLength", json!(true)),
        ("security", "passwordMinLength", json!(u64::MAX)),
        ("security", "twoFactorRequired", json!("true")),
        ("security", "twoFactorRequired", json!(1)),
        ("security", "trustedDevicesEnabled", json!([])),
        ("quotas", "defaultUserQuotaBytes", json!("1024")),
        ("quotas", "maxFileSizeBytes", json!(1.5)),
        ("quotas", "maxFileSizeBytes", json!(false)),
        ("public-links", "maxPublicLinkLifetimeDays", json!("30")),
        ("public-links", "maxPublicLinkLifetimeDays", json!({})),
    ];
    for (group, key, value) in cases {
        let body = json!({ key: value });
        let rejected = stack.patch_settings(group, &admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_VALUE_INVALID",
        );
        assert_detail(&rejected, "key", &json!(key));
        assert!(
            rejected.json()["error"]["details"].get("floor").is_none(),
            "{body}"
        );
    }
    assert_eq!(stack.all_stored_settings().await, before);

    let mixed = stack
        .patch_settings(
            "general",
            &admin,
            &json!({ "appName": "Applied?", "defaultLocale": "xx-XX" }),
        )
        .await;
    assert_code(
        &mixed,
        StatusCode::UNPROCESSABLE_ENTITY,
        "SETTING_VALUE_INVALID",
    );
    assert_eq!(stack.all_stored_settings().await, before);
    assert_eq!(stack.settings.current().app_name(), "Palmr");
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_accept_values_above_former_arbitrary_ceilings() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;

    for (group, key, values) in [
        ("security", "passwordMinLength", [129_i64, 1_000, 100_000]),
        (
            "security",
            "publicLinkPasswordMinLength",
            [129, 1_000, 100_000],
        ),
        ("security", "maxLoginAttempts", [101, 1_000, 100_000]),
        (
            "security",
            "loginLockoutMinutes",
            [10_081, 100_000, 1_000_000],
        ),
        (
            "public-links",
            "maxPublicLinkLifetimeDays",
            [3651, 36_500, 1_000_000],
        ),
    ] {
        for value in values {
            let applied = stack.patch_ok(group, &admin, &json!({ key: value })).await;
            assert_eq!(applied[key], value, "{key}");
            assert_eq!(
                stack.read_settings(group, &admin).await[key],
                value,
                "{key}"
            );
        }
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_floor_enforced_per_key() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;

    for (group, key, floor, ceiling) in BOUNDS {
        let before = stack.all_stored_settings().await;

        let below = stack
            .patch_settings(group, &admin, &json!({ key: floor - 1 }))
            .await;
        assert_code(
            &below,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_BELOW_FLOOR",
        );
        assert_detail(&below, "floor", &json!(floor));
        assert_detail(&below, "key", &json!(key));

        let above = stack
            .patch_settings(group, &admin, &json!({ key: ceiling + 1 }))
            .await;
        assert_code(
            &above,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_VALUE_INVALID",
        );
        assert_detail(&above, "max", &json!(ceiling));
        assert_detail(&above, "key", &json!(key));

        let far_below = stack
            .patch_settings(group, &admin, &json!({ key: i64::MIN }))
            .await;
        assert_code(
            &far_below,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_BELOW_FLOOR",
        );
        assert_eq!(stack.all_stored_settings().await, before, "{key}");

        let at_floor = stack.patch_ok(group, &admin, &json!({ key: floor })).await;
        assert_eq!(at_floor[key], floor, "{key}");
        let at_ceiling = stack
            .patch_ok(group, &admin, &json!({ key: ceiling }))
            .await;
        assert_eq!(at_ceiling[key], ceiling, "{key}");
        assert_eq!(
            stack.read_settings(group, &admin).await[key],
            ceiling,
            "{key}"
        );
    }

    let security = stack.settings.current();
    assert_eq!(security.security.password_min_length, u32::MAX);
    assert_eq!(security.security.max_login_attempts, u32::MAX);
    assert_eq!(security.security.login_lockout_minutes, u32::MAX);
    assert_eq!(security.security.session_absolute_days, 90);
    assert_eq!(
        security.public_links.max_public_link_lifetime_days,
        Some(u32::MAX)
    );
    assert_eq!(
        security
            .quotas
            .default_user_quota_bytes
            .map(|size| size.get()),
        Some(u64::try_from(MAX_SAFE).unwrap())
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_patch_absent_vs_null() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;

    stack
        .patch_ok(
            "quotas",
            &admin,
            &json!({ "defaultUserQuotaBytes": 1000, "maxFileSizeBytes": 500 }),
        )
        .await;
    stack
        .patch_ok(
            "public-links",
            &admin,
            &json!({ "maxPublicLinkLifetimeDays": 14 }),
        )
        .await;

    let absent = stack
        .patch_ok("quotas", &admin, &json!({ "maxFileSizeBytes": 600 }))
        .await;
    assert_eq!(
        absent,
        json!({ "defaultUserQuotaBytes": 1000, "maxFileSizeBytes": 600 })
    );
    let empty = stack.patch_ok("quotas", &admin, &json!({})).await;
    assert_eq!(empty, absent);

    let audits_before = stack.settings_audit().await.len();
    let null_one = stack
        .patch_ok("quotas", &admin, &json!({ "defaultUserQuotaBytes": null }))
        .await;
    assert_eq!(
        null_one,
        json!({ "defaultUserQuotaBytes": null, "maxFileSizeBytes": 600 })
    );
    assert_eq!(
        stack
            .stored_setting("default_user_quota_bytes")
            .await
            .unwrap()
            .0,
        "null"
    );
    assert_eq!(
        stack.stored_setting("max_file_size_bytes").await.unwrap().0,
        "600"
    );
    assert_eq!(
        stack.settings.current().quotas.default_user_quota_bytes,
        None
    );
    assert_eq!(stack.settings_audit().await.len(), audits_before + 1);

    let null_both = stack
        .patch_ok(
            "quotas",
            &admin,
            &json!({ "defaultUserQuotaBytes": null, "maxFileSizeBytes": null }),
        )
        .await;
    assert_eq!(
        null_both,
        json!({ "defaultUserQuotaBytes": null, "maxFileSizeBytes": null })
    );
    assert_eq!(
        stack.stored_setting("max_file_size_bytes").await.unwrap().0,
        "null"
    );
    assert_eq!(stack.settings.current().quotas.max_file_size_bytes, None);

    let links = stack
        .patch_ok(
            "public-links",
            &admin,
            &json!({ "maxPublicLinkLifetimeDays": null }),
        )
        .await;
    assert_eq!(links, json!({ "maxPublicLinkLifetimeDays": null }));
    assert_eq!(
        stack
            .stored_setting("max_public_link_lifetime_days")
            .await
            .unwrap()
            .0,
        "null"
    );
    assert_eq!(
        stack
            .settings
            .current()
            .public_links
            .max_public_link_lifetime_days,
        None
    );

    let reconstructed = SettingsService::load(
        &stack.pools,
        Arc::new(stack.clock.clone()),
        &InstanceKey::load_or_create(root.path()).unwrap().0,
    )
    .await
    .unwrap();
    assert_eq!(
        reconstructed.current().quotas.default_user_quota_bytes,
        None
    );
    assert_eq!(
        reconstructed
            .current()
            .public_links
            .max_public_link_lifetime_days,
        None
    );

    stack
        .patch_ok("quotas", &admin, &json!({ "maxFileSizeBytes": 0 }))
        .await;
    assert_eq!(
        stack.read_settings("quotas", &admin).await["maxFileSizeBytes"],
        0
    );

    let before = stack.all_stored_settings().await;
    for (group, key) in [
        ("general", "appName"),
        ("general", "appDescription"),
        ("general", "defaultLocale"),
        ("general", "hideVersion"),
        ("general", "poweredByVisible"),
        ("general", "thumbnailSourceLimit"),
        ("security", "passwordMinLength"),
        ("security", "publicLinkPasswordMinLength"),
        ("security", "maxLoginAttempts"),
        ("security", "loginLockoutMinutes"),
        ("security", "sessionIdleDays"),
        ("security", "sessionAbsoluteDays"),
        ("security", "recentAuthMinutes"),
        ("security", "passwordResetValidityMinutes"),
        ("security", "inviteValidityHours"),
        ("security", "twoFactorRequired"),
        ("security", "trustedDevicesEnabled"),
        ("security", "trustedDeviceDurationDays"),
        ("security", "authProvidersEnabled"),
    ] {
        let rejected = stack
            .patch_settings(group, &admin, &json!({ key: null }))
            .await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_VALUE_INVALID",
        );
        assert_detail(&rejected, "key", &json!(key));
    }
    assert_eq!(stack.all_stored_settings().await, before);
    let defaults = AppSettings::defaults();
    assert_eq!(
        stack.settings.current().general.app_name,
        defaults.general.app_name
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_change_audited_in_tx() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, admin_id) = stack.settings_admin().await;
    assert!(stack.settings_audit().await.is_empty());

    stack
        .patch_ok(
            "general",
            &admin,
            &json!({ "appName": "Nova", "hideVersion": true, "poweredByVisible": true }),
        )
        .await;
    stack
        .patch_ok(
            "quotas",
            &admin,
            &json!({ "defaultUserQuotaBytes": 2048, "maxFileSizeBytes": null }),
        )
        .await;
    stack
        .patch_ok(
            "public-links",
            &admin,
            &json!({ "maxPublicLinkLifetimeDays": 30 }),
        )
        .await;
    stack
        .patch_ok("quotas", &admin, &json!({ "defaultUserQuotaBytes": null }))
        .await;

    stack
        .patch_ok(
            "security",
            &admin,
            &json!({
                "twoFactorRequired": true,
                "passwordMinLength": 12,
                "sessionIdleDays": 7,
            }),
        )
        .await;

    let rows = stack.settings_audit().await;
    let summary: Vec<(String, String, Value)> = rows
        .iter()
        .map(|row| {
            (
                row.0.clone(),
                row.4.clone().unwrap(),
                serde_json::from_str(&row.7).unwrap(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        vec![
            (
                "SETTING_CHANGED".to_owned(),
                "app_name".to_owned(),
                json!({ "key": "app_name", "from": "Palmr", "to": "Nova" })
            ),
            (
                "SETTING_CHANGED".to_owned(),
                "show_version".to_owned(),
                json!({ "key": "show_version", "from": true, "to": false })
            ),
            (
                "SETTING_CHANGED".to_owned(),
                "default_user_quota_bytes".to_owned(),
                json!({ "key": "default_user_quota_bytes", "from": null, "to": 2048 })
            ),
            (
                "SETTING_CHANGED".to_owned(),
                "max_public_link_lifetime_days".to_owned(),
                json!({ "key": "max_public_link_lifetime_days", "from": null, "to": 30 })
            ),
            (
                "SETTING_CHANGED".to_owned(),
                "default_user_quota_bytes".to_owned(),
                json!({ "key": "default_user_quota_bytes", "from": 2048, "to": null })
            ),
            (
                "SECURITY_POLICY_CHANGED".to_owned(),
                "password_min_length".to_owned(),
                json!({ "key": "password_min_length", "from": 8, "to": 12 })
            ),
            (
                "MANDATORY_2FA_POLICY_CHANGED".to_owned(),
                "two_factor_required".to_owned(),
                json!({ "key": "two_factor_required", "from": false, "to": true })
            ),
        ]
    );
    for row in &rows {
        assert_eq!(row.1, "user");
        assert_eq!(row.2.as_deref(), Some(admin_id.as_str()));
        assert_eq!(row.3.as_deref(), Some("setting"));
        assert_eq!(row.6, "success");
    }
    assert_eq!(
        stack.stored_setting("app_name").await.unwrap().1.as_deref(),
        Some(admin_id.as_str())
    );
    let scan = rows
        .iter()
        .map(|row| row.7.to_lowercase())
        .collect::<String>();
    for forbidden in ["password\":", "secret", "token", "hash", "cipher"] {
        assert!(!scan.contains(forbidden), "{forbidden}");
    }
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_audit_failure_rolls_back_and_keeps_snapshot() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    stack
        .patch_ok("general", &admin, &json!({ "appName": "Before" }))
        .await;
    let before = stack.all_stored_settings().await;
    let audits = stack.settings_audit().await;
    let snapshot = stack.settings.current();

    stack
        .execute(
            "CREATE TRIGGER fail_settings_audit BEFORE INSERT ON audit_events
             WHEN NEW.action IN ('SETTING_CHANGED', 'SECURITY_POLICY_CHANGED')
             BEGIN SELECT RAISE(ABORT, 'audit unavailable'); END",
        )
        .await;
    for (group, body) in [
        (
            "general",
            json!({ "appName": "After", "hideVersion": true }),
        ),
        ("security", json!({ "passwordMinLength": 20 })),
        ("quotas", json!({ "maxFileSizeBytes": 9 })),
    ] {
        let failed = stack.patch_settings(group, &admin, &body).await;
        assert!(failed.status.is_server_error(), "{}", failed.text());
        assert!(
            !failed.text().contains("audit unavailable"),
            "{}",
            failed.text()
        );
        assert_eq!(stack.all_stored_settings().await, before, "{group}");
        assert!(Arc::ptr_eq(&stack.settings.current(), &snapshot), "{group}");
        assert_eq!(stack.settings_audit().await, audits);
    }
    assert_eq!(
        stack.read_settings("general", &admin).await["appName"],
        "Before"
    );

    stack.execute("DROP TRIGGER fail_settings_audit").await;
    let applied = stack
        .patch_ok(
            "general",
            &admin,
            &json!({ "appName": "After", "hideVersion": true }),
        )
        .await;
    assert_eq!(applied["appName"], "After");
    assert_eq!(stack.settings.current().app_name(), "After");
    assert_eq!(stack.settings_audit().await.len(), audits.len() + 2);
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_noop_patch_writes_nothing() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;

    let first = stack
        .patch_ok(
            "general",
            &admin,
            &json!({ "appName": "Nova", "hideVersion": true }),
        )
        .await;
    let stored = stack.all_stored_settings().await;
    let audits = stack.settings_audit().await;
    let snapshot = stack.settings.current();

    clock.advance(Duration::from_secs(120));
    let repeated = stack
        .patch_ok(
            "general",
            &admin,
            &json!({ "appName": "Nova", "hideVersion": true }),
        )
        .await;
    assert_eq!(repeated, first);
    let padded = stack
        .patch_ok("general", &admin, &json!({ "appName": "  Nova  " }))
        .await;
    assert_eq!(padded, first);
    let defaults = stack
        .patch_ok(
            "security",
            &admin,
            &json!({ "passwordMinLength": 8, "twoFactorRequired": false }),
        )
        .await;
    assert_eq!(defaults["passwordMinLength"], 8);
    let unlimited = stack
        .patch_ok("quotas", &admin, &json!({ "defaultUserQuotaBytes": null }))
        .await;
    assert_eq!(unlimited["defaultUserQuotaBytes"], Value::Null);

    assert_eq!(stack.all_stored_settings().await, stored);
    assert_eq!(stack.settings_audit().await, audits);
    assert_eq!(stack.settings.current().app_name(), snapshot.app_name());
    assert!(stack.stored_setting("password_min_length").await.is_none());
    assert!(stack
        .stored_setting("default_user_quota_bytes")
        .await
        .is_none());

    stack
        .patch_ok(
            "general",
            &admin,
            &json!({ "appName": "Nova", "appDescription": "Changed" }),
        )
        .await;
    let rows = stack.settings_audit().await;
    assert_eq!(rows.len(), audits.len() + 1);
    assert_eq!(rows.last().unwrap().4.as_deref(), Some("app_description"));
    let untouched = stack.stored_setting("app_name").await.unwrap();
    assert_eq!(
        untouched.2,
        stored.iter().find(|row| row.0 == "app_name").unwrap().2
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_take_effect_next_request() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    let ada = stack.settings_user("ada", 11).await;

    let baseline = stack.effective(&ada).await;
    assert_eq!(baseline["passwordMinLength"], 8);
    assert_eq!(baseline["quotaBytes"], Value::Null);
    assert_eq!(baseline["maxFileSizeBytes"], Value::Null);
    assert_eq!(baseline["maxPublicLinkLifetimeDays"], Value::Null);
    assert_eq!(baseline["twoFactorRequired"], false);
    assert_eq!(baseline["trustedDevicesEnabled"], true);

    stack
        .patch_ok(
            "security",
            &admin,
            &json!({ "passwordMinLength": 30, "publicLinkPasswordMinLength": 16, "trustedDeviceDurationDays": 7 }),
        )
        .await;
    let policy = stack
        .change_password(&ada, PASSWORD, NEW_PASSWORD, 11)
        .await;
    assert_code(
        &policy,
        StatusCode::UNPROCESSABLE_ENTITY,
        "PASSWORD_POLICY_VIOLATION",
    );
    assert_detail(&policy, "minLength", &json!(30));
    let effective = stack.effective(&ada).await;
    assert_eq!(effective["passwordMinLength"], 30);
    assert_eq!(effective["publicLinkPasswordMinLength"], 16);
    assert_eq!(effective["trustedDeviceDurationDays"], 7);
    stack
        .patch_ok("security", &admin, &json!({ "passwordMinLength": 8 }))
        .await;
    let changed = stack
        .change_password(&ada, PASSWORD, NEW_PASSWORD, 11)
        .await;
    assert_eq!(changed.status, StatusCode::NO_CONTENT, "{}", changed.text());
    let ada = Credentials::from(&changed);
    assert_eq!(stack.effective(&ada).await["passwordMinLength"], 8);

    stack
        .patch_ok(
            "quotas",
            &admin,
            &json!({ "defaultUserQuotaBytes": 4096, "maxFileSizeBytes": 1024 }),
        )
        .await;
    stack
        .patch_ok(
            "public-links",
            &admin,
            &json!({ "maxPublicLinkLifetimeDays": 9 }),
        )
        .await;
    let effective = stack.effective(&ada).await;
    assert_eq!(effective["quotaBytes"], 4096);
    assert_eq!(effective["maxFileSizeBytes"], 1024);
    assert_eq!(effective["maxPublicLinkLifetimeDays"], 9);
    let usage = stack
        .get("/api/v1/profile/usage", Some(&ada.session), 11)
        .await;
    assert_eq!(usage.json()["quotaBytes"], 4096, "{}", usage.text());
    stack
        .patch_ok("quotas", &admin, &json!({ "defaultUserQuotaBytes": null }))
        .await;
    assert_eq!(stack.effective(&ada).await["quotaBytes"], Value::Null);

    stack
        .patch_ok("security", &admin, &json!({ "recentAuthMinutes": 1 }))
        .await;
    clock.advance(Duration::from_secs(2 * 60));
    let lapsed = stack
        .patch_settings("security", &admin, &json!({ "recentAuthMinutes": 2 }))
        .await;
    assert_code(&lapsed, StatusCode::FORBIDDEN, "AUTH_RECENT_AUTH_REQUIRED");
    assert_eq!(stack.settings.current().security.recent_auth_minutes, 1);

    let fresh = stack.reauth_with_password(&admin, 10).await;
    assert_eq!(fresh.status, StatusCode::NO_CONTENT, "{}", fresh.text());
    let open = stack.get(PROFILE, Some(&ada.session), 11).await;
    assert_eq!(open.status, StatusCode::OK, "{}", open.text());
    stack
        .patch_ok("security", &admin, &json!({ "twoFactorRequired": true }))
        .await;
    assert_code(
        &stack.get(PROFILE, Some(&ada.session), 11).await,
        StatusCode::FORBIDDEN,
        "AUTH_2FA_ENROLLMENT_REQUIRED",
    );
    assert_eq!(
        stack.get(ME, Some(&ada.session), 11).await.json()["restriction"],
        "mfa_enrollment_required"
    );
    assert_eq!(stack.effective(&ada).await["twoFactorRequired"], true);
    stack
        .setting("two_factor_required", "boolean", "false")
        .await;
    assert_eq!(
        stack.get(PROFILE, Some(&ada.session), 11).await.status,
        StatusCode::OK
    );
    stack.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn it_settings_concurrent_group_updates_do_not_clobber() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;

    for round in 0_i64..6 {
        let general = json!({ "appName": format!("Round {round}") });
        let security = json!({ "passwordMinLength": 10 + round, "maxLoginAttempts": 4 + round });
        let quotas = json!({ "defaultUserQuotaBytes": 1000 + round, "maxFileSizeBytes": round });
        let links = json!({ "maxPublicLinkLifetimeDays": 1 + round });
        let (a, b, c, d) = tokio::join!(
            stack.patch_settings("general", &admin, &general),
            stack.patch_settings("security", &admin, &security),
            stack.patch_settings("quotas", &admin, &quotas),
            stack.patch_settings("public-links", &admin, &links),
        );
        for fetched in [&a, &b, &c, &d] {
            assert_eq!(fetched.status, StatusCode::OK, "{}", fetched.text());
        }
        let settings = stack.settings.current();
        assert_eq!(settings.app_name(), format!("Round {round}"));
        assert_eq!(
            settings.security.password_min_length,
            u32::try_from(10 + round).unwrap()
        );
        assert_eq!(
            settings.security.max_login_attempts,
            u32::try_from(4 + round).unwrap()
        );
        assert_eq!(
            settings
                .quotas
                .default_user_quota_bytes
                .map(|size| size.get()),
            Some(u64::try_from(1000 + round).unwrap())
        );
        assert_eq!(
            settings.public_links.max_public_link_lifetime_days,
            Some(u32::try_from(1 + round).unwrap())
        );
    }

    let persisted = SettingsService::load(
        &stack.pools,
        Arc::new(stack.clock.clone()),
        &InstanceKey::load_or_create(root.path()).unwrap().0,
    )
    .await
    .unwrap();
    assert_eq!(persisted.current().app_name(), "Round 5");
    assert_eq!(persisted.current().security.password_min_length, 15);
    assert_eq!(
        persisted
            .current()
            .public_links
            .max_public_link_lifetime_days,
        Some(6)
    );
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_operator_values_are_not_expressible() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let (admin, _) = stack.settings_admin().await;
    let before = stack.all_stored_settings().await;
    let mut all = stack
        .settings_call(Method::GET, None, &admin, None, 10)
        .await
        .json();
    all["smtp"].as_object_mut().unwrap().remove("port");
    let text = all.to_string().to_lowercase();
    for operator in [
        "bind",
        "port",
        "baseurl",
        "trustproxy",
        "storage",
        "s3",
        "bucket",
        "endpoint",
        "datadir",
    ] {
        assert!(!text.contains(operator), "{operator}: {text}");
    }
    for operator in crate::config::Variable::ALL {
        let camel = operator
            .name()
            .trim_start_matches("PALMR_")
            .to_lowercase()
            .split('_')
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    part.to_owned()
                } else {
                    part[..1].to_uppercase() + &part[1..]
                }
            })
            .collect::<String>();
        for group in ["general", "security", "quotas", "public-links"] {
            let body = json!({ camel.clone(): "x", operator.name(): "x" });
            let rejected = stack.patch_settings(group, &admin, &body).await;
            assert_code(
                &rejected,
                StatusCode::UNPROCESSABLE_ENTITY,
                "SETTING_UNKNOWN",
            );
        }
    }
    let smtp_members = [
        "enabled",
        "host",
        "port",
        "security",
        "username",
        "password",
        "fromName",
        "fromEmail",
        "allowSelfSignedCertificate",
        "noAuth",
    ];
    for operator in crate::config::Variable::ALL {
        let name = operator.name();
        let lowered = name.trim_start_matches("PALMR_").to_lowercase();
        if smtp_members
            .iter()
            .any(|member| member.to_lowercase() == lowered.replace('_', ""))
        {
            continue;
        }
        let body = json!({ name: "x" });
        let rejected = stack.patch_settings("smtp", &admin, &body).await;
        assert_code(
            &rejected,
            StatusCode::UNPROCESSABLE_ENTITY,
            "SETTING_UNKNOWN",
        );
    }
    assert_eq!(stack.all_stored_settings().await, before);
    stack.stop().await;
}

#[tokio::test]
async fn it_settings_session_policy_takes_effect_next_request() {
    let root = TempDir::new().unwrap();
    let clock = TestClock::new(START);
    let stack = Stack::start(root.path(), &clock).await;
    let (admin, _) = stack.settings_admin().await;
    let ada = stack.settings_user("ada", 11).await;

    let window = |me: &Value| {
        let moment = |name: &str| {
            OffsetDateTime::parse(
                me["session"][name].as_str().unwrap(),
                &time::format_description::well_known::Rfc3339,
            )
            .unwrap()
        };
        moment("expiresAt") - moment("lastSeenAt")
    };
    let baseline = stack.get(ME, Some(&ada.session), 11).await.json();
    assert_eq!(window(&baseline), time::Duration::days(7));

    stack
        .patch_ok("security", &admin, &json!({ "sessionIdleDays": 1 }))
        .await;
    clock.advance(Duration::from_secs(25 * 3600));
    let touched = stack.get(ME, Some(&ada.session), 11).await;
    assert_eq!(touched.status, StatusCode::OK, "{}", touched.text());
    assert_eq!(window(&touched.json()), time::Duration::days(1));

    let again = stack.signed_in("ada", 12).await;
    let fresh = stack.get(ME, Some(&again.session), 12).await.json();
    assert_eq!(window(&fresh), time::Duration::days(1));

    clock.advance(Duration::from_secs(2 * 24 * 3600));
    assert_code(
        &stack.get(ME, Some(&ada.session), 11).await,
        StatusCode::UNAUTHORIZED,
        "AUTH_REQUIRED",
    );
    stack.stop().await;
}
