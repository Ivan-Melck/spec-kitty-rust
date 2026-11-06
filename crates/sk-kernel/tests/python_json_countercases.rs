//! Source-measured counter-cases for the driver/manifest consumer class.
//!
//! Two *different* Source facts exist and are not conflated here:
//!
//! * The **status event model** (`specify_cli.status.models.StatusEvent`) is the
//!   H4 status-event-log owner's model. It **accepts** a lone surrogate `actor`
//!   and surrogate units inside `policy_metadata` keys and values, and its own
//!   `json.dumps` writer produces an `ensure_ascii=True` body that round-trips
//!   them (measured: `/tmp/sk-j1-evidence/j1-source-models3.87LUOX.log`).
//! * The **typed Op record model** (`specify_cli.invocation.record.OpStartedEvent`,
//!   pydantic v2) **refuses** a lone surrogate actor with a `string_unicode`
//!   `ValidationError`; `parse_op_event` reports it as `LegacyRecordError`. The
//!   Op *legacy raw-dict* reader (`executor._read_started_event`, mirrored by
//!   `legacy_json::parse_object`) still reads the same line, because it only
//!   applies `json.loads` and field access.
//!
//! So this file does **not** claim a Source-valid Op claim-model refusal: the
//! typed model's refusal is Source behavior, not a Native gap. It does show that
//! the canonical codec reproduces the status model's body exactly and that the
//! current `serde_json` consumer projection refuses it (H4 owns that consumer
//! edit). A profile/manifest-shaped document is kept as a codec-level case only;
//! the A5 Source-model branch is A5's pending probe, not waived here.

#[path = "support/oracle.rs"]
mod oracle_support;

use std::process::Command;

use sk_kernel::python_json::{PythonJsonValue, parse};

const HEADER: &str = concat!(
    "import json, os, sys\n",
    "from pathlib import Path\n",
    "assert sys.version.split()[0] == '3.11.15', sys.version\n",
    "root = Path(os.environ['SK_J1_ROOT'])\n",
    "assert Path.cwd() == root, Path.cwd()\n",
    "for key in ('HOME','TMPDIR','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_DATA_HOME','XDG_STATE_HOME'):\n",
    "    assert Path(os.environ[key]).parent == root, (key, os.environ[key])\n",
    "assert os.environ.get('PYTHONDONTWRITEBYTECODE') == '1'\n",
);

fn run_source(script: &str, args: &[&str]) -> serde_json::Value {
    let temp = tempfile::tempdir().expect("isolated Source roots");
    let root = temp.path().canonicalize().expect("canonical root");
    for name in ["home", "config", "cache", "data", "state", "tmp"] {
        std::fs::create_dir(root.join(name)).expect("isolated directory");
    }
    let output = Command::new(oracle_support::python())
        .arg("-c")
        .arg(format!("{HEADER}{script}"))
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", root.join("home"))
        .env("TMPDIR", root.join("tmp"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("SK_J1_ROOT", &root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONPATH", repository_root().join("src"))
        .current_dir(&root)
        .output()
        .expect("pinned Source oracle");
    assert!(
        output.status.success(),
        "Source must run and assert first: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Source JSON report")
}

fn repository_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

fn units(value: &serde_json::Value) -> Vec<u32> {
    value
        .as_array()
        .expect("code point array")
        .iter()
        .map(|unit| unit.as_u64().expect("code point") as u32)
        .collect()
}

#[test]
fn status_event_model_accepts_surrogate_actor_and_policy_metadata_keys() {
    let source = run_source(
        concat!(
            "from specify_cli.status.models import StatusEvent\n",
            "surrogate = '\\udc80'\n",
            "row = {\n",
            "    'event_id': '01KTK5JBD69FQ8XVRFV1J630MJ',\n",
            "    'mission_slug': '034-feature',\n",
            "    'wp_id': 'WP01',\n",
            "    'from_lane': 'planned',\n",
            "    'to_lane': 'claimed',\n",
            "    'at': '2026-06-10T20:00:00+00:00',\n",
            "    'actor': surrogate,\n",
            "    'force': False,\n",
            "    'execution_mode': 'worktree',\n",
            "    'policy_metadata': {surrogate: surrogate},\n",
            "}\n",
            "event = StatusEvent.from_dict(row)\n",
            "assert event.actor == surrogate\n",
            "assert list(event.policy_metadata) == [surrogate]\n",
            "assert event.policy_metadata[surrogate] == surrogate\n",
            "body = json.dumps(event.to_dict())\n",
            "assert body.isascii()\n",
            "reloaded = json.loads(body)\n",
            "assert [ord(c) for c in reloaded['actor']] == [0xDC80]\n",
            "assert [ord(c) for c in list(reloaded['policy_metadata'])[0]] == [0xDC80]\n",
            "assert [ord(c) for c in reloaded['policy_metadata'][list(reloaded['policy_metadata'])[0]]] == [0xDC80]\n",
            "print(json.dumps({'body': [ord(c) for c in body]}))\n",
        ),
        &[],
    );
    let body: Vec<u32> = units(&source["body"]);
    let document = body
        .iter()
        .map(|&unit| char::from_u32(unit).expect("ascii body"))
        .collect::<String>();

    // The canonical codec reads the status model's own body and re-renders it
    // byte-for-byte, with the actor/key units intact.
    let parsed = parse(&document).expect("canonical reader accepts the status body");
    assert_eq!(parsed.render_python(None, true).codepoints(), body);
    assert_eq!(
        parsed.field("actor"),
        Some(&PythonJsonValue::String(
            sk_kernel::PythonString::from_codepoints(&[0xdc80]).expect("surrogate unit")
        ))
    );
    let Some(PythonJsonValue::ExactObject(metadata)) = parsed.field("policy_metadata") else {
        panic!("policy_metadata object");
    };
    assert_eq!(metadata.keys().next().expect("key").codepoints(), &[0xdc80]);
    assert_eq!(
        metadata.get_codepoints(&[0xdc80]),
        Some(&PythonJsonValue::String(
            sk_kernel::PythonString::from_codepoints(&[0xdc80]).expect("surrogate unit")
        ))
    );

    // The current serde consumer projection refuses this Source-valid status
    // line. H4 owns the status/driver consumer edit; this codec is its API.
    assert!(
        serde_json::from_str::<serde_json::Value>(&document).is_err(),
        "serde_json cannot represent the surrogate escapes the status model writes"
    );
}

#[test]
fn op_typed_model_refusal_is_source_behavior_not_a_codec_gap() {
    let source = run_source(
        concat!(
            "from specify_cli.invocation.record import LegacyRecordError, OpStartedEvent, parse_op_event\n",
            "surrogate = '\\udc80'\n",
            "fields = {\n",
            "    'event': 'started',\n",
            "    'invocation_id': '01KTK5JBD69FQ8XVRFV1J630MJ',\n",
            "    'profile_id': 'architect',\n",
            "    'action': 'implement',\n",
            "    'request_text': '',\n",
            "    'actor': surrogate,\n",
            "    'mode_of_work': 'task_execution',\n",
            "    'governance_context_hash': '',\n",
            "    'governance_context_available': False,\n",
            "    'started_at': '2026-06-10T20:00:00+00:00',\n",
            "}\n",
            "try:\n",
            "    OpStartedEvent(**fields)\n",
            "except Exception as error:\n",
            "    model_error = type(error).__name__\n",
            "else:\n",
            "    raise AssertionError('the measured Op model accepted a surrogate actor')\n",
            "raw = dict(fields)\n",
            "raw['metadata'] = {surrogate: surrogate}\n",
            "wire = json.dumps(raw)\n",
            "assert wire.isascii()\n",
            "back = json.loads(wire)\n",
            "assert [ord(c) for c in back['actor']] == [0xDC80]\n",
            "assert [ord(c) for c in list(back['metadata'])[0]] == [0xDC80]\n",
            "try:\n",
            "    parse_op_event(back)\n",
            "except LegacyRecordError as error:\n",
            "    typed = type(error).__name__\n",
            "else:\n",
            "    raise AssertionError('parse_op_event accepted the surrogate actor')\n",
            "print(json.dumps({'model_error': model_error, 'typed': typed, 'wire': [ord(c) for c in wire]}))\n",
        ),
        &[],
    );
    assert_eq!(
        source["model_error"].as_str().expect("model error"),
        "ValidationError"
    );
    assert_eq!(
        source["typed"].as_str().expect("typed error"),
        "LegacyRecordError"
    );
    let wire: Vec<u32> = units(&source["wire"]);
    let document = wire
        .iter()
        .map(|&unit| char::from_u32(unit).expect("ascii wire"))
        .collect::<String>();
    // The legacy raw-dict Op reader path (and the canonical codec) reads it.
    let parsed = parse(&document).expect("canonical codec reads the raw Op line");
    assert_eq!(parsed.render_python(None, true).codepoints(), wire);
    assert_eq!(
        parsed.field("metadata").and_then(|value| {
            match value {
                PythonJsonValue::ExactObject(object) => object.get_codepoints(&[0xdc80]),
                _ => None,
            }
        }),
        Some(&PythonJsonValue::String(
            sk_kernel::PythonString::from_codepoints(&[0xdc80]).expect("surrogate unit")
        ))
    );
}

#[test]
fn profile_manifest_shaped_surrogates_are_a_codec_level_case() {
    // Codec-level only: no claim that the A5 profile/manifest Source branch
    // accepts this document (A5's probe is pending). The canonical codec keeps
    // the units; the serde consumer projection does not.
    let document = r#"{"profile_id": "architect", "roles": {"\ud800": {"role": "reviewer"}}, "metadata": {"note": "\ud800\udc00"}}"#;
    let parsed = parse(document).expect("canonical reader accepts the manifest shape");
    assert_eq!(
        parsed
            .render_python(None, true)
            .to_scalar_string()
            .expect("ascii body"),
        document
    );
    let Some(PythonJsonValue::ExactObject(roles)) = parsed.field("roles") else {
        panic!("roles object");
    };
    assert_eq!(
        roles.keys().next().expect("role key").codepoints(),
        &[0xd800]
    );
    assert_eq!(
        parsed
            .field("metadata")
            .and_then(|metadata| metadata.field("note")),
        Some(&PythonJsonValue::String(
            sk_kernel::PythonString::from_codepoints(&[0x10000]).expect("paired escape")
        ))
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(document).is_err(),
        "serde_json refuses surrogate keys/values; A5 owns the manifest consumer edit"
    );
}
