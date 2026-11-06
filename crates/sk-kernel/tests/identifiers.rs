use std::str::FromStr;

use proptest::prelude::*;
use sk_kernel::{KernelError, MissionSlug, WpId};

fn parse_slug(value: &str) -> Result<MissionSlug, KernelError> {
    MissionSlug::from_str(value)
}

fn parse_wp(value: &str) -> Result<WpId, KernelError> {
    WpId::from_str(value)
}

#[test]
fn mission_slug_accepts_real_world_shapes() {
    for value in [
        "001-architectural-integrity-standard",
        "058-feature",
        "01KZ6G6H4R8Y9J2N4Q7V3B6M1C",
        "mission.with_interior-dots",
        "a",
        "A0",
        "0",
        "a_b-c.d",
    ] {
        let slug = parse_slug(value).unwrap_or_else(|error| panic!("{value:?}: {error}"));
        assert_eq!(slug.as_str(), value);
        assert_eq!(slug.to_string(), value);
    }
}

#[test]
fn mission_slug_rejects_unsafe_or_non_ascii_input() {
    for value in [
        "",
        " ",
        "  slug",
        "slug ",
        "\tslug",
        "slug\n",
        ".",
        "..",
        ".hidden",
        "a..b",
        "..foo",
        "foo..",
        "../escape",
        "a/b",
        "a\\b",
        "/absolute",
        "\\absolute",
        "C:\\escape",
        "C:/escape",
        "\\\\server\\share",
        "~",
        "slug~",
        "slug:name",
        "slug*name",
        "slug?name",
        "café",
        "slug\u{00e9}",
        "slug\u{200b}",
        "sлug",
        "ｗｐ01",
        "slug\u{0000}name",
        "slug name",
        "-leading",
        "_leading",
    ] {
        assert!(
            parse_slug(value).is_err(),
            "unsafe slug unexpectedly accepted: {value:?}"
        );
    }
}

#[test]
fn wp_id_accepts_canonical_and_normalizes_safe_input() {
    for (input, canonical) in [
        ("WP01", "WP01"),
        ("WP1", "WP1"),
        ("WP001", "WP001"),
        ("wp01", "WP01"),
        ("wP01", "WP01"),
        (" WP01 ", "WP01"),
        ("\twp01\n", "WP01"),
    ] {
        let id = parse_wp(input).unwrap_or_else(|error| panic!("{input:?}: {error}"));
        assert_eq!(id.as_str(), canonical, "input {input:?}");
        assert_eq!(id.to_string(), canonical, "input {input:?}");
    }
}

#[test]
fn wp_id_rejects_non_canonical_or_unsafe_input() {
    for value in [
        "",
        " ",
        "WP",
        "WP-01",
        "WP_01",
        "WP.01",
        "WP01-some-slug",
        "01",
        "wp",
        "W01",
        "WP 01",
        "WP０１",
        "WP٠١",
        "WP01/../02",
        "WP01\\02",
        "WP01\u{0000}",
    ] {
        assert!(
            parse_wp(value).is_err(),
            "unsafe work package id unexpectedly accepted: {value:?}"
        );
    }
}

#[test]
fn identifiers_are_ordered_and_hashable() {
    let mut ids = [
        parse_wp("WP03").unwrap(),
        parse_wp("WP01").unwrap(),
        parse_wp("WP02").unwrap(),
    ];
    ids.sort();
    assert_eq!(
        ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["WP01", "WP02", "WP03"]
    );

    let mut slugs = [
        parse_slug("zeta").unwrap(),
        parse_slug("alpha").unwrap(),
        parse_slug("beta").unwrap(),
    ];
    slugs.sort();
    assert_eq!(
        slugs.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["alpha", "beta", "zeta"]
    );
}

#[test]
fn serde_round_trips_and_revalidates() {
    let slug = parse_slug("058-feature").unwrap();
    let encoded = serde_json::to_string(&slug).unwrap();
    assert_eq!(encoded, "\"058-feature\"");
    assert_eq!(serde_json::from_str::<MissionSlug>(&encoded).unwrap(), slug);

    let wp = parse_wp("wp07").unwrap();
    let encoded = serde_json::to_string(&wp).unwrap();
    assert_eq!(encoded, "\"WP07\"");
    assert_eq!(serde_json::from_str::<WpId>(&encoded).unwrap(), wp);

    for invalid in [
        "\"\"",
        "\"..\"",
        "\"a/../b\"",
        "\"caf\\u00e9\"",
        "\"WP01/02\"",
        "42",
        "null",
        "{}",
    ] {
        assert!(
            serde_json::from_str::<MissionSlug>(invalid).is_err(),
            "invalid mission slug json accepted: {invalid}"
        );
        assert!(
            serde_json::from_str::<WpId>(invalid).is_err(),
            "invalid wp id json accepted: {invalid}"
        );
    }
}

proptest! {
    #[test]
    fn parsed_mission_slugs_are_single_safe_ascii_segments(value in any::<String>()) {
        if let Ok(slug) = parse_slug(&value) {
            let text = slug.as_str();
            prop_assert!(!text.is_empty());
            prop_assert!(text.is_ascii());
            prop_assert!(!text.starts_with('.'));
            prop_assert!(!text.contains(".."));
            prop_assert!(!text.contains('/'));
            prop_assert!(!text.contains('\\'));
            prop_assert!(text.as_bytes()[0].is_ascii_alphanumeric());
            let alphabet_ok = text.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
            });
            prop_assert!(alphabet_ok);
            prop_assert_eq!(text.parse::<MissionSlug>().unwrap(), slug);
        }
    }

    #[test]
    fn parsed_wp_ids_are_canonical_ascii_and_idempotent(value in any::<String>()) {
        if let Ok(wp) = parse_wp(&value) {
            let text = wp.as_str();
            prop_assert!(text.starts_with("WP"));
            prop_assert!(text.len() > 2);
            prop_assert!(text[2..].bytes().all(|byte| byte.is_ascii_digit()));
            prop_assert!(text.is_ascii());
            prop_assert_eq!(text.parse::<WpId>().unwrap(), wp);
        }
    }

    #[test]
    fn valid_mission_slug_alphabet_round_trips(value in "[A-Za-z0-9][A-Za-z0-9._-]{0,80}") {
        prop_assume!(!value.contains(".."));
        let slug = parse_slug(&value).expect("generated alphabet must be a valid slug");
        prop_assert_eq!(slug.as_str(), value.as_str());
        prop_assert_eq!(serde_json::to_string(&slug).unwrap(), format!("\"{value}\""));
    }

    #[test]
    fn valid_wp_alphabet_round_trips(digits in "[0-9]{1,8}") {
        let canonical = format!("WP{digits}");
        let wp = parse_wp(&canonical).expect("generated WP id must be valid");
        prop_assert_eq!(wp.as_str(), canonical.as_str());
        prop_assert_eq!(serde_json::to_string(&wp).unwrap(), format!("\"{canonical}\""));
    }
}
