//! Source-first #4972 alignment and NEW-ledger-record dirty semantics.
//! This tests the bookkeeping seam, not complete fan-out/failure/schema parity.
#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

const MAIN_STAMP: &str = "2026-01-02T03:04:05+00:00";
const SKIPPED_ID: &str = "0.9.0_frontmatter_only_lanes";

fn fixture(root: &Path) {
    std::fs::create_dir_all(root.join(".kittify")).unwrap();
    std::fs::write(
        root.join(".kittify/config.yaml"),
        "agents:\n  available: []\n",
    )
    .unwrap();
    std::fs::write(
        root.join(".kittify/metadata.yaml"),
        "spec_kitty:\n  version: 3.2.0\n  schema_version: 3\n  initialized_at: '2025-01-01T00:00:00+00:00'\n  last_upgraded_at: '2025-01-01T00:00:00+00:00'\nenvironment:\n  python_version: '3.11.15'\n  platform: Linux\n  platform_version: fixture\n",
    ).unwrap();
}

#[test]
fn version_only_alignment_is_not_a_new_skipped_record() {
    if let Some(root) = std::env::var_os("SK_BOOKKEEPING_NATIVE_CHILD") {
        let root = std::path::PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in environment_paths() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        let facts: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("source.stdout")).unwrap()).unwrap();
        let aligned_root = root.join("native-aligned");
        fixture(&aligned_root);
        let request = sk_install::UpgradeRequest::new(&aligned_root, "4.0.0rc8")
            .with_scope(BTreeSet::new())
            .with_bookkeeping_stamp(MAIN_STAMP);
        let report = sk_install::run_upgrade(&request).unwrap();
        assert!(report.success && report.metadata_written && report.rows.is_empty());
        let aligned = sk_install::ProjectMetadata::load(&aligned_root.join(".kittify")).unwrap();
        assert_eq!(aligned.version, "4.0.0rc8");
        assert_eq!(
            aligned.last_upgraded_at.as_deref(),
            facts["aligned"].as_str()
        );
        let after = std::fs::read(aligned_root.join(".kittify/metadata.yaml")).unwrap();
        let rerun = sk_install::run_upgrade(&request).unwrap();
        assert!(!rerun.metadata_written);
        assert_eq!(
            after,
            std::fs::read(aligned_root.join(".kittify/metadata.yaml")).unwrap()
        );

        let recorded_root = root.join("native-new-skip");
        fixture(&recorded_root);
        // Authored retry history keeps this superseded id pending across
        // adoption. `seed_for_init` is NOT an id filter: it unconditionally
        // settles tracked ids, so it is intentionally not used here.
        let skipped_spec = sk_install::MigrationCatalog::embedded()
            .unwrap()
            .get(SKIPPED_ID)
            .unwrap();
        sk_install::MigrationLedger::new(&recorded_root)
            .append(&sk_install::LedgerRecord::for_spec(
                skipped_spec,
                "failed",
                "3.2.0",
            ))
            .unwrap();
        // A real Git anchor, never a fake .git marker, even if this no-op does
        // not require Git on the pinned catalogue.
        assert!(
            Command::new("git")
                .args(["init", "-q", "-b", "main"])
                .current_dir(&recorded_root)
                .status()
                .unwrap()
                .success()
        );
        let request = sk_install::UpgradeRequest::new(&recorded_root, "4.0.0rc8")
            .with_scope(BTreeSet::from([SKIPPED_ID.to_owned()]))
            .with_bookkeeping_stamp(MAIN_STAMP);
        let report = sk_install::run_upgrade(&request).unwrap();
        assert!(report.success);
        assert_eq!(report.rows.len(), 1);
        assert_eq!(report.rows[0].result, "skipped");
        let recorded = sk_install::ProjectMetadata::load(&recorded_root.join(".kittify")).unwrap();
        // Source's dirty signal is a NEW record, not applied-content count.
        assert_ne!(recorded.last_upgraded_at.as_deref(), Some(MAIN_STAMP));
        let spec = sk_install::MigrationCatalog::embedded()
            .unwrap()
            .get(SKIPPED_ID)
            .unwrap();
        let ledger = sk_install::MigrationLedger::new(&recorded_root);
        let _held = sk_install::ProjectLock::for_project(&recorded_root, std::time::Duration::ZERO)
            .unwrap();
        assert!(
            !sk_install::upgrade::record_scoped_result(&ledger, spec, "skipped", "4.0.0rc8")
                .unwrap()
        );
        return;
    }

    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in environment_paths() {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut source = Command::new(oracle::python());
    isolated(&mut source, &root, &repo);
    let output = source.args(["-c", r#"
import json, os, sys
from pathlib import Path
from datetime import datetime, timezone
root = Path(os.environ['SK_BOOKKEEPING_ROOT'])
assert Path.cwd() == root and sys.version_info[:3] == (3, 11, 15)
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),
                  ('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),
                  ('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root / name
from specify_cli.upgrade.runner import MigrationRunner, ProjectMetadata
old = datetime(2025, 1, 1, tzinfo=timezone.utc)
stamp = datetime(2026, 1, 2, 3, 4, 5, tzinfo=timezone.utc)
fallback = datetime(2026, 2, 3, tzinfo=timezone.utc)
def meta(version, last=old):
    return ProjectMetadata(version=version, initialized_at=old, last_upgraded_at=last)
main = meta('4.0.0rc8', stamp)
aligned = meta('3.2.0')
assert MigrationRunner._reconcile_worktree_bookkeeping(aligned, '4.0.0rc8', False, main, fallback)
assert aligned.last_upgraded_at == stamp
same = meta('4.0.0rc8')
assert not MigrationRunner._reconcile_worktree_bookkeeping(same, '4.0.0rc8', False, main, fallback)
assert same.last_upgraded_at == old
fresh = meta('3.2.0')
before = datetime.now(timezone.utc)
assert MigrationRunner._reconcile_worktree_bookkeeping(fresh, '4.0.0rc8', True, main, fallback)
after = datetime.now(timezone.utc)
assert before <= fresh.last_upgraded_at <= after
assert fresh.last_upgraded_at != stamp
for main_or_none in [None, meta('4.0.0rc8', None)]:
    left, right = meta('3.2.0'), meta('3.2.0')
    for sibling in [left, right]:
        assert MigrationRunner._reconcile_worktree_bookkeeping(sibling, '4.0.0rc8', False, main_or_none, fallback)
        assert sibling.last_upgraded_at == fallback
    assert left.last_upgraded_at == right.last_upgraded_at
print(json.dumps({'aligned': aligned.last_upgraded_at.isoformat(),
                  'fresh': fresh.last_upgraded_at.isoformat(), 'fallback': fallback.isoformat()}))
"#]).output().unwrap();
    std::fs::write(root.join("source.stdout"), &output.stdout).unwrap();
    std::fs::write(root.join("source.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "Source must assert first: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut native = Command::new(std::env::current_exe().unwrap());
    isolated(&mut native, &root, &repo);
    let output = native
        .args([
            "--exact",
            "version_only_alignment_is_not_a_new_skipped_record",
            "--nocapture",
        ])
        .env("SK_BOOKKEEPING_NATIVE_CHILD", &root)
        .output()
        .unwrap();
    std::fs::write(root.join("native.stdout"), &output.stdout).unwrap();
    std::fs::write(root.join("native.stderr"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "Native bookkeeping: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn environment_paths() -> [(&'static str, &'static str); 9] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
    ]
}

fn isolated(command: &mut Command, root: &Path, repo: &Path) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("SK_BOOKKEEPING_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .current_dir(root);
    for (key, name) in environment_paths() {
        command.env(key, root.join(name));
    }
}
