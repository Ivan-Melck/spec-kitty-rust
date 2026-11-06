//! Source-first protection-string rendering controls.
//!
//! The reference builds its protected-branch set with
//! `frozenset(str(b) for b in raw_value)`, so a non-scalar YAML item is named by
//! CPython's `repr`. The native side originally carried its own formatter, which
//! disagreed on non-printables (NBSP/ZWSP were emitted raw), NUL, and quote
//! choice. This test makes the pinned Source answer the expectation: both
//! implementations read the same real `.kittify/config.yaml`, Source runs first,
//! and the native set must match exactly.

#[path = "support/oracle.rs"]
mod oracle_support;

use std::path::Path;

const RESOLVE_SCRIPT: &str = r#"
import json, sys
from pathlib import Path
from specify_cli.git.protection_policy import ProtectionPolicy

policy = ProtectionPolicy.resolve(Path(sys.argv[1]))
print(json.dumps(sorted(policy.protected_branches)))
"#;

const YAML_CASES: &[&str] = &[
    // plain scalars are `str()`-rendered unchanged
    "protection:\n  protected_branches:\n    - plain-branch\n",
    // non-printables: CPython repr escapes these
    "protection:\n  protected_branches:\n    - \"nbsp\\u00a0branch\"\n",
    "protection:\n  protected_branches:\n    - \"zwsp\\u200bbranch\"\n",
    "protection:\n  protected_branches:\n    - \"nul\\0branch\"\n",
    // quotes, backslash, tab, newline
    "protection:\n  protected_branches:\n    - \"quote'and\\\"double\"\n",
    "protection:\n  protected_branches:\n    - \"back\\\\slash\"\n",
    "protection:\n  protected_branches:\n    - \"tab\\there\"\n",
    // printable non-ASCII stays literal
    "protection:\n  protected_branches:\n    - \"caf\\u00e9\"\n",
    "protection:\n  protected_branches:\n    - \"astral \\U0001F600\"\n",
    // a list item: `str(list)` is `repr(list)`
    "protection:\n  protected_branches:\n    - [inner, 1, 1.5, true, null]\n",
    // a mapping item: `str(dict)` is `repr(dict)`
    "protection:\n  protected_branches:\n    - {k: v, n: 2}\n",
    // an explicit empty list unprotects everything
    "protection:\n  protected_branches: []\n",
];

fn write_config(root: &Path, yaml: &str) {
    let directory = root.join(".kittify");
    std::fs::create_dir_all(&directory).expect("kittify dir");
    std::fs::write(directory.join("config.yaml"), yaml).expect("config");
}

#[test]
fn protection_names_match_the_source_repr_for_every_valid_string_shape() {
    for (index, yaml) in YAML_CASES.iter().enumerate() {
        let temp = tempfile::tempdir().expect("temp");
        write_config(temp.path(), yaml);

        // Source FIRST: the expected protected set, from the real resolver.
        let expected = oracle_support::run_json(RESOLVE_SCRIPT, &[temp.path().to_str().unwrap()]);
        let expected: Vec<String> = expected
            .as_array()
            .expect("oracle returned a list")
            .iter()
            .map(|value| value.as_str().expect("name").to_owned())
            .collect();

        // Native second.
        let policy = sk_workspace::ProtectionPolicy::resolve(temp.path())
            .unwrap_or_else(|error| panic!("case {index} refused: {error}"));
        let native: Vec<String> = policy.protected_branches().iter().cloned().collect();

        assert_eq!(native, expected, "case {index}: YAML {yaml:?}");
    }
}

#[test]
fn a_malformed_protection_block_still_fails_closed_for_every_shape() {
    for yaml in [
        "protection:\n  protected_branches: main\n",
        "protection:\n  protected_branches: 3\n",
        "[unclosed\n",
    ] {
        let temp = tempfile::tempdir().expect("temp");
        write_config(temp.path(), yaml);
        assert!(
            sk_workspace::ProtectionPolicy::resolve(temp.path()).is_err(),
            "malformed config was accepted: {yaml:?}"
        );
    }
}
