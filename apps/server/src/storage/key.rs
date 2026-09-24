use std::fmt;

use uuid::Uuid;

const OBJECTS_PREFIX: &str = "objects/";
const BRANDING_PREFIX: &str = "branding/";
const OID_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyNamespace {
    Objects,
    Branding(BrandingKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BrandingKind {
    Logo,
    Favicon,
    LoginBackground,
    EmailLogo,
    OgDefaultImage,
    Avatar,
    Hero,
}

impl BrandingKind {
    pub const ALL: [Self; 7] = [
        Self::Logo,
        Self::Favicon,
        Self::LoginBackground,
        Self::EmailLogo,
        Self::OgDefaultImage,
        Self::Avatar,
        Self::Hero,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Logo => "logo",
            Self::Favicon => "favicon",
            Self::LoginBackground => "login_background",
            Self::EmailLogo => "email_logo",
            Self::OgDefaultImage => "og_default_image",
            Self::Avatar => "avatar",
            Self::Hero => "hero",
        }
    }

    fn from_segment(segment: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == segment)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjectKey {
    namespace: KeyNamespace,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidKey;

impl ObjectKey {
    pub fn allocate(namespace: KeyNamespace) -> Self {
        let oid = Uuid::now_v7().simple().to_string();
        let text = match namespace {
            KeyNamespace::Objects => {
                format!("{OBJECTS_PREFIX}{}/{}/{oid}", &oid[..2], &oid[2..4])
            }
            KeyNamespace::Branding(kind) => format!("{BRANDING_PREFIX}{}/{oid}", kind.as_str()),
        };
        Self { namespace, text }
    }

    pub fn parse(text: &str) -> Result<Self, InvalidKey> {
        let namespace = if let Some(rest) = text.strip_prefix(OBJECTS_PREFIX) {
            parse_objects(rest)?
        } else if let Some(rest) = text.strip_prefix(BRANDING_PREFIX) {
            parse_branding(rest)?
        } else {
            return Err(InvalidKey);
        };
        Ok(Self {
            namespace,
            text: text.to_owned(),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub const fn namespace(&self) -> KeyNamespace {
        self.namespace
    }
}

fn parse_objects(rest: &str) -> Result<KeyNamespace, InvalidKey> {
    let mut segments = rest.split('/');
    let (Some(first), Some(second), Some(oid), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(InvalidKey);
    };
    if is_oid(oid) && first == &oid[..2] && second == &oid[2..4] {
        Ok(KeyNamespace::Objects)
    } else {
        Err(InvalidKey)
    }
}

fn parse_branding(rest: &str) -> Result<KeyNamespace, InvalidKey> {
    let (segment, oid) = rest.split_once('/').ok_or(InvalidKey)?;
    let kind = BrandingKind::from_segment(segment).ok_or(InvalidKey)?;
    if is_oid(oid) {
        Ok(KeyNamespace::Branding(kind))
    } else {
        Err(InvalidKey)
    }
}

fn is_oid(text: &str) -> bool {
    text.len() == OID_LEN
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

impl fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("object key does not match the storage key grammar")
    }
}

impl std::error::Error for InvalidKey {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;
    use time::macros::datetime;

    use super::{BrandingKind, InvalidKey, KeyNamespace, ObjectKey};
    use crate::domain::clock::TestClock;
    use crate::domain::id::Id;

    const OID: &str = "0192f3c8d7e94a1b8f0c2d5e6a7b8c9d";

    enum Row {}

    fn assert_rejected(input: &str) {
        assert_eq!(ObjectKey::parse(input), Err(InvalidKey), "{input:?}");
    }

    fn assert_objects_shape(key: &ObjectKey) -> &str {
        let text = key.as_str();
        let rest = text.strip_prefix("objects/").unwrap();
        let segments: Vec<&str> = rest.split('/').collect();
        assert_eq!(segments.len(), 3, "{text}");
        let oid = segments[2];
        assert_eq!(oid.len(), 32);
        assert!(oid
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
        assert_eq!(segments[0], &oid[..2]);
        assert_eq!(segments[1], &oid[2..4]);
        oid
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn prop_object_key_grammar(
            batch in 1_usize..64,
            kind_index in 0_usize..7,
            arbitrary in "\\PC{0,80}",
        ) {
            let clock = TestClock::new(datetime!(2026-01-01 00:00 UTC));
            let mut seen = HashSet::new();
            for _ in 0..batch {
                let row_id = Id::<Row>::generate(&clock).to_string().replace('-', "");
                let key = ObjectKey::allocate(KeyNamespace::Objects);
                prop_assert_eq!(key.namespace(), KeyNamespace::Objects);
                let oid = assert_objects_shape(&key).to_owned();
                prop_assert_ne!(&oid, &row_id);
                prop_assert_eq!(ObjectKey::parse(key.as_str()), Ok(key.clone()));
                prop_assert!(seen.insert(oid));
            }

            let kind = BrandingKind::ALL[kind_index];
            let branding = ObjectKey::allocate(KeyNamespace::Branding(kind));
            prop_assert_eq!(branding.namespace(), KeyNamespace::Branding(kind));
            let prefix = format!("branding/{}/", kind.as_str());
            let oid = branding.as_str().strip_prefix(&prefix).unwrap();
            prop_assert_eq!(oid.len(), 32);
            prop_assert!(oid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            prop_assert_eq!(ObjectKey::parse(branding.as_str()), Ok(branding.clone()));
            prop_assert!(seen.insert(oid.to_owned()));

            if let Ok(parsed) = ObjectKey::parse(&arbitrary) {
                prop_assert_eq!(parsed.as_str(), arbitrary.as_str());
            }
        }
    }

    #[test]
    fn unit_object_key_rejection_table() {
        let valid = format!("objects/01/92/{OID}");
        let key = ObjectKey::parse(&valid).unwrap();
        assert_eq!(key.as_str(), valid);
        assert_eq!(key.namespace(), KeyNamespace::Objects);

        let upper = format!("objects/01/92/{}", OID.to_ascii_uppercase());
        let upper_partial = format!("objects/01/92/0192F3{}", &OID[6..]);
        let short = format!("objects/01/92/{}", &OID[..31]);
        let long = format!("objects/01/92/{OID}0");
        let windows = format!("objects\\01\\92\\{OID}");
        let mixed_separators = format!("objects/01\\92/{OID}");

        let cases: Vec<String> = vec![
            "/etc/shadow".into(),
            format!("/{valid}"),
            "objects/../../etc/shadow".into(),
            format!("objects/01/92/../{OID}"),
            "objects/%2e%2e/%2e%2e/x".into(),
            format!("objects/01/92/{}", OID.replace("0192", "%30")),
            format!("objects/01/92/{OID}\0.png"),
            format!("objects/01/92/\0{}", &OID[1..]),
            upper,
            upper_partial,
            valid.to_ascii_uppercase(),
            "objects/01/92/0192f3".into(),
            short,
            long,
            format!("objects/01/92/{OID}.mp4"),
            windows,
            mixed_separators,
            format!("objects/ff/ff/{OID}"),
            format!("objects/92/01/{OID}"),
            format!("objects/01/93/{OID}"),
            String::new(),
            " ".into(),
            "\t\n  ".into(),
            format!(" {valid}"),
            format!("{valid} "),
            format!("{valid}\n"),
            format!("uploads/01/92/{OID}"),
            format!("thumbnails/01/92/{OID}"),
            format!("runtime/cache/{OID}"),
            format!("_palmr/probe/{OID}"),
            format!("Objects/01/92/{OID}"),
            format!("branding/banner/{OID}"),
            format!("branding/Logo/{OID}"),
            format!("branding/login-background/{OID}"),
            format!("branding//{OID}"),
            format!("objects/01/92/{OID}/extra"),
            format!("objects/01/92/{OID}/"),
            format!("objects/01/92//{OID}"),
            valid.replace('/', "//"),
            format!("branding/logo/{OID}/extra"),
            format!("objects/01/{OID}"),
            format!("objects/{OID}"),
            "objects/".into(),
            "objects".into(),
            format!("branding/{OID}"),
            "branding/logo/".into(),
            "branding/logo".into(),
        ];
        for input in &cases {
            assert_rejected(input);
        }

        let accepted: Vec<&str> = BrandingKind::ALL.iter().map(|k| k.as_str()).collect();
        assert_eq!(
            accepted,
            [
                "logo",
                "favicon",
                "login_background",
                "email_logo",
                "og_default_image",
                "avatar",
                "hero",
            ]
        );
        for kind in BrandingKind::ALL {
            let key = ObjectKey::parse(&format!("branding/{}/{OID}", kind.as_str())).unwrap();
            assert_eq!(key.namespace(), KeyNamespace::Branding(kind));
        }
        for unknown in [
            "banner",
            "thumbnail",
            "objects",
            "",
            "LOGO",
            "logo ",
            "hero2",
        ] {
            assert_rejected(&format!("branding/{unknown}/{OID}"));
        }

        let error = ObjectKey::parse("objects/../../etc/shadow").unwrap_err();
        assert!(!error.to_string().contains("shadow"));
        assert!(!format!("{error:?}").contains("shadow"));
    }

    trait AmbiguousIfImpl<Marker> {
        fn probe() {}
    }

    impl<T: ?Sized> AmbiguousIfImpl<()> for T {}

    macro_rules! assert_not_impl {
        ($ty:ty: $($bound:tt)+) => {
            const _: () = {
                struct Implemented;
                impl<T: ?Sized + $($bound)+> AmbiguousIfImpl<Implemented> for T {}
                let _ = <$ty as AmbiguousIfImpl<_>>::probe;
            };
        };
    }

    assert_not_impl!(ObjectKey: From<String>);
    assert_not_impl!(ObjectKey: From<&'static str>);
    assert_not_impl!(ObjectKey: From<Box<str>>);
    assert_not_impl!(ObjectKey: std::str::FromStr);
    assert_not_impl!(ObjectKey: std::ops::Deref);
    assert_not_impl!(ObjectKey: std::ops::DerefMut);
    assert_not_impl!(ObjectKey: AsMut<str>);
    assert_not_impl!(ObjectKey: AsMut<String>);
    assert_not_impl!(ObjectKey: std::borrow::BorrowMut<String>);
    assert_not_impl!(ObjectKey: Default);
    assert_not_impl!(ObjectKey: serde::de::DeserializeOwned);
    assert_not_impl!(ObjectKey: utoipa::ToSchema);
    assert_not_impl!(KeyNamespace: serde::de::DeserializeOwned);
    assert_not_impl!(BrandingKind: serde::de::DeserializeOwned);
    assert_not_impl!(BrandingKind: std::str::FromStr);

    #[test]
    fn unit_object_key_public_surface() {
        let source = include_str!("key.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();

        let struct_body = production
            .split("pub struct ObjectKey {")
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .unwrap();
        for field in struct_body.lines().map(str::trim).filter(|l| !l.is_empty()) {
            assert!(!field.starts_with("pub"), "public ObjectKey field: {field}");
        }
        assert!(!production.contains("pub struct ObjectKey("));

        let object_key_impl = production
            .split("impl ObjectKey {")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap();
        let public_fns: Vec<&str> = object_key_impl
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with("pub"))
            .collect();
        assert_eq!(
            public_fns,
            [
                "pub fn allocate(namespace: KeyNamespace) -> Self {",
                "pub fn parse(text: &str) -> Result<Self, InvalidKey> {",
                "pub fn as_str(&self) -> &str {",
                "pub const fn namespace(&self) -> KeyNamespace {",
            ]
        );

        let impl_headers: Vec<&str> = production
            .lines()
            .filter(|line| line.starts_with("impl") && line.contains("ObjectKey"))
            .collect();
        assert_eq!(impl_headers, ["impl ObjectKey {"]);
    }
}
