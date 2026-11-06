//! Source-first regressions for the `decisions/index.json` merge driver's
//! `DecisionIndex` model projection (H5 F4).
//!
//! Source's `run_decision_index_driver` unions the entries keyed by
//! `decision_id`, then validates the WHOLE merged document through
//! `specify_cli.decisions.models.DecisionIndex.model_validate` (pydantic v2),
//! sorts the validated entries by `(created_at.isoformat(), decision_id)` and
//! writes `json.dumps(model_dump(mode="json"), sort_keys=True, indent=2)`.
//!
//! This finite Source matrix exercises (not exhaustively certifies) the model:
//! required/defaulted/optional entry fields, `StrEnum`
//! members, `tuple[str, ...]` sequence coercion, pydantic's lax boolean words,
//! `dict[str, str]` summaries, the datetime grammar (date-only, `T`/`t`/space
//! separators, minute precision, `.`/`,` fractions, `Z`/`±HH:MM`/`±HHMM`
//! offsets, naive values, integer/float Unix timestamps in pydantic's seconds/
//! milliseconds units, and its negative-fraction handling), the top-level
//! `version`/`mission_id`/extras rules, the `step_id`-or-`slot_key` validator,
//! the JSON projection of every field (defaults included, datetimes normalised,
//! UTF-8 written with `ensure_ascii`), the `isoformat()`-based sort order, and
//! both refusal footprints: `%A` untouched, other inputs untouched, and a valid
//! retry afterwards succeeding.
//!
//! NOT claimed: pydantic's `ValidationError` TEXT. The pinned traceback prose is
//! separate error-wire work (H5 F11 family); a Native refusal is asserted to be
//! a refusal that names the document, and — where the fixture breaks one model
//! field — to mention that field, never to equal Source's prose.
//!
//! The pinned Source runs FIRST (CPython 3.11.15 through the repository oracle,
//! fail-closed) in an asserted temporary CWD/HOME/all-XDG/TMPDIR with an
//! absolute `PYTHONPATH` and pack root; it asserts the intended cause of every
//! fixture before recording it. The same test binary is then re-executed on
//! those recorded fixtures to assert the Native result.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn roots() -> [(&'static str, &'static str); 9] {
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

fn isolated(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("SK_H6_ROOT", root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}

/// The Source program: fixtures, intended-cause assertions, recorded results.
const SOURCE: &str = r#"
import hashlib, json, os, sys
from pathlib import Path

root = Path(os.environ["SK_H6_ROOT"])
assert Path.cwd() == root, (Path.cwd(), root)
assert sys.version_info[:3] == (3, 11, 15), sys.version
for key, name in [("HOME","home"),("XDG_CONFIG_HOME","config"),("XDG_CACHE_HOME","cache"),
                  ("XDG_DATA_HOME","data"),("XDG_STATE_HOME","state"),("XDG_RUNTIME_DIR","runtime"),
                  ("TMPDIR","tmp"),("TMP","tmp"),("TEMP","tmp")]:
    assert Path(os.environ[key]) == root / name, (key, os.environ[key])
repo = Path(os.environ["PYTHONPATH"]).parent
assert Path(os.environ["PYTHONPATH"]) == repo / "src"
assert Path(os.environ["SPEC_KITTY_PACKS_ROOT"]) == repo / "packs"

from specify_cli.consolidation.drivers import (
    MERGE_DRIVER_BODIES, MergeDriverError, union_decision_index,
)
from specify_cli.decisions.models import DecisionIndex

DRIVER = "merge-driver-decision-index"
ENTRY = {
    "decision_id": "D1",
    "origin_flow": "specify",
    "step_id": "step-1",
    "input_key": "k",
    "question": "q?",
    "status": "open",
    "created_at": "2026-01-02T03:04:05Z",
    "mission_id": "M1",
    "mission_slug": "demo",
}
ENTRY_FIELDS = [
    "decision_id", "origin_flow", "step_id", "slot_key", "input_key", "question",
    "options", "status", "final_answer", "rationale", "other_answer", "summary_json",
    "created_at", "resolved_at", "resolved_by", "opened_by", "mission_id", "mission_slug",
]


def document(**overrides):
    doc = {"mission_id": "M1", "entries": [dict(ENTRY)]}
    for key, value in overrides.items():
        if key == "entry":
            doc["entries"] = [dict(ENTRY) | value]
        else:
            doc[key] = value
    return doc


def entry(**changes):
    return document(entry=changes)


def without(field, **changes):
    base = dict(ENTRY) | changes
    base.pop(field, None)
    return {"mission_id": "M1", "entries": [base]}


cases = []
canonical_bytes = None
canonical_input = None


def run_case(name, ours, theirs=None, field=None, expect_top=None,
             expect_entry=None, expect_entry_keys=False, expect_order=None,
             cause=None, retry=True):
    theirs = {} if theirs is None else theirs
    d = root / f"source-{len(cases)}"
    d.mkdir()
    ours_text = ours if isinstance(ours, str) else json.dumps(ours)
    theirs_text = theirs if isinstance(theirs, str) else json.dumps(theirs)
    for side, payload in [("base", ""), ("ours", ours_text), ("theirs", theirs_text)]:
        (d / side).write_text(payload, encoding="utf-8")
    before = {side: (d / side).read_bytes() for side in ("base", "theirs")}
    original = (d / "ours").read_bytes()
    error = None
    try:
        outcome = MERGE_DRIVER_BODIES[DRIVER](str(d / "base"), str(d / "ours"), str(d / "theirs"))
    except (MergeDriverError, UnicodeDecodeError) as exc:
        outcome = None
        error = str(exc)
    result = (d / "ours").read_bytes()
    assert all((d / side).read_bytes() == value for side, value in before.items()), name
    assert outcome is None or outcome.notice is None, name
    record = {
        "name": name,
        "field": field,
        "ours": ours_text,
        "theirs": theirs_text,
        "ok": error is None,
        "error": error,
        "bytes": list(result),
        "retry_bytes": None,
    }
    retry_bytes = None
    if error is not None:
        assert cause is not None, (name, "refusal without an intended cause")
        assert cause in error, (name, cause, error)
        assert result == original, (name, "a refusal must leave %A untouched")
        if retry:
            (d / "ours").write_bytes(canonical_input)
            (d / "theirs").write_bytes(b"{}")
            merged = MERGE_DRIVER_BODIES[DRIVER](
                str(d / "base"), str(d / "ours"), str(d / "theirs")
            )
            assert merged is not None
            retry_bytes = (d / "ours").read_bytes()
            assert retry_bytes == canonical_bytes, (name, "corrected retry must succeed")
            record["retry_bytes"] = list(retry_bytes)
    else:
        assert cause is None, (name, "a success carries no intended refusal cause")
        document = json.loads(result)
        for key, value in (expect_top or {}).items():
            assert document[key] == value, (name, key, document[key], value)
        if expect_entry is not None:
            assert len(document["entries"]) == 1, name
            projected = document["entries"][0]
            if expect_entry_keys:
                assert list(projected) == sorted(ENTRY_FIELDS), (name, sorted(projected))
            for key, value in expect_entry.items():
                assert projected[key] == value, (name, key, projected[key], value)
        if expect_order is not None:
            assert [row["decision_id"] for row in document["entries"]] == expect_order, name
        assert result == (
            json.dumps(DecisionIndex.model_validate(document).model_dump(mode="json"),
                       sort_keys=True, indent=2) + "\n"
        ).encode(), (name, "the driver's bytes are the model's own projection")
    cases.append(record)


# The canonical valid document: the "corrected data" every failed case retries.
canonical = root / "canonical"
canonical.mkdir()
canonical_input = json.dumps(document()).encode()
for side, payload in [("base", b""), ("ours", canonical_input), ("theirs", b"{}")]:
    (canonical / side).write_bytes(payload)
MERGE_DRIVER_BODIES[DRIVER](
    str(canonical / "base"), str(canonical / "ours"), str(canonical / "theirs")
)
canonical_bytes = (canonical / "ours").read_bytes()

# Full min_length Unicode domain: Source's REAL schema rejects every surrogate
# in these TWO constrained fields, not unconstrained strings/summary values.
from pydantic import ValidationError
spectrum=[]
for field in ['input_key','question']:
    for code in range(0xd800,0xe000):
        data=entry(**{field:'k'+chr(code)})
        try:DecisionIndex.model_validate(data)
        except ValidationError as error:
            errors=error.errors()
            assert errors[0]['type']=='string_unicode' and errors[0]['loc']==('entries',0,field)
        else:raise AssertionError((field,code,'Source must reject constrained surrogate'))
        spectrum.append(dict(field=field,input=json.dumps(data)))
(root/'constrained-cs-model.json').write_text(json.dumps(spectrum))

# ---- valid: required-only entry projects EVERY field with its default ------
run_case(
    "minimal-entry-projects-defaults", document(),
    expect_top={"version": 1, "mission_id": "M1"},
    expect_entry={
        "decision_id": "D1", "origin_flow": "specify", "step_id": "step-1",
        "slot_key": None, "input_key": "k", "question": "q?", "options": [],
        "status": "open", "final_answer": None, "rationale": None,
        "other_answer": False, "summary_json": None,
        "created_at": "2026-01-02T03:04:05Z", "resolved_at": None,
        "resolved_by": None, "opened_by": None, "mission_id": "M1",
        "mission_slug": "demo",
    },
    expect_entry_keys=True,
)
run_case(
    "maximal-entry", document(entry={
        "origin_flow": "plan", "step_id": "s", "slot_key": "k", "options": ["x", "y"],
        "status": "resolved", "final_answer": "x", "rationale": "because",
        "other_answer": True, "summary_json": {"b": "2", "a": "1"},
        "created_at": "2026-01-02T03:04:05+05:30", "resolved_at": 1700000000,
        "resolved_by": "ivan", "opened_by": "ivan",
    }),
    expect_entry={
        "options": ["x", "y"], "status": "resolved", "other_answer": True,
        "summary_json": {"b": "2", "a": "1"},
        "created_at": "2026-01-02T03:04:05+05:30",
        "resolved_at": "2023-11-14T22:13:20Z",
    },
)
run_case("version-absent-defaults-one", {"mission_id": "M1", "entries": []},
         expect_top={"version": 1, "entries": []})
run_case("version-float-one", {"version": 1.0, "mission_id": "M1", "entries": []},
         expect_top={"version": 1})
run_case("version-bool-true", {"version": True, "mission_id": "M1", "entries": []},
         expect_top={"version": 1})
run_case("version-from-theirs", {"mission_id": "M1", "entries": []},
         theirs={"version": 1, "entries": []}, expect_top={"version": 1})
run_case("mission-id-empty-string-falls-back", {"mission_id": "", "entries": []},
         theirs={"mission_id": "M1"}, expect_top={"mission_id": "M1"})
run_case("entries-absent", {"mission_id": "M1"}, expect_top={"entries": []})
run_case("slot-only-entry", without("step_id", slot_key="s"),
         expect_entry={"step_id": None, "slot_key": "s"})
run_case("empty-step-with-slot", entry(step_id="", slot_key="k"),
         expect_entry={"step_id": "", "slot_key": "k"})
run_case("options-list-of-strings", entry(options=["a", "b"]),
         expect_entry={"options": ["a", "b"]})
run_case("options-empty", entry(options=[]), expect_entry={"options": []})
run_case("summary-json-object", entry(summary_json={"z": "9", "a": "1"}),
         expect_entry={"summary_json": {"z": "9", "a": "1"}})
run_case("summary-json-empty-object", entry(summary_json={}),
         expect_entry={"summary_json": {}})
run_case("boolean-word-yes", entry(other_answer="YES"), expect_entry={"other_answer": True})
run_case("boolean-word-off", entry(other_answer="off"), expect_entry={"other_answer": False})
run_case("boolean-int-one", entry(other_answer=1), expect_entry={"other_answer": True})
run_case("boolean-float-zero", entry(other_answer=0.0), expect_entry={"other_answer": False})
run_case("datetime-naive", entry(created_at="2026-01-02T03:04:05"),
         expect_entry={"created_at": "2026-01-02T03:04:05"})
run_case("datetime-space-separator", entry(created_at="2026-01-02 03:04:05"),
         expect_entry={"created_at": "2026-01-02T03:04:05"})
run_case("datetime-date-only-leap-day", entry(created_at="2024-02-29"),
         expect_entry={"created_at": "2024-02-29T00:00:00"})
run_case("datetime-minute-precision", entry(created_at="2026-01-02T03:04Z"),
         expect_entry={"created_at": "2026-01-02T03:04:00Z"})
# H7: separator and fractional-seconds grammar are independent axes.
for separator in map(chr, range(0x20, 0x7f)):
    value='2026-01-02'+separator+'03:04:05Z'
    if separator in ['T','t',' ','_']:
        run_case('h7-separator-'+hex(ord(separator)),entry(created_at=value),
                 expect_entry={'created_at':'2026-01-02T03:04:05Z'})
    else:
        run_case('h7-separator-'+hex(ord(separator)),entry(created_at=value),
                 field='created_at',cause='Input should be a valid datetime')
for field in ['created_at','resolved_at']:
    for value in ['2026-01-02T03:04.5','2026-01-02T03:04,5Z','2026-01-02_03:04.500000']:
        run_case('h7-no-seconds-fraction-'+field+'-'+value,entry(**{field:value}),
                 field=field,cause='unexpected extra characters at the end of the input')
for field in ['input_key','question']:
    for code in [0xd800,0xdbff,0xdc00,0xdfff]:
        run_case('h7-constrained-cs-'+field+'-'+hex(code),entry(**{field:'k'+chr(code)}),
                 field=field,cause='unable to parse raw data as a unicode string')
    run_case('h7-constrained-scalar-'+field,entry(**{field:'k\U0001f600'}),
             expect_entry={field:'k\U0001f600'})
run_case("datetime-offset-preserved", entry(created_at="2026-01-02T03:04:05+05:30"),
         expect_entry={"created_at": "2026-01-02T03:04:05+05:30"})
run_case("datetime-offset-without-colon", entry(created_at="2026-01-02T03:04:05-0800"),
         expect_entry={"created_at": "2026-01-02T03:04:05-08:00"})
run_case("datetime-fraction-truncated", entry(created_at="2026-01-02T03:04:05.123456789Z"),
         expect_entry={"created_at": "2026-01-02T03:04:05.123456Z"})
run_case("datetime-comma-fraction", entry(created_at="2026-01-02T03:04:05,5Z"),
         expect_entry={"created_at": "2026-01-02T03:04:05.500000Z"})
run_case("datetime-epoch-int", entry(created_at=1700000000),
         expect_entry={"created_at": "2023-11-14T22:13:20Z"})
run_case("datetime-epoch-negative-int", entry(created_at=-1),
         expect_entry={"created_at": "1969-12-31T23:59:59Z"})
run_case("datetime-epoch-float-rounds", entry(created_at=1700000000.1234567),
         expect_entry={"created_at": "2023-11-14T22:13:20.123457Z"})
run_case("datetime-epoch-negative-fraction", entry(created_at=-0.5),
         expect_entry={"created_at": "1969-12-31T23:59:59.500000Z"})
run_case("datetime-epoch-milliseconds", entry(created_at=20000000001),
         expect_entry={"created_at": "1970-08-20T11:33:20.001000Z"})
run_case("datetime-epoch-milliseconds-fraction", entry(created_at=253402300799.99),
         expect_entry={"created_at": "1978-01-11T21:31:40.799990Z"})
run_case("datetime-epoch-negative-ms-fraction", entry(created_at=-253402300799.99),
         expect_entry={"created_at": "1961-12-21T02:28:19.200990Z"})
run_case("datetime-maximum-milliseconds", entry(created_at=253402300799999),
         expect_entry={"created_at": "9999-12-31T23:59:59.999000Z"})
run_case("datetime-numeric-string-epoch", entry(created_at="20260102"),
         expect_entry={"created_at": "1970-08-23T11:48:22Z"})
run_case("datetime-numeric-string-float", entry(created_at="1.7e9"),
         expect_entry={"created_at": "2023-11-14T22:13:20Z"})
run_case("datetime-resolved-at-date", entry(resolved_at="2026-01-02"),
         expect_entry={"resolved_at": "2026-01-02T00:00:00"})
run_case("origin-flow-charter", entry(origin_flow="charter"),
         expect_entry={"origin_flow": "charter"})
run_case("status-canceled", entry(status="canceled"),
         expect_entry={"status": "canceled"})
run_case("status-deferred", entry(status="deferred"),
         expect_entry={"status": "deferred"})
run_case("boolean-word-t", entry(other_answer="T"), expect_entry={"other_answer": True})
run_case("boolean-string-one", entry(other_answer="1"), expect_entry={"other_answer": True})
run_case("input-key-single-space", entry(input_key=" "),
         expect_entry={"input_key": " "})
run_case("datetime-lowercase-z", entry(created_at="2026-01-02T03:04:05z"),
         expect_entry={"created_at": "2026-01-02T03:04:05Z"})
run_case("datetime-lowercase-t", entry(created_at="2026-01-02t03:04:05Z"),
         expect_entry={"created_at": "2026-01-02T03:04:05Z"})
run_case("datetime-negative-zero-offset", entry(created_at="2026-01-02T03:04:05-00:00"),
         expect_entry={"created_at": "2026-01-02T03:04:05Z"})
run_case("datetime-maximum-naive", entry(created_at="9999-12-31T23:59:59.999999"),
         expect_entry={"created_at": "9999-12-31T23:59:59.999999"})
run_case("datetime-numeric-string-plus-sign", entry(created_at="+1"),
         expect_entry={"created_at": "1970-01-01T00:00:01Z"})
run_case("datetime-numeric-string-leading-zero", entry(created_at="01"),
         expect_entry={"created_at": "1970-01-01T00:00:01Z"})
run_case("sort-uses-isoformat-not-json-form", {
    "mission_id": "M1",
    "entries": [
        dict(ENTRY) | {"decision_id": "Z", "created_at": "2026-01-02T03:04:05Z"},
        dict(ENTRY) | {"decision_id": "A", "created_at": "2026-01-02T03:04:05+05:30"},
        dict(ENTRY) | {"decision_id": "M", "created_at": "2026-01-02T03:04:05"},
    ],
}, expect_order=["M", "Z", "A"])
run_case("terminal-beats-open", document(),
         theirs={"entries": [dict(ENTRY) | {"status": "resolved",
                                            "resolved_at": "2026-01-02T03:04:06Z"}]},
         expect_entry={"status": "resolved",
                       "resolved_at": "2026-01-02T03:04:06Z"})
run_case("duplicate-decision-id-last-wins", {
    "mission_id": "M1",
    "entries": [dict(ENTRY), dict(ENTRY) | {"status": "deferred"}],
}, expect_order=["D1"])
run_case("divergent-invalid-status", document(),
         theirs={"entries": [dict(ENTRY) | {"status": "bogus"}]}, field="status",
         cause="has an invalid 'status'")
run_case("divergent-terminal-conflict",
         document(entry={"status": "resolved"}),
         theirs={"entries": [dict(ENTRY) | {"status": "canceled"}]}, field="diverged",
         cause="diverged on both sides with no safe precedence")
run_case("divergent-terminal-reopen-deferred-to-resolved",
         document(entry={"status": "deferred"}),
         theirs={"entries": [dict(ENTRY) | {"status": "resolved"}]},
         expect_entry={"status": "resolved"})
# Operator-authorized exception: only JSON-mode serialization (NOT validation)
# corrupts unpaired-surrogate summary_json keys. The separate preservation-
# exception suite proves actual Source loss and Native preservation; no other
# surrogate field/validator is excepted (input_key/question above MUST refuse).
run_case("surrogate-and-unicode-preserved", entry(
    options=["\u00e9", "\ud800"],
    summary_json={"caf\u00e9": "\udfff"},
    decision_id="D\ud800",
), expect_entry={"options": ["\u00e9", "\ud800"],
                 "summary_json": {"caf\u00e9": "\udfff"},
                 "decision_id": "D\ud800"})

# ---- refusals: the model's own domain ------------------------------------
run_case("invalid-status", entry(status="bogus"), field="status",
         cause="Input should be 'open', 'resolved', 'deferred' or 'canceled'")
run_case("invalid-origin-flow", entry(origin_flow="CHARTER"), field="origin_flow",
         cause="Input should be 'charter', 'specify' or 'plan'")
run_case("invalid-created-at", entry(created_at="not-a-date"), field="created_at",
         cause="Input should be a valid datetime")
run_case("invalid-calendar-date", entry(created_at="2026-13-02T03:04:05Z"),
         field="created_at", cause="Input should be a valid datetime")
run_case("created-at-boolean", entry(created_at=True), field="created_at",
         cause="Input should be a valid datetime")
run_case("created-at-after-9999", entry(created_at=253402300800000), field="created_at",
         cause="dates after 9999 are not supported as unix timestamps")
run_case("resolved-at-boolean", entry(resolved_at=True), field="resolved_at",
         cause="Input should be a valid datetime")
run_case("options-wrong-type", entry(options="notalist"), field="options",
         cause="Input should be a valid tuple")
run_case("options-item-wrong-type", entry(options=["a", 1]), field="options",
         cause="Input should be a valid string")
run_case("empty-input-key", entry(input_key=""), field="input_key",
         cause="String should have at least 1 character")
run_case("empty-question", entry(question=""), field="question",
         cause="String should have at least 1 character")
run_case("step-and-slot-missing", without("step_id"), field="step_id",
         cause="step_id or slot_key required")
run_case("step-and-slot-both-empty", entry(step_id="", slot_key=""), field="step_id",
         cause="step_id or slot_key required")
run_case("top-level-version-false", {"version": False, "mission_id": "M1", "entries": []},
         field="version", cause="Input should be 1")
run_case("top-level-version-null", {"version": None, "mission_id": "M1", "entries": []},
         field="version", cause="Input should be 1")
run_case("summary-json-list", entry(summary_json=[["a", "b"]]), field="summary_json",
         cause="Input should be a valid dictionary")
run_case("options-object", entry(options={"a": "b"}), field="options",
         cause="Input should be a valid tuple")
run_case("datetime-leap-second", entry(created_at="2026-01-02T23:59:60Z"),
         field="created_at", cause="Input should be a valid datetime")
run_case("datetime-hour-24", entry(created_at="2026-01-02T24:00:00Z"),
         field="created_at", cause="Input should be a valid datetime")
run_case("datetime-offset-hours-only", entry(created_at="2026-01-02T03:04:05+05"),
         field="created_at", cause="Input should be a valid datetime")
run_case("datetime-string-exponent-needs-dot", entry(created_at="1e10"),
         field="created_at", cause="Input should be a valid datetime")
run_case("datetime-string-infinity", entry(created_at="inf"), field="created_at",
         cause="Input should be a valid datetime")
run_case("datetime-before-range", entry(created_at=-253402300799999),
         field="created_at", cause="dates before 0000 are not supported as unix timestamps")
run_case("datetime-nan", entry(created_at=float("nan")), field="created_at",
         cause="NaN values not permitted")
run_case("datetime-infinity", entry(created_at=float("inf")), field="created_at",
         cause="dates after 9999 are not supported as unix timestamps")
run_case("other-answer-out-of-range", entry(other_answer=2), field="other_answer",
         cause="Input should be a valid boolean")
run_case("other-answer-word-unknown", entry(other_answer="bogus"), field="other_answer",
         cause="Input should be a valid boolean")
run_case("other-answer-null", entry(other_answer=None), field="other_answer",
         cause="Input should be a valid boolean")
run_case("summary-json-value-not-string", entry(summary_json={"a": 1}),
         field="summary_json", cause="Input should be a valid string")
run_case("summary-json-value-null", entry(summary_json={"a": None}),
         field="summary_json", cause="Input should be a valid string")
run_case("final-answer-not-string", entry(final_answer=1), field="final_answer",
         cause="Input should be a valid string")
run_case("entry-extra-field", entry(custom=1), field="custom",
         cause="Extra inputs are not permitted")
run_case("entry-missing-origin-flow", without("origin_flow"), field="origin_flow",
         cause="Field required")
run_case("entry-missing-mission-slug", without("mission_slug"), field="mission_slug",
         cause="Field required")
run_case("entry-missing-mission-id", without("mission_id"), field="mission_id",
         cause="Field required")
run_case("entry-decision-id-not-a-string", entry(decision_id=1), field="decision_id",
         cause="entry missing 'decision_id'")
run_case("top-level-extra-field", {"mission_id": "M1", "custom": 1, "entries": []},
         field="custom", cause="Extra inputs are not permitted")
run_case("top-level-version-two", {"version": 2, "mission_id": "M1", "entries": []},
         field="version", cause="Input should be 1")
run_case("top-level-version-string", {"version": "1", "mission_id": "M1", "entries": []},
         field="version", cause="Input should be 1")
run_case("top-level-mission-id-missing", {"entries": []}, field="mission_id",
         cause="Field required")
run_case("top-level-mission-id-not-a-string", {"mission_id": 7, "entries": []},
         field="mission_id", cause="Input should be a valid string")
run_case("top-level-entries-null", {"mission_id": "M1", "entries": None},
         field="entries", cause="'entries' is not a list")
run_case("top-level-entries-object", {"mission_id": "M1", "entries": {"a": 1}},
         field="entries", cause="'entries' is not a list")

# ---- the PUBLIC raw union keeps its distinct contract --------------------
raw = {}
for label, ours, theirs in [
    ("raw-both-empty", {}, {}),
    ("raw-empty-string-falls-back", {"mission_id": "", "version": ""},
     {"mission_id": "MT", "version": 2, "custom": 1}),
    ("raw-side-only", {"version": 2}, {}),
]:
    raw[label] = union_decision_index(ours, theirs)
assert raw["raw-both-empty"] == {"entries": []}, raw["raw-both-empty"]
assert raw["raw-empty-string-falls-back"] == {
    "mission_id": "MT", "version": 2, "custom": 1, "entries": []
}
assert raw["raw-side-only"] == {"version": 2, "entries": []}

(root / "cases.json").write_text(json.dumps(cases), encoding="utf-8")
(root / "raw-union.json").write_text(json.dumps(raw), encoding="utf-8")
print(json.dumps({
    "cases": len(cases),
    "refusals": sum(1 for case in cases if not case["ok"]),
    "drivers_sha256": hashlib.sha256(
        (repo / "src/specify_cli/consolidation/drivers.py").read_bytes()).hexdigest(),
    "models_sha256": hashlib.sha256(
        (repo / "src/specify_cli/decisions/models.py").read_bytes()).hexdigest(),
}))
"#;

#[test]
fn decision_index_model_projection_matches_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    if let Some(root) = std::env::var_os("SK_H6_NATIVE") {
        let root = PathBuf::from(root);
        assert_eq!(std::env::current_dir().unwrap(), root);
        for (key, name) in roots() {
            assert_eq!(std::env::var_os(key).unwrap(), root.join(name));
        }
        assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
        assert_eq!(
            std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
            repo.join("packs")
        );
        let spectrum: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(root.join("constrained-cs-model.json")).unwrap())
                .unwrap();
        assert_eq!(spectrum.len(), 4096);
        for sample in spectrum {
            let field = sample["field"].as_str().unwrap();
            let input = sample["input"].as_str().unwrap().as_bytes();
            let error = sk_workspace::merge_drivers::run_driver(
                "merge-driver-decision-index",
                sk_workspace::merge_drivers::MergeDriverInputs {
                    base: Some(b""),
                    ours: Some(input),
                    theirs: Some(b"{}"),
                },
            )
            .unwrap_err();
            assert!(
                error.to_string().contains(field),
                "constrained-field refusal must identify {field}: {error}"
            );
        }
        let cases: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(root.join("cases.json")).unwrap()).unwrap();
        let mut refusals = 0usize;
        for (index, case) in cases.iter().enumerate() {
            let name = case["name"].as_str().unwrap();
            let ours = case["ours"].as_str().unwrap().as_bytes().to_vec();
            let theirs = case["theirs"].as_str().unwrap().as_bytes().to_vec();
            let directory = root.join(format!("native-{index}"));
            fs::create_dir(&directory).unwrap();
            for (side, bytes) in [
                ("base", b"".to_vec()),
                ("ours", ours.clone()),
                ("theirs", theirs.clone()),
            ] {
                fs::write(directory.join(side), &bytes).unwrap();
            }
            let destination = directory.join("ours-out");
            fs::write(&destination, &ours).unwrap();
            let empty: &[u8] = b"";
            let run = |ours_bytes: &[u8], theirs_bytes: &[u8]| {
                sk_workspace::merge_drivers::run_driver_into(
                    "merge-driver-decision-index",
                    sk_workspace::merge_drivers::MergeDriverInputs {
                        base: Some(empty),
                        ours: Some(ours_bytes),
                        theirs: Some(theirs_bytes),
                    },
                    &destination,
                )
            };
            let outcome = run(&ours, &theirs);
            assert_eq!(
                outcome.is_ok(),
                case["ok"].as_bool().unwrap(),
                "{name}: {outcome:?}"
            );
            assert_eq!(
                fs::read(directory.join("base")).unwrap(),
                b"".to_vec(),
                "{name}: base untouched"
            );
            assert_eq!(
                fs::read(directory.join("theirs")).unwrap(),
                theirs.clone(),
                "{name}: theirs untouched"
            );
            let written = fs::read(&destination).unwrap();
            if let Err(error) = &outcome {
                refusals += 1;
                let text = error.to_string();
                assert!(
                    ["decisions/index.json", "ours:", "theirs:", "decision "]
                        .iter()
                        .any(|prefix| text.contains(prefix)),
                    "{name}: {text}"
                );
                if let Some(field) = case["field"].as_str() {
                    assert!(text.contains(field), "{name} names {field:?}: {text}");
                }
                assert_eq!(written, ours, "{name}: %A untouched on a refusal");
                let retry = run(&ours, &theirs).err();
                assert!(
                    retry.is_some(),
                    "{name}: an unchanged refusal stays a refusal"
                );
                assert_eq!(
                    fs::read(&destination).unwrap(),
                    ours,
                    "{name}: %A still untouched after the second refusal"
                );
                let corrected: Vec<u8> =
                    serde_json::from_value(case["retry_bytes"].clone()).unwrap();
                let outcome = run(&corrected, b"{}");
                assert!(
                    outcome.is_ok(),
                    "{name}: corrected data must succeed: {outcome:?}"
                );
                assert_eq!(
                    fs::read(&destination).unwrap(),
                    corrected,
                    "{name}: the retry writes the canonical document"
                );
                continue;
            }
            let expected: Vec<u8> = serde_json::from_value(case["bytes"].clone()).unwrap();
            assert_eq!(written, expected, "{name}: exact projection bytes");
            let outcome = outcome.expect("a successful driver outcome");
            assert_eq!(outcome.bytes, expected, "{name}: returned bytes");
            assert!(outcome.notice.is_none(), "{name}: no notice");
        }
        assert!(refusals > 0, "the suite must exercise refusals");
        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("raw-union.json")).unwrap()).unwrap();
        for (label, expected) in raw.as_object().unwrap() {
            let input: serde_json::Value = match label.as_str() {
                "raw-both-empty" => serde_json::json!([{}, {}]),
                "raw-empty-string-falls-back" => serde_json::json!([
                    {"mission_id": "", "version": ""},
                    {"mission_id": "MT", "version": 2, "custom": 1}
                ]),
                _ => serde_json::json!([{"version": 2}, {}]),
            };
            let actual = sk_workspace::merge_drivers::union_decision_index(
                input[0].as_object().unwrap(),
                input[1].as_object().unwrap(),
            )
            .unwrap();
            assert_eq!(
                serde_json::Value::Object(actual),
                expected.clone(),
                "{label}: public raw union"
            );
        }
        return;
    }

    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    eprintln!("H6 DecisionIndex evidence: {}", root.display());
    let source = isolated(&root, &repo, oracle::python())
        .args(["-c", SOURCE])
        .output()
        .unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {} {}",
        String::from_utf8_lossy(&source.stdout),
        String::from_utf8_lossy(&source.stderr)
    );
    let native = isolated(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "decision_index_model_projection_matches_actual_source",
            "--nocapture",
        ])
        .env("SK_H6_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}
