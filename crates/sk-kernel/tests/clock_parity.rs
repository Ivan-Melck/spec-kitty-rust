//! Differential clock contract parity with the pinned Python implementation.
//!
//! The clock owns distinct serialization contracts (ISO, second-precision ISO,
//! `Z` stamp, compact stamp, epoch, parse). Folding two together silently
//! changes on-disk bytes, so each one is compared byte-for-byte against the
//! Python producer under one frozen instant.

use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{TimeZone, Timelike};
use serde_json::Value;
use sk_kernel::clock::{Clock, FrozenClock};

const PINNED_PYTHON: &str = "/home/exedev/.local/share/uv/tools/spec-kitty-cli/bin/python";

fn python() -> Option<PathBuf> {
    let path = std::env::var_os("SK_KERNEL_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(PINNED_PYTHON));
    path.is_file().then_some(path)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn python_clock_contracts() -> Option<Value> {
    let python = python()?;
    let script = r#"
import json
import kernel.clock as clock
from datetime import UTC, datetime
fixed = datetime(2026, 7, 8, 12, 34, 56, 789123, tzinfo=UTC)
clock.DEFAULT_CLOCK = clock.FrozenClock(instant=fixed)
forms = [
    "2026-07-08T12:34:56.789123+00:00",
    "2026-07-08T12:34:56.789123Z",
    "2026-07-08T12:34:56+0000",
    "2026-07-08 12:34:56.789123+00:00",
    "2026-07-08T12:34:56-05:00",
]
parsed = []
for value in forms:
    instant = clock.parse_iso(value)
    parsed.append([int(instant.timestamp()), instant.microsecond * 1000])
epoch = clock.from_epoch(1783514096.123456)
print(json.dumps({
    "now_utc_iso": clock.now_utc_iso(),
    "now_utc_seconds": clock.now_utc_seconds(),
    "now_utc_stamp": clock.now_utc_stamp(),
    "now_utc_compact_stamp": clock.now_utc_compact_stamp(),
    "now_epoch": clock.now_epoch(),
    "parsed": parsed,
    "from_epoch": [int(epoch.timestamp()), epoch.microsecond * 1000],
}, sort_keys=True))
"#;
    let output = Command::new(python)
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root())
        .output()
        .expect("run python clock probe");
    assert!(
        output.status.success(),
        "python clock probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(serde_json::from_slice(&output.stdout).expect("python JSON"))
}

#[test]
fn clock_producers_match_the_python_baseline_under_one_frozen_instant() {
    let Some(expected) = python_clock_contracts() else {
        eprintln!("pinned Python oracle is not installed; skipping clock parity test");
        return;
    };
    let instant = chrono::Utc
        .with_ymd_and_hms(2026, 7, 8, 12, 34, 56)
        .single()
        .unwrap()
        .with_nanosecond(789_123_000)
        .unwrap();
    let clock = FrozenClock::new(instant);
    assert_eq!(expected["now_utc_iso"], clock.now_iso());
    assert_eq!(expected["now_utc_seconds"], clock.now_utc_seconds());
    assert_eq!(expected["now_utc_stamp"], clock.now_utc_stamp());
    assert_eq!(
        expected["now_utc_compact_stamp"],
        clock.now_utc_compact_stamp()
    );
    let expected_epoch = expected["now_epoch"].as_f64().unwrap();
    assert!((clock.now_epoch() - expected_epoch).abs() < 1e-6);

    let forms = [
        "2026-07-08T12:34:56.789123+00:00",
        "2026-07-08T12:34:56.789123Z",
        "2026-07-08T12:34:56+0000",
        "2026-07-08 12:34:56.789123+00:00",
        "2026-07-08T12:34:56-05:00",
    ];
    let expected_parsed = expected["parsed"].as_array().unwrap();
    for (value, expected) in forms.iter().zip(expected_parsed) {
        let parsed = sk_kernel::clock::parse_iso(value).expect(value);
        assert_eq!(parsed.timestamp(), expected[0].as_i64().unwrap(), "{value}");
        assert_eq!(
            parsed.timestamp_subsec_nanos(),
            expected[1].as_u64().unwrap() as u32,
            "{value}"
        );
    }
    let from_epoch = sk_kernel::clock::from_epoch(1783514096.123456).unwrap();
    assert_eq!(
        from_epoch.timestamp(),
        expected["from_epoch"][0].as_i64().unwrap()
    );
    assert_eq!(
        from_epoch.timestamp_subsec_nanos(),
        expected["from_epoch"][1].as_u64().unwrap() as u32
    );
}

#[test]
fn the_distinct_producer_contracts_are_actually_distinct() {
    let instant = chrono::Utc
        .with_ymd_and_hms(2026, 7, 8, 12, 34, 56)
        .single()
        .unwrap()
        .with_nanosecond(789_123_000)
        .unwrap();
    let clock = FrozenClock::new(instant);
    let iso = clock.now_iso();
    let seconds = clock.now_utc_seconds();
    let stamp = clock.now_utc_stamp();
    let compact = clock.now_utc_compact_stamp();
    assert_ne!(iso, seconds);
    assert_ne!(seconds, stamp);
    assert_ne!(stamp, compact);
    assert!(iso.ends_with("+00:00"));
    assert!(seconds.ends_with("+00:00"));
    assert!(stamp.ends_with('Z'));
    assert!(
        compact
            .chars()
            .all(|character| character.is_ascii_digit() || matches!(character, 'T' | 'Z'))
    );
}
