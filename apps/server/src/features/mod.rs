#[allow(
    dead_code,
    reason = "audit vocabulary and per-action builders are consumed as features add actions"
)]
pub mod audit;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "session mint/rotation and transaction-safe revocation primitives are consumed by later authentication and account-security tasks"
    )
)]
pub mod auth;
pub mod branding;
#[allow(
    dead_code,
    reason = "the transport trait, producers and template seams are consumed as features request mail"
)]
pub mod email;
#[allow(
    dead_code,
    reason = "typed settings, the default registry and the write primitive are consumed as admin settings routes and features are wired"
)]
pub mod settings;
pub mod setup;
#[allow(
    dead_code,
    reason = "the user repository, password policy, last-admin guard and quota resolver are consumed as auth and admin features are wired"
)]
pub mod users;
