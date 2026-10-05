use super::error::ExternalLoginError;
use super::resolve::{create_link, refused, LinkInsert, LinkMethod, Resolution, ResolveInput};
use crate::domain::email::Email;
use crate::domain::error_code::ErrorCode;
use crate::domain::normalize::normalize;
use crate::domain::role::Role;
use crate::domain::time::Timestamp;
use crate::domain::username::Username;
use crate::features::audit::actions::{self, UserCreatedFacts};
use crate::features::audit::model::{Actor, AuditEvent, Outcome, Target, TargetType};
use crate::features::users::error::UserError;
use crate::features::users::model::{NewUser, QuotaOverride, UserId, MAX_DISPLAY_TEXT_CHARS};
use crate::features::users::repo as users;
use crate::infra::crypto::random_bytes;
use crate::infra::db::WriteTx;

pub const USERNAME_BASE_MAX_CHARS: usize = 28;
pub const USERNAME_MIN_CHARS: usize = 3;
pub const USERNAME_FALLBACK_BASE: &str = "user";
pub const LAST_DETERMINISTIC_SUFFIX: u32 = 50;
pub const RANDOM_BASE_MAX_CHARS: usize = 20;
pub const RANDOM_SUFFIX_BYTES: usize = 5;

const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

const fn is_separator(character: char) -> bool {
    matches!(character, '.' | '_' | '-')
}

pub fn username_base(email_local_part: &str) -> String {
    let normalized = normalize(email_local_part);
    let mut base = String::with_capacity(normalized.len());
    let mut after_separator = false;
    for character in normalized.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            base.push(character);
            after_separator = false;
        } else if is_separator(character) && !after_separator {
            base.push(character);
            after_separator = true;
        }
    }
    let trimmed = base.trim_matches(is_separator);
    let truncated = &trimmed[..trimmed.len().min(USERNAME_BASE_MAX_CHARS)];
    let finished = truncated.trim_end_matches(is_separator);
    if finished.len() < USERNAME_MIN_CHARS {
        USERNAME_FALLBACK_BASE.to_owned()
    } else {
        finished.to_owned()
    }
}

pub fn deterministic_candidates(base: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(base.to_owned())
        .chain((2..=LAST_DETERMINISTIC_SUFFIX).map(move |attempt| format!("{base}-{attempt}")))
}

pub fn random_candidate(base: &str, entropy: &[u8; RANDOM_SUFFIX_BYTES]) -> String {
    let truncated = &base[..base.len().min(RANDOM_BASE_MAX_CHARS)];
    let truncated = truncated.trim_end_matches(is_separator);
    format!("{truncated}-{}", base32_suffix(entropy))
}

pub fn base32_suffix(entropy: &[u8; RANDOM_SUFFIX_BYTES]) -> String {
    let bits = entropy.iter().fold(0_u64, |accumulator, byte| {
        (accumulator << 8) | u64::from(*byte)
    });
    (0..8)
        .rev()
        .map(|index| char::from(BASE32[usize::try_from((bits >> (index * 5)) & 0x1f).unwrap_or(0)]))
        .collect()
}

pub fn display_names(name: Option<&str>, fallback: &str) -> (String, String) {
    let cleaned: String = name
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let mut parts = cleaned.split_whitespace();
    let first = parts.next().unwrap_or(fallback);
    let rest = parts.collect::<Vec<_>>().join(" ");
    (bounded(first), bounded(&rest))
}

fn bounded(text: &str) -> String {
    text.chars().take(MAX_DISPLAY_TEXT_CHARS).collect()
}

pub async fn provision(
    tx: &mut WriteTx<'_>,
    input: &ResolveInput<'_>,
    email: &Email,
) -> Result<Resolution, ExternalLoginError> {
    provision_with(tx, input, email, &random_entropy).await
}

fn random_entropy() -> Result<[u8; RANDOM_SUFFIX_BYTES], ExternalLoginError> {
    let mut entropy = [0_u8; RANDOM_SUFFIX_BYTES];
    entropy.copy_from_slice(&random_bytes(RANDOM_SUFFIX_BYTES)?);
    Ok(entropy)
}

pub async fn provision_with(
    tx: &mut WriteTx<'_>,
    input: &ResolveInput<'_>,
    email: &Email,
    entropy: &(dyn Fn() -> Result<[u8; RANDOM_SUFFIX_BYTES], ExternalLoginError> + Sync),
) -> Result<Resolution, ExternalLoginError> {
    let local_part = email.as_str().split('@').next().unwrap_or_default();
    let base = username_base(local_part);
    let (first_name, last_name) = display_names(input.identity.name.as_deref(), &base);
    let id = UserId::generate(input.clock);

    let mut candidates: Vec<String> = deterministic_candidates(&base).collect();
    let mut randomized = false;
    let mut next = 0;
    let user = loop {
        let Some(candidate) = candidates.get(next) else {
            if randomized {
                return Err(refused(ErrorCode::AuthExternalUsernameUnavailable));
            }
            randomized = true;
            candidates.push(random_candidate(&base, &entropy()?));
            continue;
        };
        next += 1;
        let username = Username::parse(candidate)
            .map_err(|_| ExternalLoginError::internal("external_username_invalid"))?;
        let new = NewUser {
            email: email.clone(),
            username,
            first_name: first_name.clone(),
            last_name: last_name.clone(),
            password_hash: None,
            must_change_password: false,
            role: Role::User,
            is_active: true,
            quota: QuotaOverride::Inherit,
            created_by: None,
        };
        match users::insert_with_id(tx, input.clock, id, &new).await {
            Ok(user) => break user,
            Err(UserError::UsernameTaken) => {}
            Err(UserError::EmailTaken) => {
                return Err(refused(ErrorCode::ProviderAutoProvisionDisabled));
            }
            Err(error) => return Err(error.into()),
        }
    };

    users::insert_preferences(tx, input.clock, user.id, input.locale).await?;
    let now = Timestamp::try_from(input.clock.now())?;
    let event = AuditEvent::new(
        actions::user_created(UserCreatedFacts {
            role: user.role,
            is_active: user.is_active,
            local_password: false,
            must_change_password: false,
            quota_mode: user.quota.mode(),
        }),
        Actor::user(&user.id.to_string(), &user.username),
        Outcome::Success,
        now,
    )
    .with_target(
        Target::new(TargetType::User)
            .id(&user.id.to_string())
            .label(&user.username),
    )
    .with_client(input.client.clone());
    input.audit.record_in_tx(tx, &event).await?;

    match create_link(tx, input, &user, email, LinkMethod::AutoProvision).await? {
        LinkInsert::Created(link) => Ok(Resolution::Provisioned(link)),
        LinkInsert::SubjectTaken(_) | LinkInsert::UserAlreadyLinked => {
            Err(ExternalLoginError::internal("provisioned_link_conflict"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_username_base_follows_the_accepted_algorithm() {
        let cases = [
            ("alice", "alice"),
            ("Alice.Smith", "alice.smith"),
            ("ＡＬＩＣＥ", "alice"),
            ("a..b__c--d", "a.b_c-d"),
            ("..alice..", "alice"),
            ("al ice+tag", "alicetag"),
            ("ab", "user"),
            ("é", "user"),
            ("---", "user"),
            ("名前", "user"),
            ("ab.c", "ab.c"),
            (
                "abcdefghijklmnopqrstuvwxyz0123456789",
                "abcdefghijklmnopqrstuvwxyz01",
            ),
            (
                "abcdefghijklmnopqrstuvwxyz0.-",
                "abcdefghijklmnopqrstuvwxyz0",
            ),
            (
                "abcdefghijklmnopqrstuvwxyz0.1",
                "abcdefghijklmnopqrstuvwxyz0",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(username_base(input), expected, "{input}");
        }
        for input in ["alice", "a.b", "Ünïcode", "x".repeat(100).as_str()] {
            let base = username_base(input);
            assert!(base.len() >= USERNAME_MIN_CHARS && base.len() <= USERNAME_BASE_MAX_CHARS);
            assert!(Username::parse(&base).is_ok());
        }
    }

    #[test]
    fn unit_deterministic_candidates_are_base_then_two_through_fifty() {
        let candidates: Vec<String> = deterministic_candidates("alice").collect();
        assert_eq!(candidates.len(), 50);
        assert_eq!(candidates[0], "alice");
        assert_eq!(candidates[1], "alice-2");
        assert_eq!(candidates[49], "alice-50");
        let widest: Vec<String> =
            deterministic_candidates(&"a".repeat(USERNAME_BASE_MAX_CHARS)).collect();
        assert!(widest.iter().all(|candidate| candidate.len() <= 64));
    }

    #[test]
    fn unit_random_candidate_truncates_to_twenty_and_appends_eight_base32() {
        let base = "abcdefghijklmnopqrstuvwxyz01";
        let candidate = random_candidate(base, &[0xff; RANDOM_SUFFIX_BYTES]);
        assert_eq!(candidate, "abcdefghijklmnopqrst-77777777");
        assert_eq!(candidate.len(), 29);
        assert_eq!(base32_suffix(&[0; RANDOM_SUFFIX_BYTES]), "aaaaaaaa");
        assert_eq!(base32_suffix(&[0x00, 0x44, 0x32, 0x14, 0xc7]), "abcdefgh");
        let trailing = random_candidate("abcdefghijklmnopqrs.t", &[0; RANDOM_SUFFIX_BYTES]);
        assert_eq!(trailing, "abcdefghijklmnopqrs-aaaaaaaa");
    }

    #[test]
    fn unit_display_names_split_the_claim_and_stay_bounded() {
        assert_eq!(
            display_names(Some("Ada King Lovelace"), "ada"),
            ("Ada".to_owned(), "King Lovelace".to_owned())
        );
        assert_eq!(
            display_names(Some("Ada"), "ada"),
            ("Ada".to_owned(), String::new())
        );
        assert_eq!(
            display_names(None, "ada"),
            ("ada".to_owned(), String::new())
        );
        assert_eq!(
            display_names(Some("  \u{0007} "), "ada"),
            ("ada".to_owned(), String::new())
        );
        let long = "x".repeat(500);
        let (first, last) = display_names(Some(&format!("{long} {long}")), "ada");
        assert_eq!(first.chars().count(), MAX_DISPLAY_TEXT_CHARS);
        assert_eq!(last.chars().count(), MAX_DISPLAY_TEXT_CHARS);
    }
}
