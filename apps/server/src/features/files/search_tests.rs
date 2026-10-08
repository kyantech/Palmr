use proptest::prelude::*;

use crate::domain::error_code::ErrorCode;

use super::search::SearchTerms;

fn expression(raw: &str) -> Option<String> {
    SearchTerms::parse(raw)
        .unwrap()
        .expression()
        .map(str::to_owned)
}

fn assert_validation(raw: &str) {
    let error = SearchTerms::parse(raw).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ValidationError, "{raw:?}");
}

fn well_formed(expression: &str) -> bool {
    let chars: Vec<char> = expression.chars().collect();
    let mut at = 0;
    let mut groups = 0;
    while at < chars.len() {
        if groups > 0 {
            if chars[at] != ' ' {
                return false;
            }
            at += 1;
        }
        if chars.get(at) != Some(&'"') {
            return false;
        }
        at += 1;
        loop {
            match chars.get(at) {
                None => return false,
                Some('"') if chars.get(at + 1) == Some(&'"') => at += 2,
                Some('"') => break,
                Some(c) if c.is_control() => return false,
                Some(_) => at += 1,
            }
        }
        at += 1;
        if chars.get(at) != Some(&'*') {
            return false;
        }
        at += 1;
        groups += 1;
    }
    groups > 0
}

#[test]
fn unit_search_terms_quote_every_word_as_a_prefix_phrase() {
    assert_eq!(expression("report").as_deref(), Some("\"report\"*"));
    assert_eq!(
        expression("  Quarterly   REPORT ").as_deref(),
        Some("\"quarterly\"* \"report\"*")
    );
    assert_eq!(
        SearchTerms::parse("Quarterly \t REPORT").unwrap().text(),
        "quarterly report"
    );
    assert_eq!(
        expression("\u{ff32}\u{ff25}\u{ff30}\u{ff2f}\u{ff32}\u{ff34}").as_deref(),
        Some("\"report\"*"),
        "compatibility forms fold to the same phrase"
    );
    assert_eq!(expression("a a A").as_deref(), Some("\"a\"*"));
    assert_eq!(
        expression("report.pdf").as_deref(),
        Some("\"report.pdf\"*"),
        "punctuation inside a word stays in the phrase for the tokenizer to split"
    );
}

#[test]
fn unit_search_terms_defuse_fts_syntax() {
    assert_eq!(
        expression("a\" OR name:secret").as_deref(),
        Some("\"a\"\"\"* \"or\"* \"name:secret\"*")
    );
    assert_eq!(
        expression("NEAR(a b) NOT c").as_deref(),
        Some("\"near(a\"* \"b)\"* \"not\"* \"c\"*")
    );
    assert_eq!(
        expression("report* -secret ^x {name}: y").as_deref(),
        Some("\"report*\"* \"-secret\"* \"^x\"* \"{name}:\"* \"y\"*")
    );
    assert_eq!(
        expression("x' OR '1'='1; DROP TABLE files; --").as_deref(),
        Some("\"x'\"* \"or\"* \"'1'='1;\"* \"drop\"* \"table\"* \"files;\"*")
    );
}

#[test]
fn unit_search_terms_without_a_usable_token_have_no_expression() {
    for raw in [
        "???",
        "...",
        "--",
        "()",
        "\"\"",
        "%%",
        "__",
        "\u{fffd}\u{fffd}",
    ] {
        let terms = SearchTerms::parse(raw).unwrap();
        assert_eq!(terms.expression(), None, "{raw:?}");
        assert!(!terms.text().is_empty(), "{raw:?}");
    }
    assert_eq!(
        expression("???  report").as_deref(),
        Some("\"report\"*"),
        "a word with no token is dropped, not turned into a phrase"
    );
}

#[test]
fn unit_search_terms_reject_blank_input() {
    for raw in [
        "",
        " ",
        "   ",
        "\t\n",
        "\u{a0}\u{3000}",
        "\0",
        "\0\0",
        "\u{1}\u{2}\u{7f}",
        " \0 ",
    ] {
        assert_validation(raw);
    }
}

#[test]
fn unit_search_terms_split_on_control_characters() {
    assert_eq!(
        expression("ab\0cd").as_deref(),
        Some("\"ab\"* \"cd\"*"),
        "a NUL would cut an FTS string short, so it separates words"
    );
    assert_eq!(
        SearchTerms::parse("ab\u{7}\u{1b}cd").unwrap().text(),
        "ab cd"
    );
}

#[test]
fn unit_search_terms_are_bounded_by_the_query_length() {
    let longest = ('0'..='9')
        .chain('a'..='z')
        .chain('\u{3b1}'..='\u{3c9}')
        .map(String::from)
        .collect::<Vec<_>>()
        .join(" ");
    assert!((110..=128).contains(&longest.chars().count()));
    let expression = expression(&longest).unwrap();
    assert!(well_formed(&expression));
    assert!(expression.matches('*').count() >= 60);
}

fn hostile_text() -> impl Strategy<Value = String> {
    let special = prop::sample::select(vec![
        '"', '\'', '*', ':', '(', ')', '+', '-', '^', '{', '}', '~', ' ', '\0', '\t', '\u{7}', 'a',
        'b', 'o', 'r', '\u{e9}', '\u{62a5}', '\u{fffd}', '%', '_',
    ]);
    prop::collection::vec(prop_oneof![1 => any::<char>(), 4 => special], 0..200)
        .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    #[test]
    fn prop_search_terms_expression_is_always_well_formed(raw in hostile_text()) {
        if let Ok(terms) = SearchTerms::parse(&raw) {
            if let Some(expression) = terms.expression() {
                prop_assert!(well_formed(expression), "{expression:?}");
                prop_assert!(!expression.contains('\0'));
            }
            prop_assert!(!terms.text().is_empty());
            prop_assert!(!terms.text().chars().any(char::is_control));
        }
    }
}
