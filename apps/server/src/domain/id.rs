use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    str::FromStr,
    sync::Mutex,
};

use uuid::{ContextV7, Timestamp as UuidTimestamp, Uuid, Variant, Version};

use super::clock::Clock;

// A single process-wide context keeps generated ids strictly increasing,
// including within one millisecond and across a backward clock step.
static CONTEXT: Mutex<ContextV7> = Mutex::new(ContextV7::new());

pub struct Id<E> {
    uuid: Uuid,
    entity: PhantomData<fn() -> E>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidId;

impl<E> Id<E> {
    pub fn generate(clock: &dyn Clock) -> Self {
        let now = clock.now();
        let seconds = u64::try_from(now.unix_timestamp()).unwrap_or(0);
        let timestamp = UuidTimestamp::from_unix(&CONTEXT, seconds, now.nanosecond());
        Self::from_uuid(Uuid::new_v7(timestamp))
    }

    const fn from_uuid(uuid: Uuid) -> Self {
        Self {
            uuid,
            entity: PhantomData,
        }
    }
}

impl<E> Clone for Id<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for Id<E> {}

impl<E> PartialEq for Id<E> {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
    }
}

impl<E> Eq for Id<E> {}

impl<E> PartialOrd for Id<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> Ord for Id<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.uuid.cmp(&other.uuid)
    }
}

impl<E> Hash for Id<E> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.uuid.hash(state);
    }
}

impl<E> fmt::Debug for Id<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Id").field(&self.uuid).finish()
    }
}

impl<E> fmt::Display for Id<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.uuid.hyphenated(), f)
    }
}

impl<E> FromStr for Id<E> {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::try_parse(text).map_err(|_| InvalidId)?;
        let canonical = uuid.get_version() == Some(Version::SortRand)
            && uuid.get_variant() == Variant::RFC4122
            && uuid.hyphenated().to_string() == text;
        if canonical {
            Ok(Self::from_uuid(uuid))
        } else {
            Err(InvalidId)
        }
    }
}

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("id is not a lowercase hyphenated UUIDv7")
    }
}

impl std::error::Error for InvalidId {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use time::macros::datetime;
    use uuid::{Uuid, Version};

    use super::{Id, InvalidId};
    use crate::domain::clock::{Clock, TestClock};

    enum Probe {}

    type ProbeId = Id<Probe>;

    fn assert_canonical_v7(text: &str) {
        assert_eq!(text.len(), 36);
        for (index, char) in text.char_indices() {
            if matches!(index, 8 | 13 | 18 | 23) {
                assert_eq!(char, '-', "{text}");
            } else {
                assert!(matches!(char, '0'..='9' | 'a'..='f'), "{text}");
            }
        }
        assert_eq!(&text[14..15], "7", "{text}");
        assert!(matches!(&text[19..20], "8" | "9" | "a" | "b"), "{text}");
    }

    fn unix_millis(id: ProbeId) -> u64 {
        let (seconds, nanos) = id.uuid.get_timestamp().unwrap().to_unix();
        seconds * 1_000 + u64::from(nanos / 1_000_000)
    }

    #[test]
    fn unit_uuidv7_sortable_and_canonical() {
        let start = datetime!(2026-09-23 17:42:31.123 UTC);
        let clock = TestClock::new(start);
        let start_millis = u64::try_from(start.unix_timestamp()).unwrap() * 1_000 + 123;

        let first = ProbeId::generate(&clock as &dyn Clock);
        assert_eq!(first.uuid.get_version(), Some(Version::SortRand));
        assert_eq!(unix_millis(first), start_millis);

        let mut ids = vec![first];
        for step in 0..3_000 {
            if step % 1_000 == 999 {
                clock.advance(Duration::from_millis(1));
            }
            ids.push(ProbeId::generate(&clock));
        }
        clock.set(start - Duration::from_secs(3_600));
        ids.extend((0..100).map(|_| ProbeId::generate(&clock)));
        clock.set(start + Duration::from_secs(60));
        let later = ProbeId::generate(&clock);
        assert_eq!(unix_millis(later), start_millis + 60_000);
        ids.push(later);

        let rendered: Vec<String> = ids.iter().map(ToString::to_string).collect();
        for text in &rendered {
            assert_canonical_v7(text);
            assert_eq!(
                text.parse::<ProbeId>().map(|id| id.to_string()).as_deref(),
                Ok(text.as_str())
            );
        }
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(rendered.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn unit_id_parse_accepts_only_canonical_v7() {
        let canonical = "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d";
        let id: ProbeId = canonical.parse().unwrap();
        assert_eq!(id.to_string(), canonical);
        assert_eq!(format!("{id:?}"), format!("Id({canonical})"));

        let v4 = Uuid::from_u128(0x6f1c_2a3b_4c5d_4e6f_8a7b_9c0d_1e2f_3a4b).to_string();
        for text in [
            "",
            "01996FC4-6A33-7C1E-9D2B-4F1A8E3C5B7D",
            "01996fc4-6a33-7c1e-9D2b-4f1a8e3c5b7d",
            "01996fc46a337c1e9d2b4f1a8e3c5b7d",
            "{01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d}",
            "urn:uuid:01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d",
            " 01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7d",
            "01996fc4-6a33-7c1e-9d2b-4f1a8e3c5b7",
            "01996fc4-6a33-7c1e-cd2b-4f1a8e3c5b7d",
            "00000000-0000-0000-0000-000000000000",
            &v4,
        ] {
            assert_eq!(text.parse::<ProbeId>(), Err(InvalidId), "{text:?}");
        }
        assert!(!InvalidId.to_string().is_empty());
    }
}
