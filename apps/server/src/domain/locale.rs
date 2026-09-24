use std::{fmt, str::FromStr};

macro_rules! locales {
    ($($variant:ident = $code:literal,)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum LocaleCode {
            $($variant,)+
        }

        impl LocaleCode {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $code,)+
                }
            }
        }

        impl FromStr for LocaleCode {
            type Err = InvalidLocaleCode;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $($code => Ok(Self::$variant),)+
                    _ => Err(InvalidLocaleCode),
                }
            }
        }
    };
}

locales! {
    ArSa = "ar-SA",
    DeDe = "de-DE",
    ElGr = "el-GR",
    EnUs = "en-US",
    EsEs = "es-ES",
    FaIr = "fa-IR",
    FrFr = "fr-FR",
    HeIl = "he-IL",
    HiIn = "hi-IN",
    IdId = "id-ID",
    ItIt = "it-IT",
    JaJp = "ja-JP",
    KoKr = "ko-KR",
    NlNl = "nl-NL",
    PlPl = "pl-PL",
    PtBr = "pt-BR",
    RuRu = "ru-RU",
    SvSe = "sv-SE",
    ThTh = "th-TH",
    TrTr = "tr-TR",
    UkUa = "uk-UA",
    ViVn = "vi-VN",
    ZhCn = "zh-CN",
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLocaleCode;

impl fmt::Display for LocaleCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for InvalidLocaleCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("locale is not a supported locale code")
    }
}

impl std::error::Error for InvalidLocaleCode {}

#[cfg(test)]
mod tests {
    use super::{InvalidLocaleCode, LocaleCode};

    #[test]
    fn unit_locale_code_closed_set() {
        let codes: Vec<&str> = LocaleCode::ALL
            .iter()
            .map(|locale| locale.as_str())
            .collect();
        assert_eq!(
            codes,
            [
                "ar-SA", "de-DE", "el-GR", "en-US", "es-ES", "fa-IR", "fr-FR", "he-IL", "hi-IN",
                "id-ID", "it-IT", "ja-JP", "ko-KR", "nl-NL", "pl-PL", "pt-BR", "ru-RU", "sv-SE",
                "th-TH", "tr-TR", "uk-UA", "vi-VN", "zh-CN",
            ]
        );
        for &locale in LocaleCode::ALL {
            assert_eq!(locale.as_str().parse(), Ok(locale));
            assert_eq!(locale.to_string(), locale.as_str());
        }
        assert_eq!("en-US".parse(), Ok(LocaleCode::EnUs));
        assert_eq!("pt-BR".parse(), Ok(LocaleCode::PtBr));

        for input in [
            "",
            "en",
            "pt",
            "en-us",
            "EN-US",
            "en_US",
            "pt-PT",
            "en-GB",
            "zh-TW",
            "zh-Hans-CN",
            "x-klingon",
            "de-DE ",
            " de-DE",
            "*",
            "und",
        ] {
            assert_eq!(
                input.parse::<LocaleCode>(),
                Err(InvalidLocaleCode),
                "{input:?}"
            );
        }
        assert!(!InvalidLocaleCode.to_string().is_empty());
    }
}
