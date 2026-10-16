use super::GrantContext;

impl GrantContext {
    pub(crate) const fn after_authorization() -> Self {
        Self { _sealed: () }
    }
}
