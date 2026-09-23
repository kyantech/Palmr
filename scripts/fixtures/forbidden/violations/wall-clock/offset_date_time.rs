use time::OffsetDateTime;

pub fn created_at() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}
