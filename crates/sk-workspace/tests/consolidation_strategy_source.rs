//! Source-first matrix for the consolidation strategy resolution.
//!
//! The pinned Source resolves the mission→target strategy with an explicit
//! precedence (`_resolve_effective_merge_strategy`) and refuses a strategy flip
//! across a resume; `.kittify/config.yaml`'s `merge.strategy` is validated, never
//! silently defaulted (`load_merge_config`). Source runs first and its answer is
//! the expectation.

#[path = "support/oracle.rs"]
mod oracle_support;

use std::path::Path;

const RESOLVE_SCRIPT: &str = r#"
import json
from specify_cli.consolidation.config import MergeStrategy
from specify_cli.cli.commands.consolidate import (
    _resolve_effective_merge_strategy,
    MergeStrategyFlipError,
)

cases = [
    (None, None, None, False),
    ("merge", None, None, False),
    (None, None, "rebase", False),
    (None, "merge", "rebase", True),
    ("merge", "squash", None, True),
    ("squash", "squash", None, True),
    (None, "squash", "merge", True),
    ("rebase", None, "merge", False),
]
out = []
for explicit, persisted, config, is_resume in cases:
    try:
        value = _resolve_effective_merge_strategy(
            explicit=MergeStrategy(explicit) if explicit else None,
            persisted=MergeStrategy(persisted) if persisted else None,
            config=MergeStrategy(config) if config else None,
            is_resume=is_resume,
        )
        out.append({"strategy": str(value.value)})
    except MergeStrategyFlipError:
        out.append({"error": "MERGE_STRATEGY_FLIP"})
print(json.dumps(out))
"#;

const CONFIG_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.consolidation.config import load_merge_config, ConfigError

texts = [
    "merge:\n  strategy: merge\n",
    "merge:\n  strategy: squash\n",
    "merge:\n  strategy: octopus\n",
    "merge:\n  strategy: 3\n",
    "other: 1\n",
    "[unclosed\n",
]
out = []
for index, text in enumerate(texts):
    root = Path(sys.argv[1]) / f"case{index}"
    (root / ".kittify").mkdir(parents=True)
    (root / ".kittify" / "config.yaml").write_text(text, encoding="utf-8")
    try:
        config = load_merge_config(root)
        out.append({"strategy": str(config.strategy.value) if config.strategy else None})
    except ConfigError:
        out.append({"error": "MERGE_STRATEGY_INVALID"})
print(json.dumps(out))
"#;

const CONFIG_TEXTS: &[&str] = &[
    "merge:\n  strategy: merge\n",
    "merge:\n  strategy: squash\n",
    "merge:\n  strategy: octopus\n",
    "merge:\n  strategy: 3\n",
    "other: 1\n",
    "[unclosed\n",
];

/// One `(explicit, persisted, config, is_resume)` resolution case.
type StrategyCase = (
    Option<sk_workspace::MergeStrategy>,
    Option<sk_workspace::MergeStrategy>,
    Option<sk_workspace::MergeStrategy>,
    bool,
);

#[test]
fn strategy_precedence_and_flip_refusal_match_source() {
    let source = oracle_support::run_json(RESOLVE_SCRIPT, &[]);
    let source = source.as_array().expect("oracle list");
    let cases: [StrategyCase; 8] = [
        (None, None, None, false),
        (Some(sk_workspace::MergeStrategy::Merge), None, None, false),
        (None, None, Some(sk_workspace::MergeStrategy::Rebase), false),
        (
            None,
            Some(sk_workspace::MergeStrategy::Merge),
            Some(sk_workspace::MergeStrategy::Rebase),
            true,
        ),
        (
            Some(sk_workspace::MergeStrategy::Merge),
            Some(sk_workspace::MergeStrategy::Squash),
            None,
            true,
        ),
        (
            Some(sk_workspace::MergeStrategy::Squash),
            Some(sk_workspace::MergeStrategy::Squash),
            None,
            true,
        ),
        (
            None,
            Some(sk_workspace::MergeStrategy::Squash),
            Some(sk_workspace::MergeStrategy::Merge),
            true,
        ),
        (
            Some(sk_workspace::MergeStrategy::Rebase),
            None,
            Some(sk_workspace::MergeStrategy::Merge),
            false,
        ),
    ];
    assert_eq!(source.len(), cases.len());

    for (index, (explicit, persisted, config, is_resume)) in cases.into_iter().enumerate() {
        let expected = &source[index];
        let native =
            sk_workspace::resolve_effective_strategy(explicit, persisted, config, is_resume);
        match expected.get("error").and_then(|value| value.as_str()) {
            Some(code) => {
                let error = native.expect_err("Source refused this case");
                assert!(
                    error.to_string().contains(code),
                    "case {index}: Source {code}, native {error}"
                );
            }
            None => {
                let strategy = native.unwrap_or_else(|error| {
                    panic!("case {index}: Source accepted, native refused: {error}")
                });
                assert_eq!(
                    strategy.as_str(),
                    expected["strategy"].as_str().unwrap(),
                    "case {index}"
                );
            }
        }
    }
}

#[test]
fn configured_strategy_loading_matches_source() {
    let temp = tempfile::tempdir().expect("temp");
    let source = oracle_support::run_json(CONFIG_SCRIPT, &[temp.path().to_str().unwrap()]);
    let source = source.as_array().expect("oracle list");
    assert_eq!(source.len(), CONFIG_TEXTS.len());

    for (index, text) in CONFIG_TEXTS.iter().enumerate() {
        let root = temp.path().join(format!("native{index}"));
        std::fs::create_dir_all(root.join(".kittify")).unwrap();
        std::fs::write(root.join(".kittify").join("config.yaml"), text).unwrap();
        let expected = &source[index];
        let native = sk_workspace::load_merge_config(Path::new(&root));
        match expected.get("error").and_then(|value| value.as_str()) {
            Some(code) => {
                let error = native.expect_err("Source refused this config");
                assert!(
                    error.to_string().contains(code),
                    "case {index}: Source {code}, native {error}"
                );
            }
            None => {
                let strategy = native.unwrap_or_else(|error| {
                    panic!("case {index}: Source accepted, native refused: {error}")
                });
                let expected_strategy = expected["strategy"].as_str();
                assert_eq!(
                    strategy.map(sk_workspace::MergeStrategy::as_str),
                    expected_strategy,
                    "case {index}"
                );
            }
        }
    }
}
