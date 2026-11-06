//! CF85 — Source fast plain-ID read, ordinary one-read transport and cache precedence.
//!
//! Business controls over the pre-existing public identity/offering APIs:
//! the reference accepts a conservative unquoted `id:` line even when unrelated YAML
//! later in the file is malformed (the artifact repository still rejects the body);
//! a completed canonical read precedes the private fast cache; a completed canonical
//! `None` is not replaced by a later fast-eligible rewrite; the private fast cache
//! survives a same-instance abort while a new index re-reads the tree.
//!
//! The guarded pair runs the actual pinned Source and Native children in independent
//! business trees, each asserting its own roots before the first business read.

mod support;

use serde_json::{Value, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{CanonicalOfferingService, IdentityIndex, OfferingContext, OfferingRoots};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const KIND: ArtifactKind = ArtifactKind::Tactic;
const ABORTING: &str = "id: !!timestamp 2021-13-45\n";

const ROOTS: [(&str, &str); 11] = [
    ("HOME", "home"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
    ("TMPDIR", "tmp"),
    ("TMP", "tmp"),
    ("TEMP", "tmp"),
];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn built_in_root() -> PathBuf {
    repo().join("packs/built-in")
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(path, text).expect("fixture file");
}

fn write_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture directory");
    }
    fs::write(path, bytes).expect("fixture file");
}

fn tactic(id: &str) -> String {
    format!("id: {id}\nschema_version: \"1.0\"\nname: {id}\nsteps:\n  - title: Do the thing\n")
}

fn project_root() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().expect("temp dir");
    let project = directory.path().join("project");
    fs::create_dir_all(project.join(KIND.project_dir())).expect("project dir");
    (directory, project)
}

fn identity(project: &Path) -> IdentityIndex {
    IdentityIndex::from_roots(&built_in_root(), &[], Some(project))
}

fn project_tactics(project: &Path) -> PathBuf {
    project.join(KIND.project_dir())
}

fn declared(index: &IdentityIndex, token: &str) -> Result<Option<String>, String> {
    index
        .try_declared_id(KIND, token)
        .map_err(|abort| format!("{abort}"))
}

fn service(root: &Path) -> CanonicalOfferingService {
    CanonicalOfferingService::new(OfferingContext::new(
        root.to_path_buf(),
        OfferingRoots {
            built_in: Some(built_in_root()),
            org_roots: vec![root.join("org")],
            project: None,
        },
        None,
    ))
}

fn scalar(ids: &[sk_kernel::PythonString]) -> Vec<String> {
    ids.iter()
        .map(|id| id.to_scalar_string().expect("scalar receipt"))
        .collect()
}

fn cf85_ids(root: &Path) -> Vec<String> {
    scalar(&service(root).artifact_ids(KIND).expect("repository load"))
        .into_iter()
        .filter(|id| id.starts_with("cf85-"))
        .collect()
}

/// A conservative plain `id:` is returned even though the rest of the body is malformed,
/// for both LF and CRLF transport; the artifact repository still rejects the body.
#[test]
fn identity_accepts_a_plain_id_despite_unrelated_malformed_body() {
    let (_directory, project) = project_root();
    let tactics = project_tactics(&project);
    write(
        &tactics.join("sigma.tactic.yaml"),
        "id: cf85-sigma\nbroken: [unclosed\n",
    );
    write_bytes(
        &tactics.join("crlf.tactic.yaml"),
        b"id: cf85-crlf\r\nbroken: [unclosed\r\n",
    );

    let index = identity(&project);
    assert_eq!(
        declared(&index, "sigma"),
        Ok(Some("cf85-sigma".to_owned())),
        "the reference's fast probe yields the plain id without a full parse"
    );
    assert_eq!(
        declared(&index, "cf85-sigma"),
        Ok(Some("cf85-sigma".to_owned()))
    );
    assert_eq!(
        declared(&index, "crlf"),
        Ok(Some("cf85-crlf".to_owned())),
        "ordinary read_text normalises CRLF before the probe"
    );

    // The rewritten malformed body is seen by a NEW index; a same-index table is warm.
    write(
        &tactics.join("sigma.tactic.yaml"),
        "id: cf85-sigma-two\nbroken: [x\n",
    );
    assert_eq!(
        declared(&identity(&project), "sigma"),
        Ok(Some("cf85-sigma-two".to_owned()))
    );
}

/// The canonical repository admission is unchanged: the malformed body is skipped.
#[test]
fn repository_admission_still_skips_the_malformed_body() {
    let directory = tempfile::tempdir().expect("temp dir");
    let root = directory.path();
    write(
        &root.join("org/tactics/sigma.tactic.yaml"),
        "id: cf85-admitted\nbroken: [unclosed\n",
    );
    let index = IdentityIndex::from_roots(&built_in_root(), &[root.join("org")], None);
    assert_eq!(
        declared(&index, "sigma"),
        Ok(Some("cf85-admitted".to_owned())),
        "identity accepts the conservative plain id"
    );
    assert!(
        cf85_ids(root).is_empty(),
        "the artifact repository still rejects the malformed body"
    );
}

/// Canonical completed reads (id or caught `None`) precede the private fast cache; the
/// private fast cache survives the abort; a new index reads the tree as it is now.
#[test]
fn canonical_completed_read_precedes_the_private_fast_cache_across_an_abort() {
    let (_directory, project) = project_root();
    let tactics = project_tactics(&project);
    write(&tactics.join("a.tactic.yaml"), &tactic("cf85-fastone"));
    write(
        &tactics.join("q.tactic.yaml"),
        "id: \"cf85-quoted\"\nschema_version: \"1.0\"\n",
    );
    write(&tactics.join("n.tactic.yaml"), "id: [unclosed\n");
    write(&tactics.join("z.tactic.yaml"), ABORTING);
    let index = identity(&project);
    let abort = index
        .try_declared_id(KIND, "z")
        .expect_err("the reference scan aborts on the uncaught constructor");
    let abort = abort.abort().expect("a Source constructor abort");
    assert_eq!(abort.python, "ValueError");

    // Every completed file is rewritten before the same-instance retry; the aborting file
    // is repaired and a new file is born.
    write(&tactics.join("a.tactic.yaml"), "id: cf85-another\n");
    write(
        &tactics.join("q.tactic.yaml"),
        "id: cf85-fastq\nbroken: [unclosed\n",
    );
    write(&tactics.join("n.tactic.yaml"), "id: cf85-sigma-two\n");
    write(&tactics.join("z.tactic.yaml"), &tactic("cf85-zed"));
    write(&tactics.join("born.tactic.yaml"), &tactic("cf85-born"));

    assert_eq!(
        declared(&index, "a"),
        Ok(Some("cf85-fastone".to_owned())),
        "the private fast cache is retained across the abort"
    );
    assert_eq!(
        declared(&index, "q"),
        Ok(Some("cf85-quoted".to_owned())),
        "a completed canonical read precedes a later fast-eligible rewrite"
    );
    assert_eq!(
        declared(&index, "n"),
        Ok(None),
        "a completed canonical None is not replaced by a fast-eligible rewrite"
    );
    assert_eq!(
        declared(&index, "z"),
        Ok(Some("cf85-zed".to_owned())),
        "the uncaught read is retried after repair"
    );
    assert_eq!(
        declared(&index, "born"),
        Ok(None),
        "a file born after the failed scan is outside the pass boundary"
    );

    let fresh = identity(&project);
    assert_eq!(declared(&fresh, "a"), Ok(Some("cf85-another".to_owned())));
    assert_eq!(
        declared(&fresh, "q"),
        Ok(Some("cf85-fastq".to_owned())),
        "a new index takes the fast path on the malformed body"
    );
    assert_eq!(declared(&fresh, "n"), Ok(Some("cf85-sigma-two".to_owned())));
    assert_eq!(declared(&fresh, "born"), Ok(Some("cf85-born".to_owned())));
}

/// The same conservative recognizer covers the agent-profile `profile-id` field.
#[test]
fn fast_probe_uses_the_kind_id_field() {
    let (_directory, project) = project_root();
    let profiles = project.join(ArtifactKind::AgentProfile.project_dir());
    write(
        &profiles.join("ivan-copy.agent.yaml"),
        "profile-id: cf85-profile\nbroken: [unclosed\n",
    );
    let index = identity(&project);
    assert_eq!(
        index
            .try_declared_id(ArtifactKind::AgentProfile, "ivan-copy")
            .expect("profile kind scans"),
        Some("cf85-profile".to_owned())
    );
}

/// A read decode failure escapes at the read phase with the reference class and is retried.
#[test]
fn decode_error_escapes_and_is_not_cached() {
    let (_directory, project) = project_root();
    let tactics = project_tactics(&project);
    write_bytes(
        &tactics.join("bad.tactic.yaml"),
        b"id: cf85-bad\n\xff\xfe\n",
    );
    write(&tactics.join("ok.tactic.yaml"), &tactic("cf85-ok"));
    let index = identity(&project);
    let abort = index
        .try_declared_id(KIND, "bad")
        .expect_err("the decode failure escapes the read phase");
    let abort = abort.abort().expect("a Source read abort");
    assert_eq!(abort.python, "UnicodeDecodeError");
    assert!(
        abort
            .message
            .contains("'utf-8' codec can't decode byte 0xff"),
        "the pinned CPython decode message is retained: {abort}"
    );

    // The uncaught decode is not cached: repairing the file lets the SAME index retry it.
    write(&tactics.join("bad.tactic.yaml"), &tactic("cf85-bad-fixed"));
    assert_eq!(
        declared(&index, "bad"),
        Ok(Some("cf85-bad-fixed".to_owned()))
    );
}

/// A read `OSError` is the reference's caught failure: the fallback retry completes it as
/// `None`, the scan continues, and the same pass does not retry it.
#[cfg(unix)]
#[test]
fn unreadable_file_is_a_completed_none_after_the_retry() {
    let (_directory, project) = project_root();
    let tactics = project_tactics(&project);
    let target = tactics.join("target.tactic.yaml");
    std::os::unix::fs::symlink(&target, tactics.join("gone.tactic.yaml"))
        .expect("dangling symlink");
    write(&tactics.join("ok.tactic.yaml"), &tactic("cf85-ok"));

    let index = identity(&project);
    assert_eq!(
        declared(&index, "gone"),
        Ok(None),
        "the caught OSError is a completed None"
    );
    assert_eq!(declared(&index, "ok"), Ok(Some("cf85-ok".to_owned())));

    // The completed None is not retried by the same pass; a new index reads the now-present target.
    write(&target, &tactic("cf85-late"));
    assert_eq!(declared(&index, "gone"), Ok(None));
    assert_eq!(
        declared(&identity(&project), "gone"),
        Ok(Some("cf85-late".to_owned()))
    );
}

// ---------------------------------------------------------------- pair ------

fn command_env(command: &mut Command, root: &Path) {
    command
        .env_clear()
        .current_dir(root)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("CF85_PAIR_ROOT", root)
        .env("CF85_PAIR_SOURCE", repo())
        .env("PYTHONPATH", repo().join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo().join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("LC_ALL", "C.UTF-8")
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in ROOTS {
        fs::create_dir_all(root.join(name)).expect("env root");
        command.env(key, root.join(name));
    }
}

fn native_guard(root: &Path) {
    assert!(root.is_absolute());
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in ROOTS {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name))
        );
    }
    for (key, expected) in [
        ("CF85_PAIR_SOURCE", repo()),
        ("PYTHONPATH", repo().join("src")),
        ("SPEC_KITTY_PACKS_ROOT", repo().join("packs")),
    ] {
        assert_eq!(std::env::var_os(key).map(PathBuf::from), Some(expected));
    }
    assert_eq!(std::env::var("PYTHONHASHSEED").unwrap(), "0");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    let stream = sk_kernel::python_stream::StreamPolicy::for_stderr().unwrap();
    assert_eq!(stream.encoding(), "utf-8");
    assert_eq!(stream.errors(), "backslashreplace");
}

fn native_business(root: &Path) -> Value {
    let folder = root.join("org/tactics");
    fs::create_dir_all(&folder).unwrap();
    let admission_folder = root.join("admission/org/tactics");
    fs::create_dir_all(&admission_folder).unwrap();
    let write_tactic = |name: &str, id: &str| {
        fs::write(folder.join(name), tactic(id)).unwrap();
    };
    write_tactic("a.tactic.yaml", "cf85-fastone");
    fs::write(
        folder.join("q.tactic.yaml"),
        "id: \"cf85-quoted\"\nschema_version: \"1.0\"\n",
    )
    .unwrap();
    fs::write(folder.join("n.tactic.yaml"), "id: [unclosed\n").unwrap();
    fs::write(folder.join("z.tactic.yaml"), ABORTING).unwrap();
    fs::write(
        admission_folder.join("sigma.tactic.yaml"),
        "id: cf85-admitted\nbroken: [unclosed\n",
    )
    .unwrap();

    let index = IdentityIndex::from_roots(&built_in_root(), &[root.join("org")], None);
    let abort = index
        .try_declared_id(KIND, "z")
        .expect_err("the source scan aborts");
    let class = abort
        .abort()
        .expect("a Source constructor abort")
        .python
        .to_owned();

    fs::write(folder.join("a.tactic.yaml"), "id: cf85-another\n").unwrap();
    fs::write(
        folder.join("q.tactic.yaml"),
        "id: cf85-fastq\nbroken: [unclosed\n",
    )
    .unwrap();
    fs::write(folder.join("n.tactic.yaml"), "id: cf85-sigma-two\n").unwrap();
    write_tactic("z.tactic.yaml", "cf85-zed");
    write_tactic("born.tactic.yaml", "cf85-born");

    let same = |token: &str| {
        index
            .try_declared_id(KIND, token)
            .expect("same-instance retry")
    };
    let fresh_index = IdentityIndex::from_roots(&built_in_root(), &[root.join("org")], None);
    let fresh = |token: &str| {
        fresh_index
            .try_declared_id(KIND, token)
            .expect("fresh index")
    };
    let admission =
        IdentityIndex::from_roots(&built_in_root(), &[root.join("admission/org")], None);
    let admitted = admission
        .try_declared_id(KIND, "sigma")
        .expect("admission index");
    let mut repository_ids = cf85_ids(&root.join("admission"));
    repository_ids.sort();
    json!({
        "class": class,
        "same": {
            "a": same("a"),
            "q": same("q"),
            "n": same("n"),
            "z": same("z"),
            "born": same("born"),
        },
        "fresh": {
            "a": fresh("a"),
            "q": fresh("q"),
            "n": fresh("n"),
            "born": fresh("born"),
        },
        "admission": {
            "declared": admitted,
            "repository_ids": repository_ids,
        },
    })
}

const SOURCE: &str = r#"
import json, os, sys
from pathlib import Path
root = Path(os.environ['CF85_PAIR_ROOT'])
expected = Path(os.environ['CF85_PAIR_SOURCE'])
assert root.is_absolute() and expected.is_absolute() and Path.cwd() == root
for key, name in [('HOME','home'),('XDG_CONFIG_HOME','config'),('XDG_CACHE_HOME','cache'),('XDG_DATA_HOME','data'),('XDG_STATE_HOME','state'),('XDG_RUNTIME_DIR','runtime'),('XDG_CONFIG_DIRS','config-dirs'),('XDG_DATA_DIRS','data-dirs'),('TMPDIR','tmp'),('TMP','tmp'),('TEMP','tmp')]:
    assert Path(os.environ[key]) == root / name
assert Path(os.environ['PYTHONPATH']) == expected / 'src'
assert Path(os.environ['SPEC_KITTY_PACKS_ROOT']) == expected / 'packs'
assert os.environ['PYTHONHASHSEED'] == '0'
assert sys.version_info[:3] == (3,11,15)
assert (sys.getfilesystemencoding(),sys.getfilesystemencodeerrors()) == ('utf-8','surrogateescape')
assert (sys.stderr.encoding,sys.stderr.errors) == ('utf-8','backslashreplace')
import charter
assert Path(charter.__file__).resolve().is_relative_to(expected / 'src')
from charter.activation import identity_index as ii
from charter.offering.artifact_kinds import ArtifactKind
from charter.offering.service import CharterOfferingService
K = ArtifactKind.TACTIC
folder = root / 'org/tactics'
admission_folder = root / 'admission/org/tactics'
folder.mkdir(parents=True)
admission_folder.mkdir(parents=True)
def tactic(name): return 'id: %s\nschema_version: "1.0"\nname: %s\nsteps:\n  - title: Do the thing\n' % (name, name)
(folder/'a.tactic.yaml').write_text(tactic('cf85-fastone'), encoding='utf-8')
(folder/'q.tactic.yaml').write_text('id: "cf85-quoted"\nschema_version: "1.0"\n', encoding='utf-8')
(folder/'n.tactic.yaml').write_text('id: [unclosed\n', encoding='utf-8')
(folder/'z.tactic.yaml').write_text('id: !!timestamp 2021-13-45\n', encoding='utf-8')
(admission_folder/'sigma.tactic.yaml').write_text('id: cf85-admitted\nbroken: [unclosed\n', encoding='utf-8')
def index(org):
    return ii.IdentityIndex.from_roots(offering_root=expected / 'src/charter/offering', org_roots=[org], layer_roots=None)
idx = index(root/'org')
try:
    idx.declared_id(K, 'z')
except ValueError as error:
    assert 'month must be in 1..12' in str(error)
    category = type(error).__name__
else: raise AssertionError('the scan must abort')
(folder/'a.tactic.yaml').write_text('id: cf85-another\n', encoding='utf-8')
(folder/'q.tactic.yaml').write_text('id: cf85-fastq\nbroken: [unclosed\n', encoding='utf-8')
(folder/'n.tactic.yaml').write_text('id: cf85-sigma-two\n', encoding='utf-8')
(folder/'z.tactic.yaml').write_text(tactic('cf85-zed'), encoding='utf-8')
(folder/'born.tactic.yaml').write_text(tactic('cf85-born'), encoding='utf-8')
fresh_idx = index(root/'org')
admission_idx = index(root/'admission/org')
repository_ids = sorted(item.id for item in CharterOfferingService(org_roots=[root/'admission/org']).tactics.list_all() if item.id.startswith('cf85-'))
receipt = {
    'class': category,
    'same': {token: idx.declared_id(K, token) for token in ['a','q','n','z','born']},
    'fresh': {token: fresh_idx.declared_id(K, token) for token in ['a','q','n','born']},
    'admission': {'declared': admission_idx.declared_id(K, 'sigma'), 'repository_ids': repository_ids},
}
(root/'receipt.json').write_text(json.dumps(receipt, sort_keys=True), encoding='utf-8')
"#;

#[test]
fn guarded_source_native_fast_id_pair() {
    if let Some(root) = std::env::var_os("CF85_PAIR_ROOT").map(PathBuf::from) {
        native_guard(&root);
        fs::write(
            root.join("receipt.json"),
            serde_json::to_vec(&native_business(&root)).unwrap(),
        )
        .unwrap();
        return;
    }
    let source = tempfile::tempdir().unwrap();
    let native = tempfile::tempdir().unwrap();
    assert_ne!(source.path(), native.path());

    let mut source_cmd = Command::new(support::python());
    command_env(&mut source_cmd, source.path());
    let output = source_cmd.args(["-c", SOURCE]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source_receipt: Value =
        serde_json::from_slice(&fs::read(source.path().join("receipt.json")).unwrap()).unwrap();

    let mut native_cmd = Command::new(std::env::current_exe().unwrap());
    command_env(&mut native_cmd, native.path());
    let output = native_cmd
        .args([
            "--exact",
            "guarded_source_native_fast_id_pair",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let native_receipt: Value =
        serde_json::from_slice(&fs::read(native.path().join("receipt.json")).unwrap()).unwrap();

    assert_eq!(source_receipt, native_receipt, "business receipts differ");
    assert_eq!(source_receipt["class"], "ValueError");
    assert_eq!(source_receipt["same"]["a"], "cf85-fastone");
    assert_eq!(source_receipt["same"]["q"], "cf85-quoted");
    assert!(source_receipt["same"]["n"].is_null());
    assert_eq!(source_receipt["same"]["z"], "cf85-zed");
    assert!(source_receipt["same"]["born"].is_null());
    assert_eq!(source_receipt["fresh"]["a"], "cf85-another");
    assert_eq!(source_receipt["fresh"]["q"], "cf85-fastq");
    assert_eq!(source_receipt["fresh"]["n"], "cf85-sigma-two");
    assert_eq!(source_receipt["fresh"]["born"], "cf85-born");
    assert_eq!(source_receipt["admission"]["declared"], "cf85-admitted");
    assert_eq!(source_receipt["admission"]["repository_ids"], json!([]));
}
