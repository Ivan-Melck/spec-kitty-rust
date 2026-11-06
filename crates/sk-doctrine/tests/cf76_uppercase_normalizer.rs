//! CF76 — Source `normalize_directive_id` fidelity and the pinned Kernel uppercase.
//!
//! The pinned Source normaliser (`charter.offering.drg.migration.id_normalizer`)
//! is three phases over a Python `str`:
//!
//! 1. `re.match(r"^DIRECTIVE_\d+$", raw)` returns the input unchanged (`\d` is
//!    Unicode `Nd`, `$` also matches before one final newline);
//! 2. `re.match(r"^(\d+)", raw)` takes the leading `Nd` digits and pads them to
//!    three with ASCII zeros;
//! 3. otherwise `raw.upper().replace("-", "_")` — CPython 3.11.15's full
//!    `str.upper()`, not Rust's newer-Unicode `char::to_uppercase()`.
//!
//! Phase 1 is output-equivalent to phase 3 for every string it accepts
//! (`DIRECTIVE_` and `Nd` digits have no case and no hyphen), so this file pins
//! the observable outputs rather than the internal branch.
//!
//! EXISTING-API SECTION (kept deliberately first): every expectation below was
//! recorded from the pinned Source before the Native change and is exercised
//! through APIs that already existed (`normalize_directive_id`,
//! `IdentityIndex::unindexed_key(_exact)`), so it is a real before-fix red, not
//! a fabricated one. APIs that only exist after this change are asserted in the
//! second half, whose Source expectation is the same probe.

use serde_json::{Value, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{
    IdentityIndex, PythonString, normalize_directive_id, normalize_directive_id_exact,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

fn scalars(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}

fn exact(units: &[u32]) -> PythonString {
    PythonString::from_codepoints(units).expect("valid code points")
}

fn scalar(value: &PythonString) -> String {
    value.to_scalar_string().expect("scalar text")
}

/// Source probe `normalize.*` rows: the Unicode `Nd` prefix and zfill phase.
#[test]
fn source_prefix_scan_uses_unicode_nd_and_ascii_zfill() {
    // `\d` is `Nd`, so an Arabic-Indic or fullwidth prefix is a number.
    assert_eq!(
        normalize_directive_id("\u{660}\u{662}\u{664}-my-slug"),
        "DIRECTIVE_\u{660}\u{662}\u{664}"
    );
    assert_eq!(normalize_directive_id("\u{663}-x"), "DIRECTIVE_00\u{663}");
    assert_eq!(
        normalize_directive_id("\u{ff11}\u{ff12}\u{ff13}"),
        "DIRECTIVE_\u{ff11}\u{ff12}\u{ff13}"
    );
    assert_eq!(
        normalize_directive_id("\u{967}\u{968}\u{969}"),
        "DIRECTIVE_\u{967}\u{968}\u{969}"
    );
    // ASCII behaviour that already agreed.
    assert_eq!(normalize_directive_id("24"), "DIRECTIVE_024");
    assert_eq!(normalize_directive_id("024"), "DIRECTIVE_024");
    assert_eq!(normalize_directive_id("0024"), "DIRECTIVE_0024");
    assert_eq!(normalize_directive_id("024 "), "DIRECTIVE_024");
    assert_eq!(normalize_directive_id(" 024"), " 024");
    assert_eq!(
        normalize_directive_id("DIRECTIVE_\u{660}\u{662}\u{664}"),
        "DIRECTIVE_\u{660}\u{662}\u{664}"
    );
}

/// Source probe `normalize.canonical_newline` row: the `$` anchor and the
/// fallback agree, and neither drops the trailing newline.
#[test]
fn source_keeps_a_trailing_newline_and_other_exact_units() {
    assert_eq!(normalize_directive_id("DIRECTIVE_024\n"), "DIRECTIVE_024\n");
    assert_eq!(
        normalize_directive_id("DIRECTIVE_024\n\n"),
        "DIRECTIVE_024\n\n"
    );
    assert_eq!(normalize_directive_id("\0abc"), "\0ABC");
    assert_eq!(normalize_directive_id("x\u{378}y"), "X\u{378}Y");
    assert_eq!(normalize_directive_id(""), "");
    assert_eq!(normalize_directive_id("-"), "_");
}

/// Source probe `normalize.sharp_s*`, `hub_slug`, `mixed_case` rows: the
/// fallback is the interpreter's full `str.upper()` then an ASCII hyphen fold.
#[test]
fn source_fallback_uses_full_ucd14_uppercase_then_ascii_hyphen_fold() {
    assert_eq!(normalize_directive_id("\u{df}-\u{fb01}"), "SS_FI");
    assert_eq!(normalize_directive_id("\u{3c0}-slug"), "\u{3a0}_SLUG");
    assert_eq!(
        normalize_directive_id("use-c4-model-techniques"),
        "USE_C4_MODEL_TECHNIQUES"
    );
    assert_eq!(normalize_directive_id("directive_024"), "DIRECTIVE_024");
    assert_eq!(normalize_directive_id("Directive_24"), "DIRECTIVE_24");
    assert_eq!(normalize_directive_id("already_snake"), "ALREADY_SNAKE");
}

/// Source probe `upper.rams_horn_0264` / `upper.l_bar_019b`: the pinned Unicode
/// assignment has NO capital for these; Rust's newer table does. Substituting
/// the Rust mapping would silently invent a directive id the Source never emits.
#[test]
fn source_uppercase_is_pinned_ucd14_not_rusts_newer_table() {
    assert_eq!(normalize_directive_id("\u{264}"), "\u{264}");
    assert_eq!(normalize_directive_id("\u{19b}"), "\u{19b}");
    assert_eq!(normalize_directive_id("\u{264}\u{19b}"), "\u{264}\u{19b}");
}

/// Source probe `normalize.surrogate_mixed` / `surrogate_pair_units` rows,
/// through the pre-existing exact API: a directive token that carries a
/// surrogate unit is still `str.upper()`-ed, and its units are preserved. The
/// temporary Native behaviour (bare Cs drops and an ASCII hyphen fold only)
/// left the cased scalars lowercase, which this pins against.
#[test]
fn exact_unknown_directive_key_uppercases_around_surrogate_units() {
    let mixed = exact(&[0x61, 0xd800, 0x62, 0x2d, 0x63]);
    assert_eq!(
        IdentityIndex::unindexed_key_exact(Some(ArtifactKind::Directive), &mixed).codepoints(),
        &[0x41, 0xd800, 0x42, 0x5f, 0x43]
    );
    let pair = exact(&[0xd83d, 0xde00, 0x2d, 0x61]);
    assert_eq!(
        IdentityIndex::unindexed_key_exact(Some(ArtifactKind::Directive), &pair).codepoints(),
        &[0xd83d, 0xde00, 0x5f, 0x41]
    );
    let expanded = exact(&[0xdf, 0xd800]);
    assert_eq!(
        IdentityIndex::unindexed_key_exact(Some(ArtifactKind::Directive), &expanded).codepoints(),
        &[0x53, 0x53, 0xd800]
    );
}

/// The scalar adapter and the exact path are the same policy, and a non-directive
/// kind is never normalised.
#[test]
fn scalar_adapter_and_other_kinds_stay_on_the_same_policy() {
    assert_eq!(
        IdentityIndex::unindexed_key(Some(ArtifactKind::Directive), "\u{df}-\u{fb01}"),
        "SS_FI"
    );
    assert_eq!(
        IdentityIndex::unindexed_key(Some(ArtifactKind::Directive), "\u{660}\u{662}\u{664}-slug"),
        "DIRECTIVE_\u{660}\u{662}\u{664}"
    );
    assert_eq!(
        IdentityIndex::unindexed_key(Some(ArtifactKind::Tactic), "\u{df}-x"),
        "\u{df}-x"
    );
    assert_eq!(IdentityIndex::unindexed_key(None, "keep-Me"), "keep-Me");
    assert_eq!(scalar(&scalars("text")), "text");
}

// ---------------------------------------------------------------------------
// New exact API (same Source expectations as the probe above).
// ---------------------------------------------------------------------------

#[test]
fn exact_normalizer_keeps_exact_units_end_to_end() {
    let cs = exact(&[0x61, 0xd800, 0x62, 0x2d, 0x63]);
    assert_eq!(
        normalize_directive_id_exact(&cs).codepoints(),
        &[0x41, 0xd800, 0x42, 0x5f, 0x43]
    );
    let nd = exact(&[0x660, 0x662, 0x664, 0x2d, 0x73]);
    assert_eq!(
        normalize_directive_id_exact(&nd).codepoints(),
        &[
            0x44, 0x49, 0x52, 0x45, 0x43, 0x54, 0x49, 0x56, 0x45, 0x5f, 0x660, 0x662, 0x664
        ]
    );
    let single = exact(&[0x663, 0x2d, 0x78]);
    assert_eq!(
        normalize_directive_id_exact(&single).codepoints(),
        &[
            0x44, 0x49, 0x52, 0x45, 0x43, 0x54, 0x49, 0x56, 0x45, 0x5f, 0x30, 0x30, 0x663
        ]
    );
    assert_eq!(
        normalize_directive_id_exact(&scalars("DIRECTIVE_024\n")).codepoints(),
        scalars("DIRECTIVE_024\n").codepoints()
    );
    // The one policy: scalar adapter == exact owner on scalar input.
    for token in ["\u{df}-\u{fb01}", "\u{264}", "use-c4-model", " 024", "24"] {
        assert_eq!(
            normalize_directive_id(token),
            scalar(&normalize_directive_id_exact(&scalars(token))),
            "{token}"
        );
    }
}

const DIRECTIVE: &str = concat!(
    "schema_version: \"1.0\"\n",
    "id: {id}\n",
    "title: {title}\n",
    "intent: {intent}\n",
    "enforcement: required\n",
);

fn write_file(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().expect("parent")).expect("fixture directory");
    fs::write(path, contents).expect("fixture file");
}

fn directive(id: &str, title: &str) -> String {
    DIRECTIVE
        .replace("{id}", id)
        .replace("{title}", title)
        .replace("{intent}", "Fixture intent.")
}

/// A fresh business tree with two built-in, one org and four project directives.
fn write_fixture(root: &Path) -> PathBuf {
    let fixture = root.join("fixture");
    let built_in = fixture.join("packs/built-in");
    let org = fixture.join("org-pack");
    for directory in ["directives", "tactics"] {
        fs::create_dir_all(built_in.join(directory)).expect("built-in directory");
    }
    fs::create_dir_all(org.join("directives")).expect("org directory");
    fs::create_dir_all(fixture.join(".kittify/charter-packs/directive"))
        .expect("project directory");
    write_file(
        &fixture,
        ".kittify/charter/charter.md",
        "# Fixture Charter\n",
    );
    write_file(
        &fixture,
        "packs/built-in/directives/024-locality.directive.yaml",
        &directive("DIRECTIVE_024", "Locality"),
    );
    write_file(
        &fixture,
        "packs/built-in/directives/040-bug.directive.yaml",
        &directive("DIRECTIVE_040", "Bug Discipline"),
    );
    write_file(
        &fixture,
        "org-pack/directives/777-org.directive.yaml",
        &directive("DIRECTIVE_777", "Org Overlay"),
    );
    write_file(
        &fixture,
        ".kittify/charter-packs/directive/use-c4.directive.yaml",
        &directive("USE_C4_MODEL_TECHNIQUES", "C4 Hub"),
    );
    write_file(
        &fixture,
        ".kittify/charter-packs/directive/alpha-beta.directive.yaml",
        &directive("DIRECTIVE_889", "Stem Alpha"),
    );
    write_file(
        &fixture,
        ".kittify/charter-packs/directive/stem-check.directive.yaml",
        &directive("alpha-beta", "Declared Alpha"),
    );
    write_file(
        &fixture,
        ".kittify/charter-packs/directive/sharp.directive.yaml",
        &directive("SS", "Sharp S"),
    );
    fixture
}

fn hex(units: &[u32]) -> String {
    units
        .iter()
        .map(|code| format!("{code:04x}"))
        .collect::<Vec<_>>()
        .join("-")
}

/// The identity questions both children ask of their own fixture.
fn index_keys(fixture: &Path) -> Value {
    let built_in = fixture.join("packs/built-in");
    let org = fixture.join("org-pack");
    let project = fixture.join(".kittify/charter-packs");
    let index = IdentityIndex::from_roots(&built_in, &[org], Some(&project));
    let directive_kind = ArtifactKind::Directive;
    let lookup = |token: &str, selection: bool| {
        hex(index
            .lookup_key_exact(Some(directive_kind), &scalars(token), selection)
            .codepoints())
    };
    let declared = |token: &str| {
        index
            .declared_id_exact(directive_kind, &scalars(token))
            .map(|id| hex(id.codepoints()))
            .unwrap_or_default()
    };
    let cs = exact(&[0x61, 0xd800, 0x62, 0x2d, 0x63]);
    json!({
        "alpha_beta_plain": lookup("alpha-beta", false),
        "alpha_beta_selection": lookup("alpha-beta", true),
        "normalized_number": lookup("777", false),
        "upper_declared": lookup("\u{df}", false),
        "hub_stem": lookup("use-c4-model-techniques", false),
        "nd_unknown": lookup("\u{660}\u{662}\u{664}", false),
        "rams_unknown": lookup("\u{264}-slug", false),
        "cs_unknown": hex(IdentityIndex::unindexed_key_exact(Some(directive_kind), &cs).codepoints()),
        "declared_number": declared("777"),
        "declared_hub": declared("use-c4-model-techniques"),
        "declared_unknown": declared("\u{264}"),
        "has_declared_ss": index.has_declared_id_exact(directive_kind, &scalars("ss")),
        "stem_for_ss": index
            .stem_for_declared_id_exact(directive_kind, &scalars("SS"))
            .map(|stem| hex(stem.codepoints()))
            .unwrap_or_default(),
        "stem_unknown": index
            .stem_exact(directive_kind, &scalars("\u{264}"))
            .map(|stem| hex(stem.codepoints()))
            .unwrap_or_default(),
    })
}

/// The `str.upper()` vectors both children run (label -> exact code units).
fn upper_vectors() -> Vec<(&'static str, PythonString)> {
    vec![
        ("sharp_s", exact(&[0xdf])),
        ("ligature_fi", exact(&[0xfb01])),
        ("greek_dasia", exact(&[0x1f80])),
        ("greek_iota", exact(&[0x1fbe])),
        ("j_caron", exact(&[0x1f0])),
        ("rams_horn", exact(&[0x264])),
        ("l_bar", exact(&[0x19b])),
        ("nul", exact(&[0x0, 0x61])),
        ("noncharacter", exact(&[0x78, 0x378, 0x79])),
        ("astral", exact(&[0x1f600, 0x61])),
        ("surrogate_mixed", exact(&[0x61, 0xd800, 0xdf, 0x2d, 0x62])),
        ("surrogate_pair", exact(&[0xd83d, 0xde00, 0x2d, 0x61])),
    ]
}

/// The directive-id vectors both children run (label -> exact code units).
fn normalize_vectors() -> Vec<(&'static str, PythonString)> {
    vec![
        ("sharp", exact(&[0xdf])),
        ("sharp_hyphen_ligature", exact(&[0xdf, 0x2d, 0xfb01])),
        ("greek_slug", exact(&[0x3c0, 0x2d, 0x73])),
        ("nd_prefix", exact(&[0x660, 0x662, 0x664, 0x2d, 0x73])),
        ("nd_single", exact(&[0x663, 0x2d, 0x78])),
        ("canonical_newline", scalars("DIRECTIVE_024\n")),
        ("canonical_two_newlines", scalars("DIRECTIVE_024\n\n")),
        ("surrogate_mixed", exact(&[0x61, 0xd800, 0x62, 0x2d, 0x63])),
        ("rams", exact(&[0x264])),
        ("hub", scalars("use-c4-model-techniques")),
        ("leading_space", scalars(" 024")),
        ("nul", exact(&[0x0, 0x61, 0x62, 0x63])),
    ]
}

fn hex_map(
    vectors: Vec<(&'static str, PythonString)>,
    apply: impl Fn(&PythonString) -> PythonString,
) -> Value {
    let mut map = serde_json::Map::new();
    for (label, value) in vectors {
        map.insert(
            label.to_owned(),
            Value::String(hex(apply(&value).codepoints())),
        );
    }
    Value::Object(map)
}

fn native_pair(fixture: &Path) -> Value {
    json!({
        "upper": hex_map(upper_vectors(), |value| value.upper()),
        "normalize": hex_map(normalize_vectors(), normalize_directive_id_exact),
        "keys": index_keys(fixture),
    })
}

const ROOT_ENV: &str = "CF76_PAIR_ROOT";
const REPO_ENV: &str = "CF76_PAIR_REPO";
const FIXTURE_ENV: &str = "CF76_PAIR_FIXTURE";
const MODE_ENV: &str = "CF76_CHILD_MODE";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
}

/// The seven XDG variables plus the three temporary-directory spellings.
fn env_roots() -> [(&'static str, &'static str); 12] {
    [
        ("HOME", "home"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CONFIG_DIRS", "config_dirs"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_DATA_DIRS", "data_dirs"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_RUNTIME_DIR", "runtime"),
        ("TMPDIR", "tmp"),
        ("TMP", "tmp"),
        ("TEMP", "tmp"),
        ("CF76_ROOT", "root"),
    ]
}

/// Both business children assert their own roots BEFORE any business read.
fn assert_child_environment(root: &Path, repo: &Path, fixture: &Path) {
    assert!(repo.is_absolute(), "absolute Source repo");
    assert!(fixture.is_absolute(), "absolute fixture");
    assert!(root.is_absolute(), "absolute child root");
    assert_eq!(
        sk_kernel::python_os_encoding::OsEncodingContext::from_environment().label(),
        "utf-8, surrogateescape"
    );
    assert_eq!(
        std::env::current_dir().expect("child cwd"),
        root,
        "child must run in its temporary root"
    );
    for (key, name) in env_roots() {
        let expected = if name == "root" {
            root.to_path_buf()
        } else {
            root.join(name)
        };
        assert_eq!(
            PathBuf::from(std::env::var_os(key).unwrap_or_default()),
            expected,
            "{key}"
        );
    }
    assert_eq!(
        PathBuf::from(std::env::var_os("PYTHONPATH").expect("absolute PYTHONPATH")),
        repo.join("src")
    );
    assert_eq!(
        PathBuf::from(std::env::var_os("SPEC_KITTY_PACKS_ROOT").expect("packs root")),
        repo.join("packs")
    );
    assert_eq!(
        std::env::var_os("PYTHONHASHSEED").as_deref(),
        Some(std::ffi::OsStr::new("0"))
    );
}

fn child_command(root: &Path, repo: &Path, fixture: &Path, mut command: Command) -> Command {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8")
        .env("LANG", "C.UTF-8")
        .env(REPO_ENV, repo)
        .env(ROOT_ENV, root)
        .env(FIXTURE_ENV, fixture);
    for (key, name) in env_roots() {
        let value = if name == "root" {
            root.to_path_buf()
        } else {
            root.join(name)
        };
        command.env(key, value);
    }
    command
}

/// The Native child re-invocation. Inert unless the parent sets the mode.
#[test]
fn cf76_native_uppercase_child() {
    let Ok(mode) = std::env::var(MODE_ENV) else {
        return;
    };
    assert_eq!(mode, "native");
    let root = PathBuf::from(std::env::var_os(ROOT_ENV).expect("root"));
    let repo = PathBuf::from(std::env::var_os(REPO_ENV).expect("repo"));
    let fixture = PathBuf::from(std::env::var_os(FIXTURE_ENV).expect("fixture"));
    assert_child_environment(&root, &repo, &fixture);
    println!("CF76_JSON={}", native_pair(&fixture));
}

/// The Source child script, written into the temporary workspace.
const SOURCE_CHILD: &str = r#"
import json, os, sys, unicodedata
from pathlib import Path

root = Path(os.environ["CF76_PAIR_ROOT"])
repo = Path(os.environ["CF76_PAIR_REPO"])
fixture = Path(os.environ["CF76_PAIR_FIXTURE"])
for name in ("HOME","XDG_CONFIG_HOME","XDG_CONFIG_DIRS","XDG_CACHE_HOME",
             "XDG_DATA_HOME","XDG_DATA_DIRS","XDG_STATE_HOME","XDG_RUNTIME_DIR",
             "TMPDIR","TMP","TEMP"):
    value = os.environ.get(name)
    assert value and Path(value).is_absolute(), name
assert Path.cwd() == root, (Path.cwd(), root)
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"
assert os.environ["PYTHONHASHSEED"] == "0"
assert sys.version.split()[0] == "3.11.15", sys.version
assert unicodedata.unidata_version == "14.0.0", unicodedata.unidata_version
assert sys.getfilesystemencoding() == "utf-8", sys.getfilesystemencoding()
assert sys.getfilesystemencodeerrors() == "surrogateescape"
assert os.path.realpath(sys.prefix).endswith(".venv-rust-oracle")
assert (repo / "packs" / "built-in" / "directives").is_dir()

from charter.offering.drg.migration import id_normalizer
from charter.activation.identity_index import IdentityIndex
from charter.offering.artifact_kinds import ArtifactKind

module_file = Path(id_normalizer.__file__).resolve()
assert str(module_file).startswith(str(repo / "src")), module_file
index_file = Path(sys.modules[IdentityIndex.__module__].__file__).resolve()
assert str(index_file).startswith(str(repo / "src")), index_file

UPPER = {
    "sharp_s": "\u00df",
    "ligature_fi": "\ufb01",
    "greek_dasia": "\u1f80",
    "greek_iota": "\u1fbe",
    "j_caron": "\u01f0",
    "rams_horn": "\u0264",
    "l_bar": "\u019b",
    "nul": "\x00a",
    "noncharacter": "x\u0378y",
    "astral": "\U0001f600a",
    "surrogate_mixed": "a\ud800\u00df-b",
    "surrogate_pair": "\ud83d\ude00-a",
}
NORMALIZE = {
    "sharp": "\u00df",
    "sharp_hyphen_ligature": "\u00df-\ufb01",
    "greek_slug": "\u03c0-s",
    "nd_prefix": "\u0660\u0662\u0664-s",
    "nd_single": "\u0663-x",
    "canonical_newline": "DIRECTIVE_024\n",
    "canonical_two_newlines": "DIRECTIVE_024\n\n",
    "surrogate_mixed": "a\ud800b-c",
    "rams": "\u0264",
    "hub": "use-c4-model-techniques",
    "leading_space": " 024",
    "nul": "\x00abc",
}

def hx(text):
    return "-".join(f"{ord(character):04x}" for character in text)

built_in = fixture / "packs" / "built-in"
org = fixture / "org-pack"
project = fixture / ".kittify" / "charter-packs"
index = IdentityIndex(org_roots=[org], project_root=project, offering_root=built_in)
K = ArtifactKind.DIRECTIVE

def lookup(token, selection=False):
    return hx(index.lookup_key(K, token, selection=selection))

def declared(token):
    value = index.declared_id(K, token)
    return hx(value) if value is not None else ""

def stem_for(declared_id):
    value = index.stem_for_declared_id(K, declared_id)
    return hx(value) if value is not None else ""

def stem(token):
    value = index.stem(K, token)
    return hx(value) if value is not None else ""

keys = {
    "alpha_beta_plain": lookup("alpha-beta"),
    "alpha_beta_selection": lookup("alpha-beta", selection=True),
    "normalized_number": lookup("777"),
    "upper_declared": lookup("\u00df"),
    "hub_stem": lookup("use-c4-model-techniques"),
    "nd_unknown": lookup("\u0660\u0662\u0664"),
    "rams_unknown": lookup("\u0264-slug"),
    "cs_unknown": hx(IdentityIndex.unindexed_key(K, "a\ud800b-c")),
    "declared_number": declared("777"),
    "declared_hub": declared("use-c4-model-techniques"),
    "declared_unknown": declared("\u0264"),
    "has_declared_ss": index.has_declared_id(K, "ss"),
    "stem_for_ss": stem_for("SS"),
    "stem_unknown": stem("\u0264"),
}

print("CF76_JSON=" + json.dumps({
    "upper": {label: hx(value.upper()) for label, value in UPPER.items()},
    "normalize": {label: hx(id_normalizer.normalize_directive_id(value)) for label, value in NORMALIZE.items()},
    "keys": keys,
}))
"#;

fn run_child(mut command: Command, label: &str) -> Value {
    let output = command.output().expect("child runs");
    assert!(
        output.status.success(),
        "{label} child failed: status {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    eprintln!(
        "{label} child ok: exit={:?} stdout_bytes={} stderr_bytes={}",
        output.status.code(),
        output.stdout.len(),
        output.stderr.len()
    );
    let line = stdout
        .lines()
        .find(|line| line.starts_with("CF76_JSON="))
        .unwrap_or_else(|| panic!("{label} child printed no CF76_JSON line:\n{stdout}"));
    serde_json::from_str(&line["CF76_JSON=".len()..]).expect("child JSON")
}

/// Two independent business children: the pinned Source and the Native pair.
#[test]
fn cf76_uppercase_and_normalizer_match_source() {
    let workspace = tempfile::tempdir().expect("workspace");
    let repo = repo();

    let source_root = workspace.path().join("source-root");
    let native_root = workspace.path().join("native-root");
    for root in [&source_root, &native_root] {
        for (_, name) in env_roots() {
            fs::create_dir_all(root.join(name)).expect("root subdir");
        }
    }
    let source_fixture = write_fixture(&source_root);
    let native_fixture = write_fixture(&native_root);
    let script = workspace.path().join("cf76_source_child.py");
    fs::write(&script, SOURCE_CHILD).expect("source child script");

    let mut source_command = Command::new(oracle::python());
    source_command = child_command(&source_root, &repo, &source_fixture, source_command);
    source_command.arg(&script);
    let source = run_child(source_command, "source");

    let mut native_command = child_command(
        &native_root,
        &repo,
        &native_fixture,
        Command::new(std::env::current_exe().expect("test binary")),
    );
    native_command
        .env(MODE_ENV, "native")
        .arg(std::env::current_exe().expect("test binary"))
        .args(["--exact", "cf76_native_uppercase_child", "--nocapture"]);
    let native = run_child(native_command, "native");

    // Non-vacuous: both children actually produced every vector and key.
    for (field, expected) in [("upper", 12), ("normalize", 12), ("keys", 14)] {
        assert_eq!(
            source[field].as_object().expect("source map").len(),
            expected,
            "source {field} rows"
        );
        assert_eq!(
            native[field].as_object().expect("native map").len(),
            expected,
            "native {field} rows"
        );
    }

    assert_eq!(
        source["upper"], native["upper"],
        "the pinned str.upper() table must match the interpreter exactly"
    );
    assert_eq!(
        source["normalize"], native["normalize"],
        "the Source normalize_directive_id policy must be reproduced exactly"
    );
    assert_eq!(
        source["keys"], native["keys"],
        "identity keys (stem/declared/unindexed precedence) must match"
    );

    // The Source's own shape, asserted on the shared values.
    assert_eq!(source["upper"]["sharp_s"], "0053-0053");
    assert_eq!(source["upper"]["rams_horn"], "0264");
    assert_eq!(
        source["upper"]["surrogate_mixed"],
        "0041-d800-0053-0053-002d-0042"
    );
    assert_eq!(
        source["normalize"]["nd_prefix"],
        "0044-0049-0052-0045-0043-0054-0049-0056-0045-005f-0660-0662-0664"
    );
    assert_eq!(
        source["normalize"]["surrogate_mixed"],
        "0041-d800-0042-005f-0043"
    );
    assert_eq!(
        source["keys"]["alpha_beta_plain"],
        "0044-0049-0052-0045-0043-0054-0049-0056-0045-005f-0038-0038-0039"
    );
    assert_eq!(
        source["keys"]["alpha_beta_selection"],
        "0061-006c-0070-0068-0061-002d-0062-0065-0074-0061"
    );
    assert_eq!(source["keys"]["has_declared_ss"], true);
    assert_eq!(source["keys"]["stem_for_ss"], "0073-0068-0061-0072-0070");
    assert_eq!(source["keys"]["declared_unknown"], "");
}
