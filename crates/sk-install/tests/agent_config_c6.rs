//! C6 differential: the `AgentConfig.auto_commit` matrix.
//!
//! Oracle: `tests/fixtures/agent_config_c6.json`, recorded by the pinned Python
//! CLI (`tests/fixtures/agent_config_c6_probe.py`) over the same 22 configs.
//! The projection under test is the canonical
//! `support::load_agent_config`, which is also the `auto_commit` authority the
//! upgrade fan-out reads.

use std::path::{Path, PathBuf};

use serde_json::Value;

const FIXTURE_REL: &str = "tests/fixtures/agent_config_c6.json";

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE_REL);
    let raw = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "the recorded Source AgentConfig oracle {} is unavailable ({error}); \
             a missing oracle is a failure, never a pass",
            path.display()
        )
    });
    serde_json::from_slice(&raw).expect("recorded AgentConfig oracle is JSON")
}

fn sandbox(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "sk-c6-agent-config-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("sandbox");
    path
}

/// The same 22 configs the Source probe wrote (`probe_agent_config_c6.py`).
const CASES: [(&str, Option<&str>); 22] = [
    ("no_file", None),
    (
        "agents_false",
        Some("agents:\n  available:\n    - claude\n  auto_commit: false\n"),
    ),
    (
        "agents_absent_setting_top_false",
        Some("agents:\n  available:\n    - claude\nauto_commit: false\n"),
    ),
    (
        "agents_null_setting_top_false",
        Some("agents:\n  available:\n    - claude\n  auto_commit: null\nauto_commit: false\n"),
    ),
    (
        "agents_null_setting_top_true",
        Some("agents:\n  available:\n    - claude\n  auto_commit: null\nauto_commit: true\n"),
    ),
    (
        "agents_string_false",
        Some("agents:\n  available:\n    - claude\n  auto_commit: \"false\"\n"),
    ),
    (
        "agents_int_one",
        Some("agents:\n  available:\n    - claude\n  auto_commit: 1\n"),
    ),
    (
        "agents_int_zero",
        Some("agents:\n  available:\n    - claude\n  auto_commit: 0\n"),
    ),
    (
        "agents_null_tools_false",
        Some("agents: null\ntools:\n  available:\n    - claude\n  auto_commit: false\n"),
    ),
    (
        "agents_empty_tools_false",
        Some("agents: {}\ntools:\n  available:\n    - claude\n  auto_commit: false\n"),
    ),
    (
        "agents_setting_no_available_tools",
        Some(
            "agents:\n  auto_commit: false\ntools:\n  available:\n    - claude\n  auto_commit: true\n",
        ),
    ),
    (
        "tools_scalar_available",
        Some("tools:\n  available: claude\n  auto_commit: false\n"),
    ),
    ("top_only_false", Some("auto_commit: false\n")),
    (
        "agents_available_null",
        Some("agents:\n  available: null\n"),
    ),
    ("agents_sequence", Some("agents: []\n")),
    ("top_scalar", Some("just-a-string\n")),
    ("empty_file", Some("{}\n")),
    (
        "unknown_agent",
        Some("agents:\n  available:\n    - not-an-agent\n"),
    ),
    (
        "agents_wins_over_tools",
        Some(
            "agents:\n  available:\n    - claude\n  auto_commit: false\ntools:\n  available:\n    - claude\n  auto_commit: true\n",
        ),
    ),
    ("agents_empty_list", Some("agents:\n  available: []\n")),
    ("agents_available_int", Some("agents:\n  available: 3\n")),
    (
        "legacy_tools_true",
        Some("tools:\n  available:\n    - claude\n  auto_commit: true\n"),
    ),
];

#[test]
fn the_auto_commit_matrix_matches_the_recorded_source_projection() {
    use sk_install::migrations::completion_modern::support::load_agent_config;

    let oracle = fixture();
    let recorded = oracle["cases"].as_object().expect("recorded cases");
    assert_eq!(recorded.len(), CASES.len());
    let sandbox = sandbox("matrix");
    for (name, text) in CASES {
        let project = sandbox.join(name);
        std::fs::create_dir_all(project.join(".kittify")).expect("project");
        if let Some(text) = text {
            std::fs::write(project.join(".kittify/config.yaml"), text).expect("config");
        }
        let expected = recorded
            .get(name)
            .unwrap_or_else(|| panic!("no recorded case {name}"));
        match load_agent_config(&project) {
            Ok(config) => {
                assert_eq!(
                    expected["outcome"].as_str(),
                    Some("ok"),
                    "{name}: native accepted a config Source refuses"
                );
                assert_eq!(
                    serde_json::json!(config.available),
                    expected["available"],
                    "{name}: available differs from the recorded Source list"
                );
                assert_eq!(
                    config.auto_commit,
                    expected["auto_commit"].as_bool().expect("recorded bool"),
                    "{name}: auto_commit differs from the recorded Source value"
                );
                assert_eq!(
                    config.explicit_agent_list,
                    expected["explicit_agent_list"]
                        .as_bool()
                        .expect("recorded bool"),
                    "{name}: explicit_agent_list differs"
                );
            }
            Err(error) => {
                assert_eq!(
                    expected["outcome"].as_str(),
                    Some("error"),
                    "{name}: native refused a config Source accepts ({error})"
                );
            }
        }
    }
}

#[test]
fn the_upgrade_consumer_reads_the_canonical_field_not_a_second_parser() {
    use sk_install::migrations::completion_modern::support::load_agent_config;

    let sandbox = sandbox("consumer");
    let project = sandbox.join("project");
    std::fs::create_dir_all(project.join(".kittify")).expect("project");
    // The legacy section wins only while `agents` carries no content, and the
    // top-level value is the fallback — the two cases the old scalar reader got
    // backwards (it read the top level first and never consulted `tools`).
    std::fs::write(
        project.join(".kittify/config.yaml"),
        "agents: {}\ntools:\n  available:\n    - claude\n  auto_commit: false\n",
    )
    .expect("config");
    assert!(!load_agent_config(&project).expect("loads").auto_commit);
}
