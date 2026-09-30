use super::*;
use crate::domain::clock::Clock;
use crate::features::audit::model::ClientMetadata;
use crate::features::auth::sessions::{AuthMethod, AuthenticatedPrincipal, SessionRestriction};
use crate::features::users::admin_service::AdminUserError;
use crate::features::users::error::UserError;

const ROUNDS: usize = 12;

struct Admins {
    first: UserId,
    second: UserId,
}

impl Stack {
    async fn lifecycle_admin(&self, username: &str) -> UserId {
        let hash = password_hash();
        let id = self
            .user(UserSpec::local(
                username,
                &format!("{username}@example.test"),
                &hash,
            ))
            .await;
        self.execute(&format!(
            "UPDATE users SET role = 'admin' WHERE id = '{id}'"
        ))
        .await;
        id
    }

    async fn two_admins(&self) -> Admins {
        Admins {
            first: self.lifecycle_admin("first").await,
            second: self.lifecycle_admin("second").await,
        }
    }

    async fn restore(&self, admins: &Admins) {
        self.execute(&format!(
            "UPDATE users SET role = 'admin', is_active = 1, deactivated_at = NULL,
                    deactivated_by = NULL
              WHERE id IN ('{}', '{}')",
            admins.first, admins.second
        ))
        .await;
    }

    fn lifecycle_principal(&self, id: UserId, username: &str) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            user_id: id,
            username: username.to_owned(),
            session_id: crate::domain::id::Id::generate(&self.clock),
            role: Role::Admin,
            restriction: SessionRestriction::None,
            last_auth_at: Timestamp::try_from(self.clock.now()).unwrap(),
            recent_auth: true,
            auth_method: AuthMethod::Password,
        }
    }

    async fn active_admins(&self) -> i64 {
        self.scalar_i64("SELECT COUNT(*) FROM users WHERE role = 'admin' AND is_active = 1")
            .await
    }

    async fn lifecycle_audits(&self, action: &str) -> i64 {
        self.scalar_i64(&format!(
            "SELECT COUNT(*) FROM audit_events WHERE action = '{action}'"
        ))
        .await
    }
}

#[derive(Clone, Copy, Debug)]
enum Removal {
    Demote,
    Deactivate,
}

impl Removal {
    const fn audit(self) -> &'static str {
        match self {
            Self::Demote => "USER_ROLE_CHANGED",
            Self::Deactivate => "USER_DEACTIVATED",
        }
    }
}

async fn remove(
    stack: &Stack,
    how: Removal,
    actor: &AuthenticatedPrincipal,
    target: UserId,
) -> Result<(), AdminUserError> {
    let client = ClientMetadata::none();
    match how {
        Removal::Demote => stack
            .admin_users
            .change_role(actor, target, Role::User, &client)
            .await
            .map(|_| ()),
        Removal::Deactivate => stack
            .admin_users
            .deactivate(actor, target, &client)
            .await
            .map(|_| ()),
    }
}

fn is_last_admin(error: &AdminUserError) -> bool {
    matches!(error, AdminUserError::User(UserError::LastAdminProtected))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_last_admin_cannot_be_demoted_deactivated_concurrently() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admins = stack.two_admins().await;
    let first = stack.lifecycle_principal(admins.first, "first");
    let second = stack.lifecycle_principal(admins.second, "second");
    assert_eq!(stack.active_admins().await, 2);

    let crossings = [
        (Removal::Demote, Removal::Demote),
        (Removal::Deactivate, Removal::Deactivate),
        (Removal::Demote, Removal::Deactivate),
        (Removal::Deactivate, Removal::Demote),
    ];
    for round in 0..ROUNDS {
        let (left, right) = crossings[round % crossings.len()];
        let cross = round % 2 == 0;
        let (left_target, right_target) = if cross {
            (admins.second, admins.first)
        } else {
            (admins.first, admins.second)
        };
        let (left_result, right_result) = tokio::join!(
            remove(&stack, left, &first, left_target),
            remove(&stack, right, &second, right_target),
        );
        let label = format!("round {round}: {left:?}/{right:?} cross={cross}");
        let outcomes = [left_result, right_result];
        let winners = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(winners, 1, "{label}: {outcomes:?}");
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| outcome.as_ref().is_err_and(is_last_admin))
                .count(),
            1,
            "{label}: {outcomes:?}"
        );
        assert_eq!(stack.active_admins().await, 1, "{label}");
        let performed = if outcomes[0].is_ok() { left } else { right };
        let landed = stack.lifecycle_audits(performed.audit()).await;
        assert!(landed >= 1, "{label}: the winner is audited");
        stack.restore(&admins).await;
        assert_eq!(stack.active_admins().await, 2, "{label}");
    }

    assert_eq!(
        stack.lifecycle_audits("USER_ROLE_CHANGED").await
            + stack.lifecycle_audits("USER_DEACTIVATED").await,
        i64::try_from(ROUNDS).unwrap(),
        "exactly one lifecycle audit row per round, none for the refused loser"
    );
    assert_eq!(stack.lifecycle_audits("USER_ACTIVATED").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it_lifecycle_guard_holds_against_a_third_admin_race() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admins = stack.two_admins().await;
    let third = stack.lifecycle_admin("third").await;
    let actor = stack.lifecycle_principal(third, "third");

    let (first, second, last) = (
        remove(&stack, Removal::Demote, &actor, admins.first),
        remove(&stack, Removal::Deactivate, &actor, admins.second),
        remove(&stack, Removal::Demote, &actor, third),
    );
    let outcomes = tokio::join!(first, second, last);
    let results = [outcomes.0, outcomes.1, outcomes.2];
    assert_eq!(
        results.iter().filter(|outcome| outcome.is_ok()).count(),
        2,
        "{results:?}"
    );
    assert_eq!(
        results
            .iter()
            .filter(|outcome| outcome.as_ref().is_err_and(is_last_admin))
            .count(),
        1,
        "{results:?}"
    );
    assert_eq!(stack.active_admins().await, 1);
}

#[tokio::test]
async fn it_lifecycle_self_actions_are_allowed_while_another_admin_remains() {
    let root = TempDir::new().unwrap();
    let stack = Stack::start(root.path(), &TestClock::new(START)).await;
    let admins = stack.two_admins().await;
    let first = stack.lifecycle_principal(admins.first, "first");

    remove(&stack, Removal::Demote, &first, admins.first)
        .await
        .unwrap();
    assert_eq!(stack.active_admins().await, 1);
    let last = stack.lifecycle_principal(admins.second, "second");
    for how in [Removal::Demote, Removal::Deactivate] {
        let refused = remove(&stack, how, &last, admins.second).await.unwrap_err();
        assert!(is_last_admin(&refused), "{how:?}: {refused:?}");
    }
    assert_eq!(stack.active_admins().await, 1);
    assert_eq!(stack.lifecycle_audits("USER_ROLE_CHANGED").await, 1);
    assert_eq!(stack.lifecycle_audits("USER_DEACTIVATED").await, 0);
}
