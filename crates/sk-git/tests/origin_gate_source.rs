//! Source-first parity for the origin-freshness GATE (policy, codes, text).
//!
//! The *facts* (`behind`/`ahead`/state) come from `GitRepository::check_branch_freshness`,
//! which `remote.rs` already binds to `ls-remote`; this file binds the *policy*
//! half: `--origin-check` resolution, the `off` warning, and
//! `enforce_merge_gate`'s warnings, refusal codes, headlines and remedy text.
//!
//! Order: the PINNED Source module runs and its output is asserted FIRST
//! (lock-exact interpreter through `sk_kernel`'s oracle seam), then the Native
//! functions are called with the same facts and must agree field by field.
//! The facts here are the *inputs* of a policy function on both sides — the
//! Native gate is handed `OriginFacts`, a plain data view; nothing claims these
//! were observed in this repository.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::process::Command;

use sk_git::{
    FreshnessState, ORIGIN_COMPARE_FAILED, ORIGIN_LANE_STALE, ORIGIN_REMOTE_AMBIGUOUS,
    ORIGIN_STATUS_STALE, ORIGIN_UNREACHABLE, OriginCheckMode, OriginFacts, enforce_merge_gate,
    origin_not_checked_warning, resolve_origin_check_mode,
};

const ORACLE: &str = r#"
import json, os
from specify_cli.git.origin_freshness import (
    FreshnessState,
    FreshnessVerdict,
    OriginCheckMode,
    OriginCheckSetting,
    OriginFreshnessRefused,
    enforce_merge_gate,
    origin_not_checked_warning,
    resolve_origin_check_mode,
)

def verdict(branch, remote, state, behind=0, ahead=0, detail=None, timeout=None):
    return FreshnessVerdict(
        branch=branch,
        remote=remote,
        state=getattr(FreshnessState, state),
        behind=behind,
        ahead=ahead,
        detail=detail,
        timeout_seconds=timeout,
    )

MODE_CASES = [
    ("flag-warn", ("warn", None)),
    ("flag-off", ("off", None)),
    ("flag-enforce", ("enforce", None)),
    ("env-off", (None, "off")),
    ("env-invalid", (None, "sometimes")),
    ("default", (None, None)),
    ("flag-beats-env", ("warn", "off")),
]

GATE_CASES = [
    ("clean", None, [], False),
    ("evidence-behind", verdict("main", "origin", "BEHIND", 2, 0), [], False),
    ("evidence-diverged-merge-record", verdict("main", "origin", "DIVERGED", 1, 3), [], True),
    ("evidence-local-missing", verdict("main", "origin", "LOCAL_MISSING"), [], False),
    ("lane-stale", None, [verdict("kitty/lane-a", "origin", "BEHIND", 4, 0)], False),
    ("lane-diverged", None, [verdict("kitty/lane-a", "origin", "DIVERGED", 1, 1)], False),
    ("lane-local-missing", None, [verdict("kitty/lane-a", "origin", "LOCAL_MISSING")], False),
    ("unreachable", None, [verdict("kitty/lane-a", "origin", "UNREACHABLE", detail="ssh: connect refused")], False),
    ("unreachable-timeout", None, [verdict("kitty/lane-a", "origin", "UNREACHABLE", detail="timed out", timeout=10.0)], False),
    ("ambiguous", verdict("main", None, "REMOTE_AMBIGUOUS", detail="a, b"), [], False),
    ("compare-failed", None, [verdict("kitty/lane-a", "origin", "COMPARE_FAILED", detail="missing object")], False),
    ("remote-missing-note", verdict("main", "origin", "REMOTE_MISSING", detail="origin/main was seen here before, but origin no longer lists main"), [], False),
    ("no-remote-note", verdict("main", None, "NO_REMOTE"), [], False),
]

out = {"modes": [], "gates": []}
for name, (flag, env) in MODE_CASES:
    os.environ.pop("SPEC_KITTY_ORIGIN_CHECK", None)
    if env is not None:
        os.environ["SPEC_KITTY_ORIGIN_CHECK"] = env
    setting = resolve_origin_check_mode(flag)
    out["modes"].append(
        {
            "name": name,
            "mode": setting.mode.value,
            "source": setting.source,
            "warning": setting.warning,
            "off_text": origin_not_checked_warning(setting) if setting.mode is OriginCheckMode.OFF else None,
        }
    )
os.environ.pop("SPEC_KITTY_ORIGIN_CHECK", None)

for name, evidence, lanes, merge_record in GATE_CASES:
    for mode_name, mode in [("enforce", OriginCheckMode.ENFORCE), ("warn", OriginCheckMode.WARN)]:
        setting = OriginCheckSetting(mode, "flag")
        try:
            warnings = enforce_merge_gate(
                evidence=evidence,
                lanes=lanes,
                setting=setting,
                merge_record_exists=merge_record,
                evidence_checkout=None,
            )
        except OriginFreshnessRefused as exc:
            out["gates"].append(
                {
                    "name": name,
                    "mode": mode_name,
                    "error": {
                        "code": exc.error_code,
                        "codes": list(exc.error_codes),
                        "headline": exc.headline,
                        "message": str(exc),
                    },
                }
            )
        else:
            out["gates"].append({"name": name, "mode": mode_name, "warnings": warnings})

print(json.dumps(out, sort_keys=True))
"#;

fn source_output() -> serde_json::Value {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let script = root.join("target/origin-gate-oracle.py");
    std::fs::write(&script, ORACLE).expect("write the oracle script");
    let output = Command::new(oracle::python())
        .arg(&script)
        .current_dir(&root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("target/origin-gate-home"))
        .env(
            "PYTHONPATH",
            format!("{}:{}", root.display(), root.join("src").display()),
        )
        .env("SPEC_KITTY_PACKS_ROOT", root.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run the pinned Source origin gate");
    assert!(
        output.status.success(),
        "the pinned Source origin gate failed (a harness/premise failure, not a business red):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("oracle json")
}

fn facts(
    branch: &str,
    remote: Option<&str>,
    state: FreshnessState,
    behind: u64,
    ahead: u64,
    detail: Option<&str>,
    timeout_seconds: Option<f64>,
) -> OriginFacts {
    OriginFacts {
        branch: branch.to_owned(),
        remote: remote.map(str::to_owned),
        state,
        behind,
        ahead,
        detail: detail.map(str::to_owned),
        timeout_seconds,
    }
}

#[test]
fn origin_check_mode_resolution_matches_the_pinned_source() {
    let source = source_output();
    let modes = source["modes"].as_array().expect("modes");
    assert_eq!(modes.len(), 7);
    for row in modes {
        let name = row["name"].as_str().expect("name");
        let (flag, environment) = match name {
            "flag-warn" => (Some("warn"), None),
            "flag-off" => (Some("off"), None),
            "flag-enforce" => (Some("enforce"), None),
            "env-off" => (None, Some("off")),
            "env-invalid" => (None, Some("sometimes")),
            "default" => (None, None),
            "flag-beats-env" => (Some("warn"), Some("off")),
            other => panic!("unexpected mode case {other}"),
        };
        let setting = resolve_origin_check_mode(flag, environment).expect("a valid flag value");
        assert_eq!(
            setting.mode.as_str(),
            row["mode"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(setting.source, row["source"].as_str().unwrap(), "{name}");
        assert_eq!(
            setting.warning.as_deref(),
            row["warning"].as_str(),
            "{name} warning"
        );
        if setting.mode == OriginCheckMode::Off {
            assert_eq!(
                origin_not_checked_warning(&setting),
                row["off_text"].as_str().expect("off text"),
                "{name} off warning"
            );
        } else {
            assert!(row["off_text"].is_null(), "{name} has no off text");
        }
    }
}

#[test]
fn merge_gate_policy_and_text_match_the_pinned_source() {
    let source = source_output();
    let gates = source["gates"].as_array().expect("gates");
    assert_eq!(gates.len(), 26);
    for row in gates {
        let name = row["name"].as_str().expect("name");
        let mode = match row["mode"].as_str().expect("mode") {
            "enforce" => OriginCheckMode::Enforce,
            "warn" => OriginCheckMode::Warn,
            other => panic!("unexpected mode {other}"),
        };
        let label = format!("{name}/{}", row["mode"].as_str().unwrap());
        let (evidence, lanes, merge_record) = match name {
            "clean" => (None, Vec::new(), false),
            "evidence-behind" => (
                Some(facts(
                    "main",
                    Some("origin"),
                    FreshnessState::Behind,
                    2,
                    0,
                    None,
                    None,
                )),
                Vec::new(),
                false,
            ),
            "evidence-diverged-merge-record" => (
                Some(facts(
                    "main",
                    Some("origin"),
                    FreshnessState::Diverged,
                    1,
                    3,
                    None,
                    None,
                )),
                Vec::new(),
                true,
            ),
            "evidence-local-missing" => (
                Some(facts(
                    "main",
                    Some("origin"),
                    FreshnessState::LocalMissing,
                    0,
                    0,
                    None,
                    None,
                )),
                Vec::new(),
                false,
            ),
            "lane-stale" => (
                None,
                vec![facts(
                    "kitty/lane-a",
                    Some("origin"),
                    FreshnessState::Behind,
                    4,
                    0,
                    None,
                    None,
                )],
                false,
            ),
            "lane-diverged" => (
                None,
                vec![facts(
                    "kitty/lane-a",
                    Some("origin"),
                    FreshnessState::Diverged,
                    1,
                    1,
                    None,
                    None,
                )],
                false,
            ),
            "lane-local-missing" => (
                None,
                vec![facts(
                    "kitty/lane-a",
                    Some("origin"),
                    FreshnessState::LocalMissing,
                    0,
                    0,
                    None,
                    None,
                )],
                false,
            ),
            "unreachable" => (
                None,
                vec![facts(
                    "kitty/lane-a",
                    Some("origin"),
                    FreshnessState::Unreachable,
                    0,
                    0,
                    Some("ssh: connect refused"),
                    None,
                )],
                false,
            ),
            "unreachable-timeout" => (
                None,
                vec![facts(
                    "kitty/lane-a",
                    Some("origin"),
                    FreshnessState::Unreachable,
                    0,
                    0,
                    Some("timed out"),
                    Some(10.0),
                )],
                false,
            ),
            "ambiguous" => (
                Some(facts(
                    "main",
                    None,
                    FreshnessState::RemoteAmbiguous,
                    0,
                    0,
                    Some("a, b"),
                    None,
                )),
                Vec::new(),
                false,
            ),
            "compare-failed" => (
                None,
                vec![facts(
                    "kitty/lane-a",
                    Some("origin"),
                    FreshnessState::CompareFailed,
                    0,
                    0,
                    Some("missing object"),
                    None,
                )],
                false,
            ),
            "remote-missing-note" => (
                Some(facts(
                    "main",
                    Some("origin"),
                    FreshnessState::RemoteMissing,
                    0,
                    0,
                    Some("origin/main was seen here before, but origin no longer lists main"),
                    None,
                )),
                Vec::new(),
                false,
            ),
            "no-remote-note" => (
                Some(facts(
                    "main",
                    None,
                    FreshnessState::NoRemote,
                    0,
                    0,
                    None,
                    None,
                )),
                Vec::new(),
                false,
            ),
            other => panic!("unexpected gate case {other}"),
        };

        let setting = sk_git::OriginCheckSetting {
            mode,
            source: "flag",
            warning: None,
        };
        let native = enforce_merge_gate(evidence.as_ref(), &lanes, &setting, merge_record, None);
        match (&row["error"], native) {
            (serde_json::Value::Null, Ok(warnings)) => {
                let expected: Vec<String> = row["warnings"]
                    .as_array()
                    .expect("warnings")
                    .iter()
                    .map(|value| value.as_str().expect("warning").to_owned())
                    .collect();
                assert_eq!(warnings, expected, "{label} warnings");
            }
            (serde_json::Value::Null, Err(refused)) => {
                panic!(
                    "{label}: Native refused where Source warned: {}",
                    refused.message
                )
            }
            (error, Ok(warnings)) => {
                panic!("{label}: Source refused with {error} but Native warned: {warnings:?}")
            }
            (error, Err(refused)) => {
                assert_eq!(
                    refused.error_code,
                    error["code"].as_str().expect("code"),
                    "{label} first code"
                );
                let codes: Vec<&str> = error["codes"]
                    .as_array()
                    .expect("codes")
                    .iter()
                    .map(|value| value.as_str().expect("code"))
                    .collect();
                assert_eq!(refused.error_codes, codes, "{label} codes");
                assert_eq!(
                    refused.headline,
                    error["headline"].as_str().expect("headline"),
                    "{label} headline"
                );
                assert_eq!(
                    refused.message,
                    error["message"].as_str().expect("message"),
                    "{label} operator text"
                );
            }
        }
    }
}

#[test]
fn every_refusal_code_is_the_source_vocabulary() {
    // The codes are constants, not free strings; assert the exact Source spells.
    assert_eq!(ORIGIN_STATUS_STALE, "ORIGIN_STATUS_STALE");
    assert_eq!(ORIGIN_LANE_STALE, "ORIGIN_LANE_STALE");
    assert_eq!(ORIGIN_UNREACHABLE, "ORIGIN_UNREACHABLE");
    assert_eq!(ORIGIN_REMOTE_AMBIGUOUS, "ORIGIN_REMOTE_AMBIGUOUS");
    assert_eq!(ORIGIN_COMPARE_FAILED, "ORIGIN_COMPARE_FAILED");
}
