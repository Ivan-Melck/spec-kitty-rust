//! Differential home, user-dir and pack-path parity with the pinned Python.
//!
//! Environment branches run in child processes because edition 2024 makes
//! `std::env::set_var` unsafe and this workspace forbids `unsafe`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[path = "support/oracle.rs"]
mod oracle_support;
const CASE: &str = "SK_HP_CASE";

fn python() -> Option<PathBuf> {
    // Missing or drifted oracles panic; None must never certify zero comparisons.
    Some(oracle_support::python())
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root")
}

fn rust_child(case: &str, env: &[(&str, &Path)]) -> Value {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "home_paths_child", "--nocapture"])
        .env(CASE, case)
        .env_remove("XDG_CACHE_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("SPEC_KITTY_HOME")
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env_remove("SPEC_KITTY_TEMPLATE_ROOT")
        .env_remove("LOCALAPPDATA");
    command.env(
        "HOME",
        env_value(env, "HOME").unwrap_or_else(|| Path::new("/nonexistent")),
    );
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run Rust home child");
    assert!(
        output.status.success(),
        "Rust child {case} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or_else(|| panic!("Rust child printed no JSON:\n{stdout}"));
    serde_json::from_str(line).expect("Rust JSON")
}

fn env_value<'a>(env: &'a [(&str, &Path)], name: &str) -> Option<&'a Path> {
    env.iter()
        .find(|(candidate, _)| *candidate == name)
        .map(|(_, value)| *value)
}

#[test]
fn home_paths_child() {
    let Some(case) = std::env::var_os(CASE) else {
        return;
    };
    let value = match case.to_string_lossy().as_ref() {
        "dirs" => serde_json::json!({
            "kittify_home": sk_kernel::get_kittify_home().unwrap().display().to_string(),
            "runtime_state_root": sk_kernel::get_runtime_state_root().unwrap().display().to_string(),
            "cache_default": sk_kernel::user_cache_dir(true).unwrap().display().to_string(),
            "cache_xdg_only": sk_kernel::user_cache_dir(false).unwrap().display().to_string(),
            "state": sk_kernel::user_state_dir().unwrap().display().to_string(),
        }),
        "packs" => {
            let built_in = sk_kernel::resolve_built_in_pack_root().unwrap();
            serde_json::json!({
                "built_in": built_in.path.display().to_string(),
                "packs_root_default": sk_kernel::get_packs_root_default().unwrap().display().to_string(),
                "assets": sk_kernel::get_package_asset_root().unwrap().display().to_string(),
                "ignored_override": built_in.ignored_override.map(|path| path.display().to_string()),
            })
        }
        other => panic!("unknown home path case {other}"),
    };
    println!("{}", serde_json::to_string(&value).unwrap());
}

fn python_observe(case: &str, env: &[(&str, &Path)]) -> Option<Value> {
    let python = python()?;
    let script = if case == "dirs" {
        r#"
import json
from kernel.paths import get_kittify_home, get_runtime_state_root
from kernel.user_dirs import user_cache_dir, user_state_dir
print(json.dumps({
    "kittify_home": str(get_kittify_home()),
    "runtime_state_root": str(get_runtime_state_root()),
    "cache_default": str(user_cache_dir()),
    "cache_xdg_only": str(user_cache_dir(windows_local_appdata=False)),
    "state": str(user_state_dir()),
}, sort_keys=True))
"#
    } else {
        r#"
import json, warnings
from kernel.paths import get_built_in_pack_root, get_package_asset_root, get_packs_root_default
with warnings.catch_warnings():
    warnings.simplefilter("ignore")
    built_in = get_built_in_pack_root()
    override = None
    import os
    value = os.environ.get("SPEC_KITTY_PACKS_ROOT")
    if value:
        candidate = __import__("pathlib").Path(value) / "built-in"
        if not candidate.is_dir():
            override = str(candidate)
    print(json.dumps({
        "built_in": str(built_in),
        "packs_root_default": str(get_packs_root_default()),
        "assets": str(get_package_asset_root()),
        "ignored_override": override,
    }, sort_keys=True))
"#
    };
    let mut command = Command::new(python);
    command
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", repo_root().join("src"))
        .current_dir(repo_root())
        .env_remove("XDG_CACHE_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("SPEC_KITTY_HOME")
        .env_remove("SPEC_KITTY_PACKS_ROOT")
        .env_remove("SPEC_KITTY_TEMPLATE_ROOT")
        .env_remove("LOCALAPPDATA");
    command.env(
        "HOME",
        env_value(env, "HOME").unwrap_or_else(|| Path::new("/nonexistent")),
    );
    for (name, value) in env {
        command.env(name, value);
    }
    let output = command.output().expect("run Python path probe");
    assert!(
        output.status.success(),
        "Python path probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(serde_json::from_slice(&output.stdout).expect("python JSON"))
}

#[test]
fn home_and_user_directories_match_the_python_baseline() {
    let Some(_) = python() else {
        eprintln!("pinned Python oracle is not installed; skipping home-path parity test");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let xdg_cache = temp.path().join("xdg-cache");
    let xdg_state = temp.path().join("xdg-state");
    let spec_home = temp.path().join("spec-home");
    let local_app_data = temp.path().join("local-app-data");

    let cases: Vec<(&str, Vec<(&str, &Path)>)> = vec![
        ("default", vec![("HOME", home.as_path())]),
        (
            "xdg",
            vec![
                ("HOME", home.as_path()),
                ("XDG_CACHE_HOME", xdg_cache.as_path()),
                ("XDG_STATE_HOME", xdg_state.as_path()),
            ],
        ),
        (
            "spec-home",
            vec![
                ("HOME", home.as_path()),
                ("SPEC_KITTY_HOME", spec_home.as_path()),
            ],
        ),
        (
            "local-app-data-ignored-on-posix",
            vec![
                ("HOME", home.as_path()),
                ("LOCALAPPDATA", local_app_data.as_path()),
            ],
        ),
    ];
    for (name, env) in cases {
        let rust = rust_child("dirs", &env);
        let python = python_observe("dirs", &env).expect("python");
        assert_eq!(rust, python, "home/user-dir mismatch for case {name}");
    }
}

#[test]
fn pack_roots_and_assets_match_the_python_baseline() {
    let Some(_) = python() else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let packs_root = temp.path().join("packs-root");
    fs::create_dir_all(packs_root.join("built-in/missions/software-dev/templates")).unwrap();
    fs::write(
        packs_root.join("built-in/missions/software-dev/templates/spec.md"),
        b"# spec",
    )
    .unwrap();
    let invalid_packs_root = temp.path().join("invalid-packs-root");
    fs::create_dir_all(&invalid_packs_root).unwrap();
    let template_root = temp.path().join("checkout");
    fs::create_dir_all(template_root.join("packs/built-in/missions/software-dev/templates"))
        .unwrap();
    fs::write(
        template_root.join("packs/built-in/missions/software-dev/templates/spec.md"),
        b"# spec",
    )
    .unwrap();

    let cases: Vec<(&str, Vec<(&str, &Path)>)> = vec![
        ("installed", vec![("HOME", home.as_path())]),
        (
            "packs-root",
            vec![
                ("HOME", home.as_path()),
                ("SPEC_KITTY_PACKS_ROOT", packs_root.as_path()),
            ],
        ),
        (
            "invalid-packs-root-falls-back",
            vec![
                ("HOME", home.as_path()),
                ("SPEC_KITTY_PACKS_ROOT", invalid_packs_root.as_path()),
            ],
        ),
        (
            "template-root",
            vec![
                ("HOME", home.as_path()),
                ("SPEC_KITTY_TEMPLATE_ROOT", template_root.as_path()),
            ],
        ),
    ];
    for (name, env) in cases {
        let rust = rust_child("packs", &env);
        let python = python_observe("packs", &env).expect("python");
        assert_eq!(rust, python, "pack-path mismatch for case {name}");
    }
}
