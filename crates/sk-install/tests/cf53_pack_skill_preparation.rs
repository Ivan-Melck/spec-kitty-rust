//! CF53 — pack-skill preparation, hashing, rendering and the physical write
//! phase (installer side), plus the paired Source/Native proof.
//!
//! Source: `src/charter/activation/skill_preparation.py`,
//! `src/specify_cli/skills/{catalog,pack_skill_renderer}.py`.
//!
//! The reference serves a pack skill whose **unconstrained** string fields carry
//! a lone surrogate (`version`, `parameters[].default`, `parameters[].description`,
//! `triggers`, `tools`, `maintainers`, `invocation.side_effects`,
//! `expands_to.args`): `_source_hash` succeeds (`json.dumps` escapes it), the
//! renderer composes the text, and `Path.write_text(..., encoding="utf-8")` raises
//! `UnicodeEncodeError`. A **constrained** field (`title`, `description`,
//! `parameters[].name`, `expands_to.target`) is refused by pydantic at load, so
//! the record is skipped before preparation ever sees it.
//!
//! The paired test runs BOTH actual isolated children: the pinned CPython 3.11.15
//! oracle and this test binary re-executed. Each asserts CWD, HOME, every XDG root
//! (DIRS included), TMPDIR/TMP/TEMP, absolute `PYTHONPATH`, the pinned pack root
//! and the hash seed before computing, and both report exact code points.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::path::{Path, PathBuf};
use std::process::Command;

use sk_charter::PackTier;
use sk_install::skills::PreparedSkill;
use sk_install::skills::pack_skill::{
    PACK_SKILL_STAGING_REL, PackSkillRepository, render_pack_skill, resolve_project_skill_catalog,
    source_hash,
};
use sk_kernel::PythonString;

const EXTRA_XDG_ROOTS: [(&str, &str); 2] = [
    ("XDG_CONFIG_DIRS", "config-dirs"),
    ("XDG_DATA_DIRS", "data-dirs"),
];

const SURROGATE_SKILL: &str = "schema_version: '1.0'\nid: exact-demo\ntitle: Exact demo\ndescription: Ships   the  release.\nform: prompt\nbody_path: exact-demo.md\ntriggers: [go]\nparameters:\n  - name: scope\n    description: Which issues\n    default: \"14d\\ud800\"\nversion: \"1.0.\\ud800\"\nmaintainers: [\"m\\ud800\"]\n";

const PLAIN_SKILL: &str = "schema_version: '1.0'\nid: plain-demo\ntitle: \"T\\u00e9\"\ndescription: \"Plain\\u00a0description\"\nform: prompt\nbody_path: plain-demo.md\ntriggers: [go]\n";

const SKIPPED_SKILL: &str = "schema_version: '1.0'\nid: skipped-demo\ntitle: \"T\\ud800\"\ndescription: D\nform: prompt\nbody_path: skipped-demo.md\n";

const WRAPPER_SKILL: &str = "schema_version: '1.0'\nid: wrapper-demo\ntitle: Wrapper demo\ndescription: Runs the status command.\nform: wrapper\nexpands_to:\n  target: \"cli:spec-kitty status\"\n  args: \"--json\\ud800\"\n";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
    std::fs::write(path, text).expect("write");
}

/// A project whose org pack carries the exact-surrogate records.
fn project(root: &Path) {
    write(
        &root.join("packs/built-in/pack.yaml"),
        "name: built-in\npack_level: built-in\n",
    );
    let skills = root.join("packs/acme/skills");
    write(
        &root.join("packs/acme/org-charter.yaml"),
        "schema_version: 1\norg_name: acme\nskill_namespace: acme\n",
    );
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    write(&skills.join("exact-demo.skill.yaml"), SURROGATE_SKILL);
    write(&skills.join("exact-demo.md"), "Do the exact thing.\n");
    write(&skills.join("plain-demo.skill.yaml"), PLAIN_SKILL);
    write(&skills.join("plain-demo.md"), "Do the plain thing.\n");
    write(&skills.join("skipped-demo.skill.yaml"), SKIPPED_SKILL);
    write(&skills.join("skipped-demo.md"), "Never loads.\n");
    write(&skills.join("wrapper-demo.skill.yaml"), WRAPPER_SKILL);
}

/// The first unencodable position in an encoder error's code units.
fn error_start(error: &sk_kernel::python_utf8::PythonUtf8EncodeError) -> usize {
    error
        .object()
        .iter()
        .position(|code| (0xD800..=0xDFFF).contains(code))
        .expect("a surrogate unit")
}

fn prepared(repository: &PackSkillRepository, id: &str) -> PreparedSkill {
    let skill = repository.skill(id).expect("served").clone();
    let name = format!("acme-{id}");
    let body = repository.body_text(id);
    let requires: Vec<String> = Vec::new();
    PreparedSkill {
        id: id.to_owned(),
        tier: PackTier::Org,
        rendered_name: name.clone(),
        form: skill.form,
        body: body.clone(),
        expansion: skill.expands_to.clone(),
        requires: requires.clone(),
        source_path: repository.source_path(id).expect("source").to_path_buf(),
        source_hash: source_hash(&skill, body.as_deref(), &requires, &name),
        skill,
    }
}

#[test]
fn preparation_renders_exact_text_and_the_write_phase_refuses_the_unencodable_one() {
    let root = std::env::temp_dir().join(format!("sk-cf53-install-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean");
    }
    project(&root);
    let repository = PackSkillRepository::load(
        &root.join("packs/built-in/skills"),
        &[root.join("packs/acme/skills")],
        None,
        Some("acme"),
    )
    .expect("loads");

    // The constrained-field record never reaches preparation.
    assert!(repository.skill("skipped-demo").is_none());

    let exact = prepared(&repository, "exact-demo");
    let text = render_pack_skill(&exact).expect("renders");
    assert!(
        text.codepoints()
            .iter()
            .any(|code| (0xD800..=0xDFFF).contains(code)),
        "the rendered text carries the exact surrogate"
    );
    // The surrogate is in the default and the version, so the physical encode is
    // the first thing that cannot carry it.
    let error = sk_kernel::python_utf8::encode_python_string(&text).expect_err("surrogate");
    // CPython's own encoder message, produced by the canonical encoder.
    let position = error_start(&error);
    assert_eq!(
        error.to_string(),
        format!(
            "'utf-8' codec can't encode character '\\ud800' in position {position}: surrogates not allowed"
        )
    );

    let plain = prepared(&repository, "plain-demo");
    let plain_text = render_pack_skill(&plain).expect("renders");
    let bytes = sk_kernel::python_utf8::encode_python_string(&plain_text).expect("encodable");
    // `ensure_ascii=False` in the frontmatter: authored code points stay literal,
    // exactly like the reference's `json.dumps(..., ensure_ascii=False)`.
    let text = String::from_utf8(bytes).expect("utf-8");
    assert!(text.contains("name: \"acme-plain-demo\""), "{text}");
    // The rendered description collapses Python `str.split()` whitespace (the
    // authored NBSP included), and `ensure_ascii=False` keeps authored code points
    // literal rather than escaping them.
    assert!(
        text.contains("description: \"Plain description Triggers: go.\""),
        "{text}"
    );
    assert!(text.contains("# T\u{e9}"), "{text}");
    assert!(!text.contains("\\u00e9"), "{text}");

    // The wrapper's `args` surrogate is rendered into the instruction block.
    let wrapper = prepared(&repository, "wrapper-demo");
    let wrapper_text = render_pack_skill(&wrapper).expect("renders");
    assert!(
        wrapper_text.codepoints().contains(&0xD800),
        "the wrapper instruction carries the exact args"
    );
    assert!(
        wrapper_text.to_scalar_string().is_err(),
        "the exact text is not scalar"
    );
}

#[test]
fn the_absent_key_default_reads_the_coercing_org_required_ids() {
    let root = std::env::temp_dir().join(format!("sk-cf53-consumer-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean");
    }
    project(&root);
    // `required_skills` with mixed members: the *coercing* reader (the reference's
    // `_absent_key_default`) answers `1`, `plain-demo`, `True`, `None`; the strict
    // per-field floor read answers `[]` and would retire every installed skill.
    write(
        &root.join("packs/acme/org-charter.yaml"),
        "schema_version: 1\norg_name: acme\nskill_namespace: acme\nrequired_skills: [1, '  plain-demo  ', true, null]\n",
    );
    // A coercing member that names no skill proves the consumer read the set
    // through `read_org_required_ids`: the reference refuses an in-force id that
    // does not load, and names it. A strict per-field floor read would have
    // required nothing and served nothing here.
    let error = resolve_project_skill_catalog(&root, false).expect_err("coercing id is in force");
    let message = error.to_string();
    assert!(
        message.contains("activated skill \"1\" is not available"),
        "{message}"
    );
    assert_eq!(
        sk_doctrine::read_org_required_tokens(&root, sk_charter::ArtifactKind::Skill, false)
            .expect("floor"),
        Vec::<String>::new()
    );

    // With a list the strict reader accepts, both readers agree and the skill is
    // served — this is the ordinary path the fix must not disturb.
    write(
        &root.join("packs/acme/org-charter.yaml"),
        "schema_version: 1\norg_name: acme\nskill_namespace: acme\nrequired_skills: ['  plain-demo  ']\n",
    );
    let catalog = resolve_project_skill_catalog(&root, false).expect("resolves");
    // The shipped built-in skills are merged beside the pack skill.
    assert!(
        catalog.names().contains(&"acme-plain-demo".to_owned()),
        "{:?}",
        catalog.names()
    );
}

#[test]
fn resolution_writes_nothing_when_the_render_cannot_be_encoded() {
    let root = std::env::temp_dir().join(format!("sk-cf53-stage-{}", std::process::id()));
    if root.exists() {
        std::fs::remove_dir_all(&root).expect("clean");
    }
    project(&root);
    write(
        &root.join(".kittify/config.yaml"),
        "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: packs/acme\n",
    );
    // The activation ledger names the exact-surrogate skill.
    write(
        &root.join(".kittify/charter/activation.yaml"),
        "activated_skills:\n  - exact-demo\n",
    );
    let error = resolve_project_skill_catalog(&root, true).expect_err("unencodable render");
    let message = error.to_string();
    assert!(
        message.contains("surrogates not allowed"),
        "the failure is the physical write: {message}"
    );
    assert_eq!(error.code(), "io_failed");
    // The late phase: no staged file and no staged directory for the skill.
    let staged = root.join(PACK_SKILL_STAGING_REL).join("acme-exact-demo");
    assert!(
        !staged.exists(),
        "nothing may be staged for a failed encode"
    );
}

// ---------------------------------------------------------------------------
// Paired Source/Native proof (both actual isolated children)
// ---------------------------------------------------------------------------

fn child_command(root: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .current_dir(root)
        .env("SK_CF53_ROOT", root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONHASHSEED", "0")
        .env("PYTHONPATH", repository.join("src"))
        .env("SK_CF53_SOURCE_ROOT", &repository)
        .env("SPEC_KITTY_PACKS_ROOT", repository.join("packs"));
    for (key, name) in isolated_roots().into_iter().chain(EXTRA_XDG_ROOTS) {
        command.env(key, root.join(name));
    }
    command
}

/// The isolated roots every child asserts. `(env var, directory name)`.
fn isolated_roots() -> [(&'static str, &'static str); 9] {
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

fn fixture(name: &str) -> PathBuf {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/cf53-pack-skill");
    std::fs::create_dir_all(&base).expect("fixture base");
    let root = tempfile::Builder::new()
        .prefix(name)
        .tempdir_in(&base)
        .expect("fresh exclusive fixture")
        .keep()
        .canonicalize()
        .expect("canonical fixture path");
    for (_, directory) in isolated_roots().into_iter().chain(EXTRA_XDG_ROOTS) {
        std::fs::create_dir_all(root.join(directory)).expect("isolated root");
    }
    project(&root);
    root
}

const SOURCE_SCRIPT: &str = r#"
import json, os, pathlib, sys, warnings
root = pathlib.Path(os.environ["SK_CF53_ROOT"])
assert pathlib.Path.cwd() == root, (str(pathlib.Path.cwd()), str(root))
assert sys.version_info[:3] == (3, 11, 15), sys.version_info
assert os.environ["PYTHONHASHSEED"] == "0"
assert pathlib.Path(os.environ["PYTHONPATH"]).is_absolute()
repo = pathlib.Path(os.environ["SK_CF53_SOURCE_ROOT"])
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
from charter.activation.skill_preparation import PreparedSkill, _source_hash
from charter.offering.pack_skills.repository import PackSkillRepository
from charter.offering.pack_skills.validation import rendered_description, skill_limit_violation
from specify_cli.skills.pack_skill_renderer import render_pack_skill
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter("always")
    repository = PackSkillRepository(
        built_in_dir=root / "packs/built-in/skills",
        org_dirs=[root / "packs/acme/skills"],
        project_dir=None,
        skill_namespace="acme",
    )
records = {}
for skill_id in sorted(item.id for item in repository.list_all()):
    skill = repository.get(skill_id)
    name = "acme-" + skill_id
    body = repository.body_text(skill_id) or ""
    digest = _source_hash(skill, body, (), name)
    prepared = PreparedSkill(
        id=skill_id, tier="org", rendered_name=name, form=skill.form, body=body,
        expansion=skill.expands_to, requires=(), source_path=repository.source_path(skill_id),
        source_hash=digest, skill=skill,
    )
    text = render_pack_skill(prepared)
    target = root / "out" / name / "SKILL.md"
    target.parent.mkdir(parents=True, exist_ok=True)
    try:
        target.write_text(text, encoding="utf-8")
        write = {"outcome": "written", "bytes": target.stat().st_size}
    except Exception as exc:  # noqa: BLE001 - the physical write is the phase under test
        write = {"outcome": "raised", "type": type(exc).__name__, "message": str(exc)}
    records[skill_id] = {
        "form": skill.form,
        "version": [ord(char) for char in skill.version],
        "default": [ord(char) for char in (skill.parameters[0].default or "")] if skill.parameters else [],
        "maintainers": [[ord(char) for char in item] for item in skill.maintainers],
        "args": [ord(char) for char in (skill.expands_to.args if skill.expands_to else "")],
        "rendered_description": [ord(char) for char in rendered_description(skill)],
        "limit": skill_limit_violation(skill, name),
        "source_hash": digest,
        "render": [ord(char) for char in text],
        "write": write,
    }
payload = {
    "runner": "source-python-" + ".".join(str(part) for part in sys.version_info[:3]),
    "cwd": str(pathlib.Path.cwd()),
    "hashseed": os.environ.get("PYTHONHASHSEED"),
    "ids": sorted(item.id for item in repository.list_all()),
    "warnings": [str(item.message) for item in caught if issubclass(item.category, UserWarning)],
    "records": records,
}
print("CF53_RECEIPT " + json.dumps(payload, sort_keys=True))
"#;

#[test]
fn cf53_installer_native_child_and_receipt() {
    let Some(root) = std::env::var_os("SK_CF53_ROOT").map(PathBuf::from) else {
        return;
    };
    assert_eq!(std::env::current_dir().expect("cwd"), root);
    for (key, name) in isolated_roots().into_iter().chain(EXTRA_XDG_ROOTS) {
        assert_eq!(
            std::env::var_os(key).map(PathBuf::from),
            Some(root.join(name)),
            "{key} is the isolated fixture root"
        );
    }
    assert_eq!(std::env::var("PYTHONHASHSEED").as_deref(), Ok("0"));
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(source_root.is_absolute());
    assert_eq!(
        std::env::var_os("PYTHONPATH").map(PathBuf::from),
        Some(source_root.join("src"))
    );
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").map(PathBuf::from),
        Some(source_root.join("packs"))
    );
    let repository = PackSkillRepository::load(
        &root.join("packs/built-in/skills"),
        &[root.join("packs/acme/skills")],
        None,
        Some("acme"),
    )
    .expect("loads");
    let mut records = serde_json::Map::new();
    for id in repository.ids() {
        let item = prepared(&repository, &id);
        let text = render_pack_skill(&item).expect("renders");
        let write = match sk_kernel::python_utf8::encode_python_string(&text) {
            Ok(bytes) => {
                let target = root.join("out").join(&item.rendered_name).join("SKILL.md");
                std::fs::create_dir_all(target.parent().expect("parent")).expect("dir");
                std::fs::write(&target, &bytes).expect("write");
                serde_json::json!({"outcome": "written", "bytes": bytes.len()})
            }
            Err(error) => serde_json::json!({
                "outcome": "raised",
                "type": "UnicodeEncodeError",
                "message": error.to_string(),
            }),
        };
        let exact_name = PythonString::from_scalars(&item.rendered_name);
        records.insert(
            id.clone(),
            serde_json::json!({
                "form": item.skill.form.as_str(),
                "version": item.skill.version.codepoints(),
                "default": item.skill.parameters.first().and_then(|parameter| parameter.default.as_ref()).map(|value| value.codepoints()).unwrap_or(&[]),
                "maintainers": item.skill.maintainers.iter().map(|value| value.codepoints()).collect::<Vec<_>>(),
                "args": item.skill.expands_to.as_ref().map(|expansion| expansion.args.codepoints()).unwrap_or(&[]),
                "rendered_description": sk_doctrine::rendered_description(&item.skill).codepoints(),
                "limit": sk_doctrine::skill_limit_violation(&item.skill, &exact_name),
                "source_hash": item.source_hash,
                "render": text.codepoints(),
                "write": write,
            }),
        );
    }
    println!(
        "CF53_RECEIPT {}",
        serde_json::json!({
            "runner": "native",
            "cwd": std::env::current_dir().expect("cwd").display().to_string(),
            "hashseed": std::env::var("PYTHONHASHSEED").ok(),
            "ids": repository.ids(),
            "warnings": repository.warnings(),
            "records": records,
        })
    );
}

fn parse_receipt(stdout: &str) -> serde_json::Value {
    let index = stdout
        .find("CF53_RECEIPT ")
        .unwrap_or_else(|| panic!("no CF53 receipt in {stdout:?}"));
    let line = stdout[index + "CF53_RECEIPT ".len()..]
        .lines()
        .next()
        .expect("receipt line");
    serde_json::from_str(line.trim()).expect("receipt JSON")
}

#[test]
fn source_and_native_agree_on_exact_preparation_hashing_rendering_and_the_write_phase() {
    let root = fixture("cf53-pair-");
    let roots_env = root.display().to_string();
    let output = child_command(&root, oracle::python())
        .arg("-c")
        .arg(SOURCE_SCRIPT)
        .env("SK_CF53_ROOT", &roots_env)
        .output()
        .expect("run the pinned Source");
    std::fs::write(root.join("cf53-source.stdout"), &output.stdout).expect("stdout receipt");
    std::fs::write(root.join("cf53-source.stderr"), &output.stderr).expect("stderr receipt");
    assert!(
        output.status.success(),
        "Source child failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let source = parse_receipt(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(source["runner"], "source-python-3.11.15");
    assert_eq!(source["cwd"], root.display().to_string());
    assert_eq!(source["hashseed"], "0");
    // The Source really served the unconstrained-surrogate record and skipped the
    // constrained one.
    assert_eq!(
        source["ids"],
        serde_json::json!(["exact-demo", "plain-demo", "wrapper-demo"])
    );

    let executable = std::env::current_exe().expect("test binary");
    let output = child_command(&root, executable)
        .args([
            "--exact",
            "cf53_installer_native_child_and_receipt",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("SK_CF53_ROOT", &roots_env)
        .output()
        .expect("run the Native child");
    std::fs::write(root.join("cf53-native.stdout"), &output.stdout).expect("stdout receipt");
    std::fs::write(root.join("cf53-native.stderr"), &output.stderr).expect("stderr receipt");
    assert!(
        output.status.success(),
        "Native child failed: {} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let native = parse_receipt(&String::from_utf8_lossy(&output.stdout));
    assert_eq!(native["runner"], "native");
    assert_eq!(native["cwd"], root.display().to_string());
    assert_eq!(native["hashseed"], "0");

    assert_eq!(source["ids"], native["ids"], "served ids");
    for id in ["exact-demo", "plain-demo", "wrapper-demo"] {
        for key in [
            "form",
            "version",
            "default",
            "maintainers",
            "args",
            "rendered_description",
            "limit",
            "source_hash",
            "render",
            "write",
        ] {
            assert_eq!(
                source["records"][id][key], native["records"][id][key],
                "{id}: {key} differs from the Source"
            );
        }
    }
    // The unencodable records fail at the physical write with CPython's message;
    // the plain one writes the same byte count on both sides.
    assert_eq!(
        source["records"]["exact-demo"]["write"]["type"],
        serde_json::json!("UnicodeEncodeError")
    );
    assert_eq!(
        source["records"]["exact-demo"]["write"]["message"],
        native["records"]["exact-demo"]["write"]["message"]
    );
    assert_eq!(
        source["records"]["plain-demo"]["write"]["outcome"],
        serde_json::json!("written")
    );
}
