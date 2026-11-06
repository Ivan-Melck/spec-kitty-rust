//! Source-first differential for `2.0.1_tool_config_key_rename`.
//!
//! The pinned Source re-dumps `config.yaml` after
//! `data["tools"] = data.pop("agents")`. The case below was produced by
//! `.venv-rust-oracle` (CPython 3.11.15 / 53) and is written in the crate's own
//! canonical config layout, so the exact Source bytes are reproducible.

use sk_install::migrations::completion_legacy::surfaces::apply_tool_config_key_rename;

const INPUT: &str = "# header comment\nagents:\n- claude\n- codex\nproject: demo\n";
const SOURCE_OUTPUT: &str = "# header comment\nproject: demo\ntools:\n- claude\n- codex\n";

#[test]
fn rename_moves_the_key_to_the_end_like_ruamel_pop_append() {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let kittify = sandbox.path().join(".kittify");
    std::fs::create_dir_all(&kittify).expect("create .kittify");
    std::fs::write(kittify.join("config.yaml"), INPUT).expect("write config");
    apply_tool_config_key_rename(sandbox.path()).expect("apply");
    let actual = std::fs::read_to_string(kittify.join("config.yaml")).expect("read config");
    assert_eq!(actual, SOURCE_OUTPUT, "must match the pinned Source bytes");
}

#[test]
fn rename_is_idempotent() {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let kittify = sandbox.path().join(".kittify");
    std::fs::create_dir_all(&kittify).expect("create .kittify");
    std::fs::write(kittify.join("config.yaml"), INPUT).expect("write config");
    apply_tool_config_key_rename(sandbox.path()).expect("apply");
    let first = std::fs::read_to_string(kittify.join("config.yaml")).expect("read");
    apply_tool_config_key_rename(sandbox.path()).expect("apply again");
    let second = std::fs::read_to_string(kittify.join("config.yaml")).expect("read");
    assert_eq!(first, second);
}

#[test]
fn a_config_with_both_keys_is_left_alone() {
    let sandbox = tempfile::tempdir().expect("tempdir");
    let kittify = sandbox.path().join(".kittify");
    std::fs::create_dir_all(&kittify).expect("create .kittify");
    let both = "agents:\n- claude\ntools:\n- codex\n";
    std::fs::write(kittify.join("config.yaml"), both).expect("write config");
    apply_tool_config_key_rename(sandbox.path()).expect("apply");
    let actual = std::fs::read_to_string(kittify.join("config.yaml")).expect("read");
    assert_eq!(actual, both);
}
