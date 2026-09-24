use unicode_normalization::UnicodeNormalization;

pub fn normalize(input: &str) -> String {
    let lowered = input.nfkc().collect::<String>().to_lowercase();
    lowered.nfkc().collect::<String>().trim().to_owned()
}

#[cfg(test)]
pub(crate) mod tests {
    use proptest::prelude::*;
    use unicode_normalization::UnicodeNormalization;

    use super::normalize;

    const SPACING_DIACRITICS: [char; 16] = [
        '\u{00a8}', '\u{00af}', '\u{00b4}', '\u{00b8}', '\u{02d8}', '\u{02d9}', '\u{02da}',
        '\u{02db}', '\u{02dc}', '\u{02dd}', '\u{1fbd}', '\u{1fc1}', '\u{1fed}', '\u{203e}',
        '\u{fe70}', '\u{ffe3}',
    ];

    const WHITESPACE: [char; 8] = [
        ' ', '\u{00a0}', '\u{0085}', '\u{1680}', '\u{2003}', '\u{2028}', '\u{202f}', '\u{3000}',
    ];

    fn base() -> impl Strategy<Value = char> {
        prop_oneof![
            prop::char::range('A', 'Z'),
            prop::char::range('a', 'z'),
            prop::char::range('\u{00c0}', '\u{024f}'),
            prop::char::range('\u{0370}', '\u{03ff}'),
            prop::char::range('\u{0400}', '\u{052f}'),
            prop::char::range('\u{1e00}', '\u{1fff}'),
            prop::sample::select(vec![
                'J', 'I', '\u{0130}', '\u{1e9e}', '\u{03a3}', '\u{2126}', '\u{212a}', '\u{212b}',
            ]),
        ]
    }

    fn mark() -> impl Strategy<Value = char> {
        prop_oneof![
            4 => prop::char::range('\u{0300}', '\u{036f}'),
            1 => prop::sample::select(vec!['\u{030c}', '\u{0307}', '\u{0345}', '\u{0342}']),
        ]
    }

    pub(crate) fn unicode_text() -> impl Strategy<Value = String> {
        let segment = prop_oneof![
            3 => (base(), prop::collection::vec(mark(), 0..3))
                .prop_map(|(base, marks)| std::iter::once(base).chain(marks).collect::<String>()),
            2 => prop::sample::select(SPACING_DIACRITICS.to_vec()).prop_map(String::from),
            2 => prop::sample::select(WHITESPACE.to_vec()).prop_map(String::from),
            2 => prop_oneof![
                prop::char::range('\u{2000}', '\u{218f}'),
                prop::char::range('\u{2460}', '\u{24ff}'),
                prop::char::range('\u{3300}', '\u{33ff}'),
                prop::char::range('\u{fb00}', '\u{fb4f}'),
                prop::char::range('\u{fe10}', '\u{ffef}'),
                prop::char::range('\u{1d400}', '\u{1d7ff}'),
            ]
            .prop_map(String::from),
            1 => mark().prop_map(String::from),
            1 => prop::char::range('\u{0021}', '\u{007e}').prop_map(String::from),
            1 => any::<char>().prop_map(String::from),
        ];
        prop::collection::vec(segment, 0..10).prop_map(|segments| segments.concat())
    }

    #[test]
    fn unit_unicode_text_pools_hold_edge_classes() {
        for diacritic in SPACING_DIACRITICS {
            let expanded: String = diacritic.to_string().nfkc().collect();
            assert!(expanded.starts_with(' '), "{diacritic:?}");
        }
        assert!(WHITESPACE.iter().all(|c| c.is_whitespace()));
    }

    #[test]
    fn unit_normalize_regression_fixtures() {
        for (input, expected) in [
            ("Daniel@Example.COM", "daniel@example.com"),
            (
                "  \u{ff24}\u{ff41}\u{ff4e}\u{ff49}\u{ff45}\u{ff4c} ",
                "daniel",
            ),
            ("DANIEL", "daniel"),
            ("STRA\u{1e9e}E", "stra\u{00df}e"),
            ("stra\u{00df}e", "stra\u{00df}e"),
            ("STRASSE", "strasse"),
            ("J\u{030c}", "\u{01f0}"),
            ("\u{00a8}x", "\u{0308}x"),
            ("\u{2126}", "\u{03c9}"),
            ("\u{212a}elvin", "kelvin"),
            ("\u{fb01}le", "file"),
            ("\u{2460}", "1"),
            ("\u{3000}Mixed\u{00a0}Case\u{2003}", "mixed case"),
            ("\u{1d40f}\u{1d41a}\u{1d425}\u{1d426}\u{1d42b}", "palmr"),
            ("", ""),
        ] {
            let normalized = normalize(input);
            assert_eq!(normalized, expected, "{input:?}");
            assert_eq!(normalize(&normalized), normalized, "{input:?}");
        }
        assert_ne!(normalize("STRASSE"), normalize("STRA\u{1e9e}E"));
    }

    #[test]
    fn unit_normalize_idempotent_for_every_scalar() {
        for scalar in (0..=u32::from(char::MAX)).filter_map(char::from_u32) {
            for input in [
                scalar.to_string(),
                format!("{scalar}x"),
                format!("J{scalar}"),
            ] {
                let once = normalize(&input);
                assert_eq!(normalize(&once), once, "{input:?}");
            }
        }
    }
}
