//! Source-first execution metadata; catalogue order/ledger selection are unchanged.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::path::Path;
use std::process::Command;

#[test]
fn all_declared_worktree_flags_match_actual_source_registry() {
    let root = tempfile::tempdir().expect("Source sandbox").keep();
    for name in ["home", "tmp", "config", "cache", "data", "state"] {
        std::fs::create_dir(root.join(name)).unwrap();
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new(oracle::python())
        .args(["-c", r#"
import json,os,sys
from pathlib import Path
root=Path(os.environ['SK_SCOPE_ROOT'])
assert Path.cwd()==root and sys.version_info[:3]==(3,11,15)
for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):assert Path(os.environ[key]).parent==root
from specify_cli.upgrade.migrations import auto_discover_migrations
from specify_cli.upgrade.registry import MigrationRegistry
from specify_cli.upgrade.migrations.base import BaseMigration
auto_discover_migrations()
rows=[[m.migration_id,m.runs_on_worktrees] for m in MigrationRegistry.get_all()]
assert BaseMigration.runs_on_worktrees is True
assert len(rows)==136 and all(type(value) is bool for _,value in rows)
assert sum(not value for _,value in rows)==22
assert dict(rows)['runtime_state_backfill'] is False
assert dict(rows)['4.0.0rc8_command_skill_hyphen_names'] is True
print(json.dumps(rows))
"#])
        .env_clear().env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home")).env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("config")).env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data")).env("XDG_STATE_HOME", root.join("state"))
        .env("SK_SCOPE_ROOT", &root).env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repo.join("src")).env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .current_dir(&root).output().expect("actual Source registry");
    std::fs::write(root.join("source.stdout"), &output.stdout).unwrap();
    std::fs::write(root.join("source.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "Source must assert before Native: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source: Vec<(String, bool)> =
        serde_json::from_slice(&output.stdout).expect("Source metadata");
    assert_eq!(source.len(), 136);
    // Pure Native metadata consumption only after all Source assertions.
    let catalog = sk_install::MigrationCatalog::embedded().expect("Native catalogue");
    let native: Vec<_> = catalog
        .specs()
        .iter()
        .map(|spec| (spec.id.clone(), spec.runs_on_worktrees))
        .collect();
    assert_eq!(native, source);
    for migration in sk_install::native_migrations() {
        assert_eq!(
            migration.runs_on_worktrees().expect("declared callback"),
            catalog.get(migration.id).unwrap().runs_on_worktrees
        );
    }
    let runtime = catalog.get("runtime_state_backfill").unwrap();
    assert!(!runtime.runs_on_worktrees);
    // Preserve the existing serialized inspection/CLI schema: this is internal
    // catalogue execution metadata, not an unsolicited new wire key.
    assert!(
        serde_json::to_value(runtime)
            .unwrap()
            .get("runs_on_worktrees")
            .is_none()
    );
}
