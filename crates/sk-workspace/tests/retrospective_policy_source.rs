//! Source-first parity for the CONFIGURED retrospective policy resolver.
//!
//! `specify_cli/retrospective/policy.py::resolve_policy` is what the post-merge
//! capture actually consults. Its precedence is not a single map: charter.yaml
//! `governance.retrospective` is the authority, charter.md frontmatter is an
//! overridden secondary, `.kittify/config.yaml` fills what no charter layer
//! claimed (unless a charter layer sets `precedence: config`), and the built-in
//! defaults are the ONLY source of `<default>`.
//!
//! This file binds the Native resolver to the pinned Source for the cases the
//! capture actually meets: absent configuration, a partial config block, both
//! charter layers, the `precedence: config` delegation, granular permissions and
//! `generator`, a disabled policy, unknown keys with `strict_keys` on and off,
//! malformed charter.yaml that DOES and does NOT mention a retrospective block
//! (#3163), and the deprecated environment variables being OBSERVED but never
//! authoritative.
//!
//! For each case the WHOLE policy object (every field, permissions included) and
//! the WHOLE source map are compared, or — for malformed input — the
//! `PolicyResolutionError`'s `source` and `reason`. The `detail` prose is a
//! parser's message (ruamel/pydantic vs `serde_yaml_ng`), so it is asserted to be
//! present, never compared byte for byte.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sk_workspace::retrospective_policy;

const ORACLE: &str = r#"
import json, os, sys
from pathlib import Path
from specify_cli.retrospective.policy import PolicyResolutionError, resolve_policy

root = Path(sys.argv[1])
env = json.loads(sys.argv[2])
try:
    policy, source_map = resolve_policy(root, env=env)
except PolicyResolutionError as exc:
    print(json.dumps({"error": {"source": exc.source, "reason": exc.reason, "detail": str(exc.detail)}}))
    raise SystemExit(0)
print(json.dumps({
    "policy": {
        "enabled": policy.enabled,
        "timing": policy.timing,
        "failure_policy": policy.failure_policy,
        "write_record": policy.write_record,
        "generate_proposals": policy.generate_proposals,
        "apply_proposals": policy.apply_proposals,
        "permissions": {
            "write_record": policy.permissions.write_record,
            "inspect_mission_artifacts": policy.permissions.inspect_mission_artifacts,
            "propose_glossary_changes": policy.permissions.propose_glossary_changes,
            "propose_drg_changes": policy.permissions.propose_drg_changes,
            "propose_doctrine_changes": policy.permissions.propose_doctrine_changes,
            "apply_low_risk_changes": policy.permissions.apply_low_risk_changes,
            "apply_structural_changes": policy.permissions.apply_structural_changes,
        },
        "precedence": policy.precedence,
        "generator": policy.generator,
    },
    "source_map": source_map,
}))
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Case {
    name: &'static str,
    files: &'static [(&'static str, &'static str)],
    env: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    Case {
        name: "absent",
        files: &[],
        env: &[],
    },
    Case {
        name: "config-partial",
        files: &[(
            ".kittify/config.yaml",
            "retrospective:\n  enabled: false\n  failure_policy: block\n",
        )],
        env: &[],
    },
    Case {
        name: "charter-md-secondary",
        files: &[(
            ".kittify/charter/charter.md",
            "---\nretrospective:\n  timing: before_completion\n---\n\n# Charter\n",
        )],
        env: &[],
    },
    Case {
        name: "charter-yaml-authority-wins",
        files: &[
            (
                ".kittify/charter/charter.md",
                "---\nretrospective:\n  timing: before_completion\n  failure_policy: block\n---\n",
            ),
            (
                ".kittify/charter/charter.yaml",
                "governance:\n  retrospective:\n    timing: post_completion\n    precedence: charter\n",
            ),
        ],
        env: &[],
    },
    Case {
        name: "precedence-config-delegation",
        files: &[
            (
                ".kittify/charter/charter.yaml",
                "governance:\n  retrospective:\n    precedence: config\n    permissions:\n      apply_low_risk_changes: true\n",
            ),
            (
                ".kittify/config.yaml",
                "retrospective:\n  precedence: config\n  apply_proposals: low_risk_auto\n  permissions:\n    propose_glossary_changes: false\n",
            ),
        ],
        env: &[],
    },
    Case {
        name: "charter-yaml-permissions-and-generator",
        files: &[(
            ".kittify/charter/charter.yaml",
            "governance:\n  retrospective:\n    generator: python\n    generate_proposals: false\n    permissions:\n      apply_structural_changes: true\n      write_record: false\n    strict_keys: false\n",
        )],
        env: &[],
    },
    Case {
        name: "disabled-by-config",
        files: &[(".kittify/config.yaml", "retrospective:\n  enabled: false\n")],
        env: &[],
    },
    Case {
        name: "unknown-key-lenient",
        files: &[(
            ".kittify/config.yaml",
            "retrospective:\n  bogus_key: 1\n  timing: before_completion\n",
        )],
        env: &[],
    },
    Case {
        name: "unknown-key-strict",
        files: &[(
            ".kittify/config.yaml",
            "retrospective:\n  strict_keys: true\n  bogus_key: 1\n",
        )],
        env: &[],
    },
    Case {
        name: "corrupt-charter-yaml-mentions-retrospective",
        files: &[(
            ".kittify/charter/charter.yaml",
            "governance:\n  retrospective:\n    timing: [unclosed\n",
        )],
        env: &[],
    },
    Case {
        name: "corrupt-charter-yaml-unrelated-falls-through",
        files: &[
            (
                ".kittify/charter/charter.yaml",
                "catalog:\n  broken: [unclosed\n",
            ),
            (
                ".kittify/charter/charter.md",
                "---\nretrospective:\n  timing: before_completion\n---\n",
            ),
        ],
        env: &[],
    },
    Case {
        name: "charter-md-not-a-mapping",
        files: &[(
            ".kittify/charter/charter.md",
            "---\nretrospective: [1, 2]\n---\n",
        )],
        env: &[],
    },
    Case {
        name: "invalid-enum-in-config",
        files: &[(
            ".kittify/config.yaml",
            "retrospective:\n  timing: sometimes\n",
        )],
        env: &[],
    },
    Case {
        name: "env-observed-only",
        files: &[(
            ".kittify/config.yaml",
            "retrospective:\n  failure_policy: block\n",
        )],
        env: &[
            ("SPEC_KITTY_RETROSPECTIVE", "1"),
            ("SPEC_KITTY_MODE", "strict"),
        ],
    },
];

fn write_case(root: &Path, case: &Case) -> BTreeMap<String, String> {
    for (relative, contents) in case.files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("case dir");
        std::fs::write(&path, contents).expect("case file");
    }
    case.env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn source_output(root: &Path, env: &BTreeMap<String, String>) -> serde_json::Value {
    let script = root.join("policy-oracle.py");
    std::fs::write(&script, ORACLE).expect("write oracle");
    let output: Output = Command::new(oracle::python())
        .arg(&script)
        .arg(root)
        .arg(serde_json::to_string(env).expect("env json"))
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_RUNTIME_DIR", root.join("runtime"))
        .env("TMPDIR", root.join("tmp"))
        .env("TMP", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .env(
            "PYTHONPATH",
            format!(
                "{}:{}",
                repo_root().display(),
                repo_root().join("src").display()
            ),
        )
        .env(
            "SPEC_KITTY_PACKS_ROOT",
            repo_root().join("packs").display().to_string(),
        )
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .expect("run the pinned Source resolver");
    assert!(
        output.status.success(),
        "the pinned Source policy resolver failed (a harness/premise failure, not a business red):\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("oracle json");
    std::fs::remove_file(&script).expect("remove oracle");
    value
}

fn native_policy_json(policy: &retrospective_policy::RetrospectivePolicy) -> serde_json::Value {
    serde_json::json!({
        "enabled": policy.enabled,
        "timing": policy.timing,
        "failure_policy": policy.failure_policy,
        "write_record": policy.write_record,
        "generate_proposals": policy.generate_proposals,
        "apply_proposals": policy.apply_proposals,
        "permissions": {
            "write_record": policy.permissions.write_record,
            "inspect_mission_artifacts": policy.permissions.inspect_mission_artifacts,
            "propose_glossary_changes": policy.permissions.propose_glossary_changes,
            "propose_drg_changes": policy.permissions.propose_drg_changes,
            "propose_doctrine_changes": policy.permissions.propose_doctrine_changes,
            "apply_low_risk_changes": policy.permissions.apply_low_risk_changes,
            "apply_structural_changes": policy.permissions.apply_structural_changes,
        },
        "precedence": policy.precedence,
        "generator": policy.generator,
    })
}

#[test]
fn configured_retrospective_policy_matches_the_pinned_source() {
    let temp = tempfile::tempdir().expect("sandbox");
    for case in CASES {
        let root = temp.path().join(case.name);
        std::fs::create_dir_all(&root).expect("case root");
        let env = write_case(&root, case);

        // ---------------- Source FIRST ----------------
        let source = source_output(&root, &env);

        // ---------------- Native SECOND ----------------
        let native = retrospective_policy::resolve_with_env(&root, &env);
        match source.get("error") {
            Some(error) => {
                let failure = match native {
                    Err(failure) => failure,
                    Ok(_) => panic!(
                        "{}: Source refused the input but the Native resolver accepted it",
                        case.name
                    ),
                };
                assert_eq!(
                    failure.source,
                    error["source"].as_str().expect("source"),
                    "{}: the malformed input is attributed to the same file",
                    case.name
                );
                assert_eq!(
                    failure.reason,
                    error["reason"].as_str().expect("reason"),
                    "{}: the Source reason code matches",
                    case.name
                );
                assert!(
                    !failure.detail.is_empty(),
                    "{}: the failure carries a human detail",
                    case.name
                );
            }
            None => {
                let resolved = native.unwrap_or_else(|error| {
                    panic!(
                        "{}: the Native resolver refused a Source-valid input: {error}",
                        case.name
                    )
                });
                assert_eq!(
                    native_policy_json(&resolved.policy),
                    source["policy"],
                    "{}: the WHOLE resolved policy matches Source",
                    case.name
                );
                assert_eq!(
                    serde_json::json!(resolved.source_map),
                    source["source_map"],
                    "{}: the WHOLE per-field source map matches Source",
                    case.name
                );
            }
        }
    }
}

#[test]
fn the_default_source_map_is_all_default_and_the_keys_are_source_ordered() {
    let map = retrospective_policy::default_source_map();
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    let mut expected: Vec<String> = retrospective_policy::POLICY_KEYS
        .iter()
        .map(|key| (*key).to_owned())
        .collect();
    expected.sort();
    assert_eq!(
        keys, expected,
        "the map carries exactly Source's fifteen leaf keys (the WRITER emits them in \
         `policy.py` declaration order; a sorted map stores them sorted)"
    );
    assert!(
        map.values().all(|value| value == "<default>"),
        "every leaf is `<default>` until a source claims it"
    );
    let error_map = retrospective_policy::resolution_error_source_map();
    let mut error_keys: Vec<String> = error_map.keys().cloned().collect();
    error_keys.sort();
    assert_eq!(
        error_keys,
        vec!["enabled", "failure_policy", "timing"],
        "the resolution-error map is Source's minimal three-key map"
    );
}
