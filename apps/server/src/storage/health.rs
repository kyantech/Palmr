#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageHealth {
    Ok,
    Degraded,
    Down,
}

impl StorageHealth {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Degraded => "degraded",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfTestReport {
    pub passed: bool,
}

#[cfg(test)]
mod tests {
    use super::StorageHealth;

    #[test]
    fn unit_storage_health_vocabulary() {
        let names = [
            StorageHealth::Ok,
            StorageHealth::Degraded,
            StorageHealth::Down,
        ]
        .map(StorageHealth::as_str);
        assert_eq!(names, ["ok", "degraded", "down"]);
    }
}
