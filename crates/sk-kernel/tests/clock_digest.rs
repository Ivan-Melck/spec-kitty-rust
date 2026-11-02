//! Acceptance tests for the canonical clock and content-digest helpers.

use chrono::{TimeZone, Utc};
use sk_kernel::clock::{
    Clock, FrozenClock, SystemClock, UTC_SECOND_TIMESTAMP_FORMAT, from_epoch,
    now_utc_compact_stamp, now_utc_iso, now_utc_seconds, now_utc_stamp, parse_iso, parse_stamp,
};
use sk_kernel::content_digest::{sha256_digest, sha256_hex};

#[test]
fn sha256_digest_uses_the_documented_provenance_prefix() {
    assert_eq!(
        sha256_digest(b""),
        "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_digest(b"abc"),
        format!("sha256:{}", sha256_hex(b"abc"))
    );
}

#[test]
fn digest_is_over_exact_bytes_and_does_not_normalise() {
    assert_ne!(sha256_digest(b"line\n"), sha256_digest(b"line\r\n"));
    assert_ne!(sha256_digest(b"line\n"), sha256_digest(b"line"));
}

#[test]
fn frozen_clock_freezes_every_producer_shape() {
    let instant = Utc.with_ymd_and_hms(2026, 11, 2, 13, 14, 15).unwrap();
    let clock = FrozenClock::new(instant);
    assert_eq!(clock.now(), instant);
    assert_eq!(clock.now_iso(), "2026-11-02T13:14:15+00:00");
    assert_eq!(clock.now_utc_stamp(), "2026-11-02T13:14:15Z");
    assert_eq!(clock.now_utc_compact_stamp(), "20261102T131415Z");
    assert_eq!(clock.now_utc_seconds(), "2026-11-02T13:14:15+00:00");
    assert_eq!(clock.now_epoch(), instant.timestamp() as f64);
}

#[test]
fn iso_producer_matches_python_isoformat_shapes() {
    let instant = Utc.with_ymd_and_hms(2026, 11, 2, 13, 14, 15).unwrap();
    assert_eq!(
        FrozenClock::new(instant).now_iso(),
        "2026-11-02T13:14:15+00:00"
    );
    let fractional = instant + chrono::Duration::microseconds(123_456);
    assert_eq!(
        FrozenClock::new(fractional).now_iso(),
        "2026-11-02T13:14:15.123456+00:00"
    );
}

#[test]
fn system_clock_produces_parseable_values() {
    let clock = SystemClock;
    assert!(parse_iso(&clock.now_iso()).is_ok());
    assert!(parse_stamp(&clock.now_utc_stamp(), UTC_SECOND_TIMESTAMP_FORMAT).is_ok());
    assert_eq!(now_utc_stamp().len(), 20);
    assert_eq!(now_utc_compact_stamp().len(), 16);
    assert!(now_utc_iso().ends_with("+00:00"));
    assert!(now_utc_seconds().ends_with("+00:00"));
}

#[test]
fn parse_and_format_are_inverses() {
    let text = "2026-11-02T13:14:15Z";
    let parsed = parse_stamp(text, UTC_SECOND_TIMESTAMP_FORMAT).unwrap();
    assert_eq!(parsed.format(UTC_SECOND_TIMESTAMP_FORMAT).to_string(), text);
}

#[test]
fn from_epoch_round_trips_negative_and_fractional_instants() {
    for value in [0.0_f64, 1_700_000_000.5, -1.25] {
        let parsed = from_epoch(value).unwrap();
        assert!(
            (parsed.timestamp() as f64 + f64::from(parsed.timestamp_subsec_nanos()) / 1e9 - value)
                .abs()
                < 1e-6
        );
    }
}

#[test]
fn invalid_timestamp_text_is_a_typed_error() {
    assert!(parse_iso("not a timestamp").is_err());
    assert!(parse_stamp("2026-11-02", UTC_SECOND_TIMESTAMP_FORMAT).is_err());
    assert!(from_epoch(f64::NAN).is_err());
}
