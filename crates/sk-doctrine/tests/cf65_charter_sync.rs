//! CF65 — the Source charter-bundle freshness gate and the retired sync, proven
//! against the actual reference and through actual processes.
//!
//! The reference keeps `charter.activation.sync.ensure_charter_bundle_fresh` /
//! `sync` alive after the IC-04 retirement: the gate resolves the canonical
//! (main-checkout) charter read root, treats an absent `charter.md` as healthy
//! absence, forces the sync when the manifest's required derived files are missing
//! or `charter.hasher.is_stale` says the bundle is stale, reads through the ONE
//! encoding-provenance chokepoint, and writes **no** derived artifact any more.
//!
//! These controls are deliberately small and split in two:
//!
//! * in-process Native checks for each phase and for the retired sync's
//!   zero-write contract (including the two warning bodies the reference logs);
//! * one **paired process control**: the Source child is the pinned CPython
//!   3.11.15 oracle calling the actual `ensure_charter_bundle_fresh`, the Native
//!   child is this test binary re-executed calling
//!   [`sk_doctrine::ensure_charter_bundle_fresh`]. Each child first asserts its own
//!   CWD, HOME, all seven XDG roots, TMP/TMPDIR/TEMP, absolute `PYTHONPATH`, the
//!   pinned packs root and `PYTHONHASHSEED`, in its own independent fixture tree,
//!   and reports only business facts: result fields, provenance-record placement
//!   and count, and the unchanged charter-bundle footprint.
//!
//! The two trees are independent fresh fixtures — never one tree the Source child
//! populated for the Native child to "leave alone".
//!
//! Not CLI acceptance, not whole-command parity, not approval: the live
//! action-context consumer is proven in `sk-cli/tests/cf65_context_sync_consumer.rs`.

mod support;

#[path = "support/isolated.rs"]
mod isolated;

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value as JsonValue, json};
use sk_doctrine::{
    CharterStaleness, CharterSyncError, CharterSyncResult, charter_bundle_staleness,
    ensure_charter_bundle_fresh, source_cwd_journal, sync_charter_bundle,
};
use sk_kernel::encoding_provenance::EncodingProvenanceJournal;

const CHARTER_BODY: &str =
    "# Probe Charter\n\n## Policy Summary\n\n- Keep one canonical authority.\n";

/// The non-`ROOTS` XDG variables the standing orders also require.
const EXTRA_XDG: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

// ---------------------------------------------------------------------------
// Fixture construction
// ---------------------------------------------------------------------------

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("fixture parent");
    }
    std::fs::write(path, contents).expect("fixture write");
}

fn git_init(root: &Path) {
    let status = Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("-b")
        .arg("main")
        .arg(root)
        .status()
        .expect("git is available for a real repository fixture");
    assert!(status.success(), "git init {root:?} failed");
}

/// `metadata.yaml` whose recorded digest matches `body` exactly.
fn fresh_metadata(body: &str) -> String {
    format!(
        "charter_hash: {}\ntimestamp_utc: '2026-01-01T00:00:00+00:00'\n",
        sk_charter::hash_content(body)
    )
}

/// One case: a real Git project root with a charter bundle in the given shape.
fn case(root: &Path, shape: &str) {
    std::fs::create_dir_all(root).expect("case dir");
    git_init(root);
    let charter_dir = root.join(".kittify/charter");
    match shape {
        "absent" => {
            std::fs::create_dir_all(&charter_dir).expect("charter dir");
        }
        "fresh" => {
            write(&charter_dir.join("charter.md"), CHARTER_BODY);
            write(
                &charter_dir.join("metadata.yaml"),
                &fresh_metadata(CHARTER_BODY),
            );
        }
        "stale" => {
            write(&charter_dir.join("charter.md"), CHARTER_BODY);
        }
        "bom-stale" => {
            write(
                &charter_dir.join("charter.md"),
                &format!("\u{feff}{CHARTER_BODY}"),
            );
        }
        "prose-is-a-directory" => {
            std::fs::create_dir_all(charter_dir.join("charter.md")).expect("dir prose");
        }
        "metadata-is-a-directory" => {
            write(&charter_dir.join("charter.md"), CHARTER_BODY);
            std::fs::create_dir_all(charter_dir.join("metadata.yaml")).expect("dir metadata");
        }
        other => panic!("unknown case shape {other}"),
    }
}

/// The five bundle shapes the paired control compares.
const PAIRED_CASES: [(&str, &str); 6] = [
    ("absent", "absent"),
    ("fresh", "fresh"),
    ("stale", "stale"),
    ("bom-stale", "bom-stale"),
    ("prose-is-a-directory", "prose-is-a-directory"),
    ("metadata-is-a-directory", "metadata-is-a-directory"),
];

fn build_paired_cases(root: &Path) {
    std::fs::create_dir_all(root.join("cases")).expect("cases dir");
    for (name, shape) in PAIRED_CASES {
        case(&root.join("cases").join(name), shape);
    }
}

/// Every path under `root`, relative and sorted (the footprint witness).
fn footprint(root: &Path) -> Vec<String> {
    fn walk(base: &Path, current: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(current) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in paths {
            let relative = path
                .strip_prefix(base)
                .expect("path under base")
                .display()
                .to_string();
            out.push(relative);
            if path.is_dir() {
                walk(base, &path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out
}

/// The lines of a project-level provenance journal under `root`.
fn global_log_lines(root: &Path) -> Vec<String> {
    let path = root.join(".kittify/encoding-provenance/global.jsonl");
    match std::fs::read_to_string(&path) {
        Ok(text) => text
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => panic!("unreadable provenance journal {}: {error}", path.display()),
    }
}

// ---------------------------------------------------------------------------
// In-process Native controls
// ---------------------------------------------------------------------------

#[test]
fn an_absent_charter_is_healthy_absence_and_records_nothing() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    case(root, "absent");
    let before = footprint(root);

    let journal = EncodingProvenanceJournal::new(root.join("journal-base"));
    let result = ensure_charter_bundle_fresh(root, None, &journal).expect("resolution");

    assert!(
        result.is_none(),
        "no charter.md means there is nothing to refresh"
    );
    assert!(global_log_lines(root).is_empty());
    assert_eq!(footprint(root), before, "healthy absence writes nothing");
}

#[test]
fn a_complete_fresh_bundle_reports_without_ingesting() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    case(root, "fresh");
    let before = footprint(root);

    let journal = EncodingProvenanceJournal::new(root.join("journal-base"));
    let result = ensure_charter_bundle_fresh(root, None, &journal)
        .expect("resolution")
        .expect("a charter.md is present");

    assert_eq!(
        result,
        CharterSyncResult {
            synced: false,
            stale_before: false,
            files_written: Vec::new(),
            extraction_mode: String::new(),
            error: None,
            canonical_root: Some(root.to_path_buf()),
            warnings: Vec::new(),
        }
    );
    assert!(
        global_log_lines(root).is_empty(),
        "a fresh bundle must not read (or record) provenance at all"
    );
    assert_eq!(footprint(root), before);
}

#[test]
fn an_incomplete_bundle_syncs_once_and_writes_no_derived_artifact() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    // The journal base is a DIFFERENT directory: the reference's project-level
    // provenance path is CWD-relative, never read-root-relative.
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(root, "stale");
    let before = footprint(root);

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let result = ensure_charter_bundle_fresh(root, None, &journal)
        .expect("resolution")
        .expect("a charter.md is present");

    assert!(!result.synced, "the extraction is retired");
    assert!(
        result.stale_before,
        "a bundle with no metadata.yaml is stale"
    );
    assert!(
        result.files_written.is_empty(),
        "no derived file is written"
    );
    assert!(result.extraction_mode.is_empty());
    assert_eq!(result.error, None);
    assert_eq!(result.canonical_root.as_deref(), Some(root));
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);

    for derived in ["governance.yaml", "directives.yaml"] {
        assert!(
            !root.join(".kittify/charter").join(derived).exists(),
            "{derived} must never be written by the retired sync"
        );
    }
    assert_eq!(footprint(root), before, "the bundle footprint is unchanged");

    let lines = global_log_lines(journal_base.path());
    assert_eq!(
        lines.len(),
        1,
        "exactly one provenance record per sync: {lines:?}"
    );
    let record: JsonValue = serde_json::from_str(&lines[0]).expect("record JSON");
    assert_eq!(
        record["file_path"],
        json!(
            root.join(".kittify/charter/charter.md")
                .display()
                .to_string()
        )
    );
    assert_eq!(record["source_encoding"], json!("utf-8"));
    assert_eq!(record["actor"], json!("spec-kitty charter"));
    assert_eq!(record["normalization_applied"], json!(false));
    assert_eq!(record["bypass_used"], json!(false));
    assert_eq!(record["mission_id"], JsonValue::Null);
    assert_eq!(
        record.as_object().expect("object").len(),
        9,
        "the Kernel record shape is the reference's nine keys"
    );
    assert!(
        global_log_lines(root).is_empty(),
        "the read root must not receive the record"
    );
}

#[test]
fn a_repeat_call_appends_a_second_record_without_deduplication() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(root, "stale");

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let first = ensure_charter_bundle_fresh(root, None, &journal)
        .expect("resolution")
        .expect("charter");
    let second = ensure_charter_bundle_fresh(root, None, &journal)
        .expect("resolution")
        .expect("charter");
    assert_eq!(first.stale_before, second.stale_before);

    let lines = global_log_lines(journal_base.path());
    assert_eq!(lines.len(), 2, "two reads append two records: {lines:?}");
    let ids: Vec<JsonValue> = lines
        .iter()
        .map(|line| serde_json::from_str::<JsonValue>(line).expect("record")["event_id"].clone())
        .collect();
    assert_ne!(ids[0], ids[1], "each record mints its own identity");
}

#[test]
fn a_broken_metadata_forces_the_sync_and_keeps_both_warnings() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(root, "metadata-is-a-directory");

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let result = ensure_charter_bundle_fresh(root, None, &journal)
        .expect("resolution")
        .expect("charter");

    assert!(!result.synced);
    assert!(
        !result.stale_before,
        "the caught failure keeps the reference's false"
    );
    assert!(result.files_written.is_empty());
    let error = result.error.expect("the sync reports its caught failure");
    assert!(!error.is_empty());
    assert_eq!(result.warnings.len(), 2, "{:#?}", result.warnings);
    assert!(
        result.warnings[0].starts_with("Failed to evaluate charter bundle freshness: "),
        "{:#?}",
        result.warnings
    );
    assert!(
        result.warnings[1]
            .starts_with("Charter auto-sync failed while refreshing extracted artifacts: "),
        "{:#?}",
        result.warnings
    );
    // The read itself succeeded (the prose is a file), so the provenance chokepoint
    // recorded it; only the staleness re-check with that content failed.
    assert_eq!(
        global_log_lines(journal_base.path()).len(),
        1,
        "the successful read is recorded before the staleness re-check fails"
    );
}

#[test]
fn a_directory_at_the_prose_path_is_not_healthy_absence() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(root, "prose-is-a-directory");

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let result = ensure_charter_bundle_fresh(root, None, &journal)
        .expect("resolution")
        .expect("exists() is true for a directory, exactly like the reference");

    assert!(!result.synced);
    assert!(!result.stale_before);
    assert!(result.files_written.is_empty());
    assert_eq!(result.canonical_root.as_deref(), Some(root));
    assert!(
        result.error.is_some(),
        "the read refuses, it is not absence"
    );
    // Two warning phases: the staleness probe cannot read a directory either, so
    // the bundle is forced AND the forced sync then fails.
    assert_eq!(result.warnings.len(), 2, "{:#?}", result.warnings);
    assert!(
        result.warnings[0].starts_with("Failed to evaluate charter bundle freshness: "),
        "{:#?}",
        result.warnings
    );
    assert!(
        result.warnings[1]
            .starts_with("Charter auto-sync failed while refreshing extracted artifacts: ")
    );
    assert!(global_log_lines(journal_base.path()).is_empty());
}

#[test]
fn an_explicit_read_scope_overrides_the_canonical_root() {
    let temp = tempfile::tempdir().expect("temp");
    let repository = temp.path().join("repository");
    let monorepo_package = temp.path().join("packages/app");
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(&repository, "fresh");
    // The scope root is deliberately NOT a Git repository: an explicit scope is
    // honoured verbatim and no resolver fallback runs (the reference's C-001).
    write(
        &monorepo_package.join(".kittify/charter/charter.md"),
        CHARTER_BODY,
    );

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let result = ensure_charter_bundle_fresh(&repository, Some(&monorepo_package), &journal)
        .expect("an explicit scope needs no resolution")
        .expect("the scoped charter exists");

    assert_eq!(
        result.canonical_root.as_deref(),
        Some(monorepo_package.as_path())
    );
    assert!(
        result.stale_before,
        "the scoped bundle has no metadata.yaml"
    );
    let lines = global_log_lines(journal_base.path());
    assert_eq!(lines.len(), 1);
    let record: JsonValue = serde_json::from_str(&lines[0]).expect("record");
    assert_eq!(
        record["file_path"],
        json!(
            monorepo_package
                .join(".kittify/charter/charter.md")
                .display()
                .to_string()
        )
    );
}

#[test]
fn a_linked_worktree_reads_the_main_checkout_charter() {
    let temp = tempfile::tempdir().expect("temp");
    let main = temp.path().join("main");
    let linked = temp.path().join("linked");
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(&main, "fresh");
    // A real commit is required before Git can create a linked worktree, and the
    // tracked bundle is what the worktree checks out.
    let status = Command::new("git")
        .current_dir(&main)
        .args(["add", "-A"])
        .status()
        .expect("git add");
    assert!(status.success(), "fixture add failed");
    let status = Command::new("git")
        .current_dir(&main)
        .args([
            "-c",
            "user.name=CF65",
            "-c",
            "user.email=cf65@example.invalid",
        ])
        .args(["commit", "-q", "-m", "cf65 fixture"])
        .status()
        .expect("git commit");
    assert!(status.success(), "fixture commit failed");
    let status = Command::new("git")
        .current_dir(&main)
        .args(["worktree", "add", "-q"])
        .arg(&linked)
        .status()
        .expect("git worktree add");
    assert!(status.success(), "linked worktree creation failed");
    assert!(linked.join(".kittify/charter/charter.md").exists());

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let result = ensure_charter_bundle_fresh(&linked, None, &journal)
        .expect("resolution")
        .expect("charter");

    assert_eq!(
        result.canonical_root.as_deref(),
        Some(main.as_path()),
        "the canonical read root is the main checkout that owns the common dir"
    );
    assert!(!result.stale_before, "the main checkout bundle is fresh");
    assert!(global_log_lines(journal_base.path()).is_empty());
}

#[test]
fn the_retired_sync_reports_staleness_and_never_writes_derived_files() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    let journal_base = tempfile::tempdir().expect("journal temp");
    case(root, "stale");
    let charter_path = root.join(".kittify/charter/charter.md");
    let before = footprint(root);

    let journal = EncodingProvenanceJournal::new(journal_base.path());
    let forced = sync_charter_bundle(
        &charter_path,
        Some(&root.join(".kittify/charter")),
        &journal,
        true,
    );
    assert!(!forced.synced, "force no longer changes anything");
    assert!(forced.stale_before);
    assert!(forced.files_written.is_empty());
    assert_eq!(forced.error, None);
    assert_eq!(footprint(root), before);
    assert_eq!(global_log_lines(journal_base.path()).len(), 1);

    // The same call with a matching metadata.yaml is fresh, and still writes
    // nothing.
    write(
        &root.join(".kittify/charter/metadata.yaml"),
        &fresh_metadata(CHARTER_BODY),
    );
    let second = sync_charter_bundle(
        &charter_path,
        Some(&root.join(".kittify/charter")),
        &journal,
        false,
    );
    assert!(!second.stale_before);
    assert!(second.files_written.is_empty());
    assert_eq!(global_log_lines(journal_base.path()).len(), 2);
}

#[test]
fn the_staleness_owner_reports_the_reference_triple() {
    let temp = tempfile::tempdir().expect("temp");
    let root = temp.path();
    case(root, "fresh");
    let charter_path = root.join(".kittify/charter/charter.md");
    let metadata_path = root.join(".kittify/charter/metadata.yaml");

    let from_path = charter_bundle_staleness(Some(&charter_path), &metadata_path, None)
        .expect("staleness from the file");
    assert_eq!(
        from_path,
        CharterStaleness {
            stale: false,
            current_hash: sk_charter::hash_content(CHARTER_BODY),
            stored_hash: sk_kernel::python_value::PythonValue::Str(
                sk_kernel::PythonString::from_scalars(&sk_charter::hash_content(CHARTER_BODY))
            ),
        }
    );

    // Already-read content wins, and no second read happens: the path does not
    // even have to exist.
    let from_content = charter_bundle_staleness(
        Some(Path::new("/nonexistent/charter.md")),
        &metadata_path,
        Some("# Different\n"),
    )
    .expect("staleness from content");
    assert!(from_content.stale);

    // An absent metadata.yaml is stale, never a healthy verdict.
    let absent = charter_bundle_staleness(
        None,
        &root.join(".kittify/charter/missing-metadata.yaml"),
        Some(CHARTER_BODY),
    )
    .expect("absent metadata");
    assert!(absent.stale);
    assert_eq!(
        absent.stored_hash,
        sk_kernel::python_value::PythonValue::Str(sk_kernel::PythonString::default())
    );

    // No content and no path is the reference's ValueError, not an empty digest.
    match charter_bundle_staleness(None, &metadata_path, None) {
        Err(CharterSyncError::NoInput) => {}
        other => panic!("expected the reference's ValueError shape, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Paired process control: pinned Source child, then Native child
// ---------------------------------------------------------------------------

/// The Source child's script: assert the isolated environment, then run the
/// actual `ensure_charter_bundle_fresh` per case and report business facts.
const SOURCE_PROBE: &str = r#"
import json, logging, os, pathlib, sys

root = pathlib.Path(os.environ["SK_CF65_ROOT"])
cwd = pathlib.Path.cwd()
assert cwd == root, (str(cwd), str(root))
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
repo = pathlib.Path(os.environ["SK_CF65_SOURCE_ROOT"])
assert repo.is_absolute()
assert pathlib.Path(os.environ["PYTHONPATH"]) == repo / "src"
assert pathlib.Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
for key, name in [
    ("HOME", "home"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"), ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"), ("TMP", "tmp"), ("TEMP", "tmp"),
]:
    assert pathlib.Path(os.environ[key]) == root / name, (key, os.environ[key])
assert os.environ["PYTHONHASHSEED"] == "0", os.environ["PYTHONHASHSEED"]

from charter.activation.sync import ensure_charter_bundle_fresh

GLOBAL = root / ".kittify/encoding-provenance/global.jsonl"

# The reference logs instead of returning warnings; capture its own logger so the
# warning phase is a Source fact, not a Native guess.
WARNINGS = []


class _Collect(logging.Handler):
    def emit(self, record):
        WARNINGS.append(record.getMessage())


logger = logging.getLogger("charter.activation.sync")
logger.addHandler(_Collect(level=logging.WARNING))
logger.setLevel(logging.WARNING)


def footprint(base):
    return [str(path.relative_to(root)) for path in sorted(base.rglob("*"))]


def lines():
    if not GLOBAL.exists():
        return []
    return [line for line in GLOBAL.read_text(encoding="utf-8").splitlines() if line]


def prefixes(messages):
    out = []
    for message in messages:
        if message.startswith("Failed to evaluate charter bundle freshness: "):
            out.append("freshness")
        elif message.startswith("Charter auto-sync failed while refreshing extracted artifacts: "):
            out.append("auto-sync")
        else:
            out.append("other")
    return out


cases = []
for name in [
    "absent", "fresh", "stale", "bom-stale", "prose-is-a-directory",
    "metadata-is-a-directory",
]:
    case_root = root / "cases" / name
    before = footprint(case_root)
    records_before = len(lines())
    WARNINGS.clear()
    result = ensure_charter_bundle_fresh(case_root)
    records = lines()[records_before:]
    cases.append({
        "case": name,
        "present": result is not None,
        "synced": None if result is None else bool(result.synced),
        "stale_before": None if result is None else bool(result.stale_before),
        "files_written": None if result is None else list(result.files_written),
        "extraction_mode": None if result is None else result.extraction_mode,
        "error": None if result is None else (None if result.error is None else "present"),
        "canonical_root": None if result is None else str(result.canonical_root),
        "records": len(records),
        "record_paths": [
            str(pathlib.Path(json.loads(line)["file_path"]).relative_to(root)) for line in records
        ],
        "warning_prefixes": prefixes(WARNINGS),
        "footprint_unchanged": footprint(case_root) == before,
    })

payload = {"runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
           "cwd": str(cwd), "cases": cases}
print("SOURCE_RECEIPT " + json.dumps(payload))
"#;

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn isolated_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let repository = repository_root();
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("SK_CF65_SOURCE_ROOT", &repository)
        .env("PYTHONPATH", repository.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated::ROOTS {
        command.env(key, root.join(name));
    }
    for (key, name) in EXTRA_XDG {
        command.env(key, root.join(name));
    }
    command
}

fn run_source(root: &Path) -> Vec<JsonValue> {
    let output = isolated_command(root, support::python())
        .arg("-c")
        .arg(SOURCE_PROBE)
        .env("SK_CF65_ROOT", root)
        .output()
        .expect("run the pinned Source child");
    std::fs::write(root.join("source.stdout"), &output.stdout).expect("source stdout");
    std::fs::write(root.join("source.stderr"), &output.stderr).expect("source stderr");
    assert!(
        output.status.success(),
        "Source child failed ({:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Source stdout is UTF-8");
    let line = stdout
        .lines()
        .find(|line| line.starts_with("SOURCE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Source receipt in {stdout:?}"));
    let payload: JsonValue =
        serde_json::from_str(line["SOURCE_RECEIPT ".len()..].trim()).expect("receipt JSON");
    assert_eq!(payload["runner"], json!("source-python-3.11.15"));
    assert_eq!(payload["cwd"], json!(root.display().to_string()));
    payload["cases"].as_array().expect("cases").clone()
}

/// The Native child: this test binary re-executed with the fixture root.
#[test]
fn cf65_native_ensure_child() {
    let Some(root) = std::env::var_os("SK_CF65_NATIVE_ROOT").map(PathBuf::from) else {
        return; // ordinary run: the parent test drives this child
    };
    isolated::assert_isolated(&root);
    for (key, name) in EXTRA_XDG {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    assert_eq!(std::env::var("PYTHONHASHSEED").as_deref(), Ok("0"));
    let mut cases = Vec::new();
    for (name, _) in PAIRED_CASES {
        let case_root = root.join("cases").join(name);
        let before = footprint(&case_root);
        let records_before = global_log_lines(&root);
        let journal = source_cwd_journal();
        let result = ensure_charter_bundle_fresh(&case_root, None, &journal).expect("resolution");
        let records = global_log_lines(&root);
        let new_records = records[records_before.len()..].to_vec();
        let record_paths: Vec<String> = new_records
            .iter()
            .map(|line| {
                let record: JsonValue = serde_json::from_str(line).expect("record");
                record["file_path"]
                    .as_str()
                    .expect("file_path")
                    .trim_start_matches(&format!("{}/", root.display()))
                    .to_owned()
            })
            .collect();
        let warning_prefixes: Vec<&str> = result
            .as_ref()
            .map(|result| {
                result
                    .warnings
                    .iter()
                    .map(|warning| {
                        if warning.starts_with("Failed to evaluate charter bundle freshness: ") {
                            "freshness"
                        } else if warning.starts_with(
                            "Charter auto-sync failed while refreshing extracted artifacts: ",
                        ) {
                            "auto-sync"
                        } else {
                            "other"
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        cases.push(json!({
            "case": name,
            "present": result.is_some(),
            "synced": result.as_ref().map(|result| result.synced),
            "stale_before": result.as_ref().map(|result| result.stale_before),
            "files_written": result.as_ref().map(|result| result.files_written.clone()),
            "extraction_mode": result.as_ref().map(|result| result.extraction_mode.clone()),
            "error": result.as_ref().map(|result| result.error.as_ref().map(|_| "present")),
            "canonical_root": result
                .as_ref()
                .and_then(|result| result.canonical_root.as_ref())
                .map(|path| path.display().to_string()),
            "records": new_records.len(),
            "record_paths": record_paths,
            "warning_prefixes": warning_prefixes,
            "footprint_unchanged": footprint(&case_root) == before,
        }));
    }
    println!(
        "NATIVE_RECEIPT {}",
        json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "cases": cases,
        })
    );
}

fn run_native(root: &Path) -> Vec<JsonValue> {
    let executable = std::env::current_exe().expect("test binary");
    let output = isolated_command(root, executable)
        .args(["--exact", "cf65_native_ensure_child", "--nocapture"])
        .env("SK_CF65_NATIVE_ROOT", root)
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("native.stdout"), &output.stdout).expect("native stdout");
    std::fs::write(root.join("native.stderr"), &output.stderr).expect("native stderr");
    assert!(
        output.status.success(),
        "Native child failed ({:?}): {} {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("Native stdout is UTF-8");
    let line = stdout
        .lines()
        .find(|line| line.starts_with("NATIVE_RECEIPT "))
        .unwrap_or_else(|| panic!("no Native receipt in {stdout:?}"));
    let payload: JsonValue =
        serde_json::from_str(line["NATIVE_RECEIPT ".len()..].trim()).expect("receipt JSON");
    assert_eq!(payload["runner"], json!("native"));
    assert_eq!(payload["cwd"], json!(root.display().to_string()));
    payload["cases"].as_array().expect("cases").clone()
}

#[test]
fn source_and_native_agree_on_the_freshness_phases_and_record_placement() {
    let source_root = isolated::fixture("cf65-ensure-source");
    build_paired_cases(&source_root);
    let source = run_source(&source_root);

    // The intended Source cause is asserted BEFORE the Native run.
    let by_case = |cases: &[JsonValue], name: &str| -> JsonValue {
        cases
            .iter()
            .find(|case| case["case"] == json!(name))
            .unwrap_or_else(|| panic!("no {name} case"))
            .clone()
    };
    let absent = by_case(&source, "absent");
    assert_eq!(absent["present"], json!(false));
    assert_eq!(absent["records"], json!(0));
    assert_eq!(absent["warning_prefixes"], json!([]));
    let fresh = by_case(&source, "fresh");
    assert_eq!(fresh["present"], json!(true));
    assert_eq!(fresh["stale_before"], json!(false));
    assert_eq!(fresh["synced"], json!(false));
    assert_eq!(fresh["files_written"], json!([]));
    assert_eq!(fresh["extraction_mode"], json!(""));
    assert_eq!(fresh["error"], JsonValue::Null);
    assert_eq!(fresh["records"], json!(0), "a fresh bundle is not ingested");
    assert_eq!(fresh["warning_prefixes"], json!([]));
    let stale = by_case(&source, "stale");
    assert_eq!(stale["stale_before"], json!(true));
    assert_eq!(stale["records"], json!(1));
    assert_eq!(
        stale["record_paths"],
        json!(["cases/stale/.kittify/charter/charter.md"]),
        "the Source record names the read charter"
    );
    assert_eq!(stale["warning_prefixes"], json!([]));
    let bom = by_case(&source, "bom-stale");
    assert_eq!(bom["stale_before"], json!(true), "the hash is BOM-agnostic");
    assert_eq!(bom["error"], JsonValue::Null, "a BOM charter is readable");
    assert_eq!(bom["records"], json!(1));
    let prose_dir = by_case(&source, "prose-is-a-directory");
    assert_eq!(prose_dir["present"], json!(true));
    assert_eq!(prose_dir["error"], json!("present"));
    assert_eq!(prose_dir["stale_before"], json!(false));
    assert_eq!(prose_dir["records"], json!(0));
    assert_eq!(
        prose_dir["warning_prefixes"],
        json!(["freshness", "auto-sync"]),
        "the reference logs both phases"
    );
    let broken = by_case(&source, "metadata-is-a-directory");
    assert_eq!(broken["error"], json!("present"));
    assert_eq!(broken["stale_before"], json!(false));
    assert_eq!(
        broken["records"],
        json!(1),
        "the read succeeds (and is recorded) before the re-check fails"
    );
    assert_eq!(
        broken["warning_prefixes"],
        json!(["freshness", "auto-sync"])
    );
    for case in &source {
        assert_eq!(
            case["footprint_unchanged"],
            json!(true),
            "the retired sync writes no bundle file: {case}"
        );
    }

    let native_root = isolated::fixture("cf65-ensure-native");
    build_paired_cases(&native_root);
    let native = run_native(&native_root);

    for (name, _) in PAIRED_CASES {
        let source_case = by_case(&source, name);
        let native_case = by_case(&native, name);
        for field in [
            "present",
            "synced",
            "stale_before",
            "files_written",
            "extraction_mode",
            "error",
            "records",
            "record_paths",
            "warning_prefixes",
            "footprint_unchanged",
        ] {
            assert_eq!(
                source_case[field], native_case[field],
                "case {name}: field {field} differs (Source {source_case} vs Native {native_case})"
            );
        }
        let expected_root = if source_case["present"] == json!(true) {
            json!(source_root.join("cases").join(name).display().to_string())
        } else {
            JsonValue::Null
        };
        assert_eq!(
            source_case["canonical_root"], expected_root,
            "case {name}: Source resolved the case's own checkout (or none for absence)"
        );
        let expected_native_root = if expected_root == JsonValue::Null {
            JsonValue::Null
        } else {
            json!(native_root.join("cases").join(name).display().to_string())
        };
        assert_eq!(
            native_case["canonical_root"], expected_native_root,
            "case {name}: Native resolved the case's own checkout"
        );
    }
}
