//! Source-first oracle for the pure `ActionRouter`.
//!
//! The authority is the pinned Python oracle (`.venv-rust-oracle`, CPython
//! 3.11.15, 53 locked runtime requirements, Unicode 14.0.0) verified through the
//! shared fail-closed harness in `tests/support` (`run_python` asserts the
//! isolated CWD/HOME/XDG/TMP roots *inside* the interpreter and runs
//! `scripts/rust/verify_oracle.py` before the first probe). A missing or drifted
//! interpreter fails the test instead of producing an empty comparison.
//!
//! Four independent comparisons:
//!
//! 1. the interpreter pin itself (exact version, Unicode version, 53 locked
//!    requirements),
//! 2. the reference tables (45 stop words, 59 canonical verbs, the eight
//!    canonical role rows) against the real Source modules,
//! 3. every one of the 1,114,112 code points for `\w` membership, the
//!    `str.lower()` mapping and the `Final_Sigma` context classes, plus
//!    composed/deterministic sequences through the whole lowercasing pipeline,
//! 4. 54 real routing cases: the projected `ProfileRegistry` snapshot and the
//!    exact `ActionRouter` decision or error, field by field, in code points.
//!
//! Every fixture root is a real temporary project (legacy
//! `.kittify/profiles/*.agent.yaml` overlay plus `activation.yaml` allow lists
//! and one `config.yaml` language scope) created under the sandbox `TMPDIR`, so
//! the reference catalogue is real Source state, not a monkeypatched guard.

#[path = "../tests/support/mod.rs"]
mod support;

use std::collections::BTreeMap;

use crate::{
    ActionRouter, CanonicalVerbMatch, PythonString, RoutableProfile, RouterConfidence,
    RouterErrorCode, STOP_WORDS, canonical_verb, canonical_verb_entries, normalize_tokens,
    router_unicode,
};
use serde_json::Value;

/// `(profile_id, action, confidence, match_reason)` of one candidate.
type AlternativeKey = (PythonString, String, String, PythonString);
type DecisionKey = (
    PythonString,
    String,
    String,
    PythonString,
    Vec<AlternativeKey>,
);
type ErrorKey = (
    String,
    PythonString,
    PythonString,
    PythonString,
    Vec<AlternativeKey>,
);

const PIN_SCRIPT: &str = r#"
import json, os, subprocess, sys, unicodedata

root = os.path.dirname(os.environ["PYTHONPATH"])
report = subprocess.run(
    [sys.executable, os.path.join(root, "scripts/rust/verify_oracle.py"), "--root", root],
    capture_output=True,
    text=True,
    check=True,
    cwd=os.environ["TMPDIR"],
)
pin = json.loads(report.stdout)
print(json.dumps({
    "python": ".".join(str(part) for part in sys.version_info[:3]),
    "implementation": sys.implementation.name,
    "unidata": unicodedata.unidata_version,
    "requirements": pin["checked_runtime_requirements"],
    "ok": pin["ok"],
    "drift": pin["drift"],
    "missing": pin["missing"],
}))
"#;

const TABLES_SCRIPT: &str = r#"
import json, sys

from charter.offering.agent_profiles.capabilities import DEFAULT_ROLE_CAPABILITIES
from specify_cli.invocation.router import CANONICAL_VERB_MAP, STOP_WORDS


def cps(text):
    return [ord(character) for character in text]


def row(caps):
    return {
        "role": cps(str(caps.role)),
        "default_capabilities": [cps(item) for item in caps.default_capabilities],
        "canonical_verbs": [cps(item) for item in caps.canonical_verbs],
        "description": cps(caps.description),
    }


print(json.dumps({
    "stop_words": sorted(cps(word) for word in STOP_WORDS),
    "canonical_verbs": sorted(
        [cps(token), cps(action), cps(str(role))]
        for token, (action, role) in CANONICAL_VERB_MAP.items()
    ),
    "roles": [row(caps) for caps in DEFAULT_ROLE_CAPABILITIES.values()],
}))
"#;

const UNICODE_SCRIPT: &str = r#"
import json, sys, unicodedata

LAST = 0x110000
CAPITAL_SIGMA = "\u03a3"
CAPITAL_ALPHA = "\u0391"
SMALL_FINAL_SIGMA = "\u03c2"


def merged(codes):
    ranges = []
    for code in codes:
        if ranges and ranges[-1][1] + 1 == code:
            ranges[-1][1] = code
        else:
            ranges.append([code, code])
    return ranges


word = []
lower_map = []
cased = []
ignorable = []
suffix_final = []
for code in range(LAST):
    character = chr(code)
    if character.isalnum() or code == 0x5F:
        word.append(code)
    mapped = [ord(item) for item in character.lower()]
    if mapped != [code]:
        lower_map.append([code, mapped])
    if (character + CAPITAL_SIGMA).lower()[-1] == SMALL_FINAL_SIGMA:
        cased.append(code)
    elif (CAPITAL_ALPHA + character + CAPITAL_SIGMA).lower()[-1] == SMALL_FINAL_SIGMA:
        ignorable.append(code)
    # Independent suffix observation, not inferred from the two before-probes.
    if (CAPITAL_ALPHA + CAPITAL_SIGMA + character + CAPITAL_ALPHA).lower()[1] == SMALL_FINAL_SIGMA:
        suffix_final.append(code)

alphabet = [
    0x41, 0x42, 0x5A, 0x61, 0x62, 0x7A, 0x5F, 0x2D, 0x2E, 0x3A, 0x27, 0x20, 0x35, 0xA0, 0x301,
    0x345, 0x391, 0x3A3, 0x3C2, 0x3C3, 0x3B1, 0x130, 0x131, 0xD800, 0x1F600, 0x378, 0x660,
    0x6F0, 0x1F08, 0x10FFFF,
]
composed = []
for item in alphabet:
    for pattern in ("{0}\u03a3", "\u0391{0}\u03a3", "\u0391\u03a3{0}", "\u0391\u03a3{0}\u0391",
                    "{0}\u0391\u03a3{0}", "\u03a3{0}\u03a3"):
        text = pattern.format(chr(item))
        composed.append([[ord(character) for character in text],
                         [ord(character) for character in text.lower()]])

state = [20261104]
def draw():
    state[0] = (state[0] * 1103515245 + 12345) & 0x7FFFFFFF
    return state[0]

for _ in range(2000):
    length = 1 + draw() % 7
    sequence = [alphabet[draw() % len(alphabet)] for _ in range(length)]
    text = "".join(chr(code) for code in sequence)
    composed.append([sequence, [ord(character) for character in text.lower()]])

print(json.dumps({
    "unidata": unicodedata.unidata_version,
    "word": merged(word),
    "word_count": len(word),
    "lower": lower_map,
    "identity_count": LAST - len(lower_map),
    "cased": merged(cased),
    "ignorable": merged(ignorable),
    "suffix_final": merged(suffix_final),
    "composed": composed,
}))
"#;

const FIXTURE_SCRIPT: &str = r#"
import json, os, sys, tempfile
from pathlib import Path

from specify_cli.invocation.errors import RouterAmbiguityError
from specify_cli.invocation.registry import ProfileRegistry
from specify_cli.invocation.router import ActionRouter


def cps(text):
    return [ord(character) for character in text]


def quoted(text):
    return json.dumps(text, ensure_ascii=True)


PROFILE_TEMPLATE = """profile-id: {pid}
name: {name}
schema-version: "1.0"
roles:
{roles}
routing-priority: {priority}
purpose: Fixture profile for the native router oracle.
specialization:
  primary-focus: Deterministic routing fixture.
collaboration:
  canonical-verbs: [{verbs}]
specialization-context:
  domain-keywords: [{keywords}]
"""


def write_profile(root, filename, profile_id, roles, priority=50, keywords=(), verbs=()):
    directory = root / ".kittify" / "profiles"
    directory.mkdir(parents=True, exist_ok=True)
    body = PROFILE_TEMPLATE.format(
        pid=quoted(profile_id),
        name=quoted("fixture"),
        roles="\n".join("  - " + quoted(role) for role in roles),
        priority=priority,
        verbs=", ".join(quoted(verb) for verb in verbs),
        keywords=", ".join(quoted(keyword) for keyword in keywords),
    )
    (directory / (filename + ".agent.yaml")).write_text(body, encoding="utf-8")


def allow(root, ids):
    directory = root / ".kittify" / "charter"
    directory.mkdir(parents=True, exist_ok=True)
    if ids is None:
        (directory / "activation.yaml").write_text("activated_kinds: []\n", encoding="utf-8")
    else:
        (directory / "activation.yaml").write_text(
            "activated_agent_profiles: [" + ", ".join(ids) + "]\n", encoding="utf-8"
        )


def project(profile):
    context = getattr(profile, "specialization_context", None)
    keywords = list(context.domain_keywords) if context and context.domain_keywords else []
    collaboration = getattr(profile, "collaboration", None)
    verbs = list(collaboration.canonical_verbs) if collaboration and collaboration.canonical_verbs else []
    return {
        "profile_id": cps(profile.profile_id),
        "role": cps(str(profile.role)),
        "routing_priority": profile.routing_priority,
        "domain_keywords": [cps(keyword) for keyword in keywords],
        "canonical_verbs": [cps(verb) for verb in verbs],
    }


def alternative(candidate):
    return {
        "profile_id": cps(candidate["profile_id"]),
        "action": cps(candidate["action"]),
        "confidence": candidate.get("_confidence", candidate.get("confidence")),
        "match_reason": cps(candidate["match_reason"]),
    }


def run_case(root, request, hint):
    registry = ProfileRegistry(root)
    profiles = registry.list_all()
    case = {
        "catalogue_ids": [cps(profile.profile_id) for profile in profiles],
        "snapshot": [project(profile) for profile in profiles],
        "request": cps(request),
        "hint": None if hint is None else cps(hint),
    }
    try:
        decision = ActionRouter(registry).route(request, hint)
    except RouterAmbiguityError as error:
        case["error"] = {
            "code": error.error_code,
            "request_text": cps(error.request_text),
            "candidates": [alternative(candidate) for candidate in error.candidates],
            "suggestion": cps(error.suggestion),
            "str": cps(str(error)),
        }
        return case
    case["decision"] = {
        "profile_id": cps(decision.profile_id),
        "action": cps(decision.action),
        "confidence": decision.confidence,
        "match_reason": cps(decision.match_reason),
        "alternatives": [alternative(item) for item in decision.alternatives],
    }
    return case


work = Path(os.environ["TMPDIR"])
cases = {}


def fresh(name):
    return Path(tempfile.mkdtemp(prefix="route-" + name + "-", dir=work))


root = fresh("empty")
allow(root, [])
cases["empty_catalogue"] = run_case(root, "implement the widget", None)
cases["empty_catalogue_with_hint"] = run_case(root, "implement the widget", "implementer-ivan")

root = fresh("builtin")
cases["unknown_hint"] = run_case(root, "implement it", "no-such-profile")
cases["empty_hint"] = run_case(root, "implement it", "")
cases["no_candidates_builtin"] = run_case(root, "zzzz qqqq", None)
cases["ambiguous_builtin_implement"] = run_case(root, "please implement the widget", None)
cases["keyword_builtin_chart"] = run_case(root, "draw a chart of the pipeline", None)

root = fresh("hint")
allow(root, [])
write_profile(root, "hint-target", "hint-target", ["implementer"])
write_profile(root, "custom-target", "custom-target", ["senior-tech-lead"])
write_profile(root, "curator-target", "curator-target", ["curator"])
cases["hint_exact_verb"] = run_case(root, "review the code", "hint-target")
cases["hint_exact_default"] = run_case(root, "the widget", "hint-target")
cases["hint_custom_role_default"] = run_case(root, "the widget", "custom-target")
cases["hint_token_derived_not_role_default"] = run_case(root, "coordinate the release", "curator-target")
cases["hint_empty_string_is_miss"] = run_case(root, "implement it", "")
cases["hint_missing_available_repr"] = run_case(root, "implement it", "nope")
cases["hint_nul_id"] = run_case(root, "implement it", "hint\u0000target")

root = fresh("verb")
allow(root, [])
write_profile(root, "solo-impl", "solo-impl", ["implementer"])
cases["verb_unique"] = run_case(root, "implement it", None)
cases["stop_words_removed"] = run_case(root, "please kindly implement the widget for me", None)
cases["verb_case_insensitive"] = run_case(root, "IMPLEMENT it", None)
cases["verb_underscore_not_split"] = run_case(root, "review_the_change", None)
cases["verb_digits_not_matched"] = run_case(root, "123 456", None)
cases["verb_surrogate_prefix"] = run_case(root, "\ud800implement it", None)
cases["verb_nul_separator"] = run_case(root, "implement\u0000it", None)
cases["verb_unicode_no_match"] = run_case(root, "caf\u00e9 na\u00efve \u03a9 \u0130stanbul", None)

root = fresh("tie")
allow(root, [])
write_profile(root, "tie-b", "tie-b", ["manager"], 10)
write_profile(root, "tie-a", "tie-a", ["manager"], 10)
cases["verb_tie_ambiguous"] = run_case(root, "coordinate it", None)

root = fresh("priority")
allow(root, [])
write_profile(root, "pri-low", "pri-low", ["planner"], 10)
write_profile(root, "pri-high", "pri-high", ["planner"], 90)
cases["verb_priority_tiebreak"] = run_case(root, "plan it", None)

root = fresh("priority_bounds")
allow(root, [])
write_profile(root, "zero-pri", "zero-pri", ["designer"], 0, keywords=["zerokw"])
write_profile(root, "hundred-pri", "hundred-pri", ["designer"], 100, keywords=["hundredkw"])
write_profile(root, "verb-zero", "verb-zero", ["planner"], 0)
write_profile(root, "verb-hundred", "verb-hundred", ["planner"], 100)
cases["priority_zero"] = run_case(root, "zerokw", None)
cases["priority_hundred"] = run_case(root, "hundredkw", None)
cases["verb_priority_bounds"] = run_case(root, "plan it", None)

root = fresh("two-tier")
allow(root, [])
write_profile(root, "verb-manager", "verb-manager", ["manager"], 10, verbs=["coordinate"])
write_profile(root, "keyword-manager", "keyword-manager", ["curator"], 100, keywords=["coordinate"])
cases["two_tier_verb_beats_priority"] = run_case(root, "coordinate the release", None)

root = fresh("keyword")
allow(root, [])
write_profile(root, "kw-uniq", "kw-uniq", ["curator"], 50, keywords=["chart"])
write_profile(root, "multikey", "multikey", ["curator"], 50, keywords=["alpha", "beta"])
write_profile(root, "multiword", "multiword", ["curator"], 50, keywords=["two words"])
write_profile(root, "custom-role", "custom-role", ["senior-tech-lead"], 50, keywords=["legacywidget"])
write_profile(root, "impl-kw", "impl-kw", ["implementer"], 50, keywords=["implkw"])
write_profile(root, "review-kw", "review-kw", ["reviewer"], 50, keywords=["auditkw"])
write_profile(root, "verb-kw", "verb-kw", ["designer"], 50, verbs=["blueprint"])
cases["keyword_unique_action"] = run_case(root, "draw a chart", None)
cases["keyword_first_raw_order"] = run_case(root, "beta alpha", None)
cases["keyword_multiword_never_matches"] = run_case(root, "two words", None)
cases["keyword_custom_role_review"] = run_case(root, "legacywidget", None)
cases["keyword_implementer_generate"] = run_case(root, "implkw", None)
cases["keyword_reviewer_audit"] = run_case(root, "auditkw", None)
cases["keyword_from_collaboration_verbs"] = run_case(root, "blueprint", None)
cases["keyword_unicode_no_match"] = run_case(root, "caf\u00e9", None)
cases["keyword_sigma_no_match"] = run_case(root, "\u0391\u03a3", None)
cases["keyword_istanbul_no_match"] = run_case(root, "\u0130stanbul", None)

root = fresh("case-keyword")
allow(root, [])
write_profile(root, "case-kw", "case-kw", ["architect"], 50, keywords=["Chart"])
cases["keyword_original_case_in_reason"] = run_case(root, "chart", None)

root = fresh("keyword-priority")
allow(root, [])
write_profile(root, "kw-low", "kw-low", ["curator"], 10, keywords=["tieup"])
write_profile(root, "kw-high", "kw-high", ["manager"], 90, keywords=["tieup"])
cases["keyword_priority_tiebreak"] = run_case(root, "tieup", None)

root = fresh("dedup")
allow(root, [])
write_profile(root, "verb-and-kw", "verb-and-kw", ["manager"], 50,
              keywords=["coordinate"], verbs=["coordinate"])
cases["verb_candidate_not_duplicated"] = run_case(root, "coordinate", None)

root = fresh("multi-role")
allow(root, [])
write_profile(root, "multi-role", "multi-role", ["researcher", "implementer"], 50,
              keywords=["multifold"])
cases["multi_role_primary_only_verb"] = run_case(root, "implement it", None)
cases["multi_role_primary_matches"] = run_case(root, "research it", None)
cases["multi_role_keyword_still_matches"] = run_case(root, "multifold", None)

root = fresh("codepoints")
allow(root, [])
write_profile(root, "nul-id", "nul\u0000id", ["manager"], 70)
write_profile(root, "surrogate-id", "sur\ud800gate", ["manager"], 60)
cases["nul_profile_id"] = run_case(root, "coordinate", None)
cases["surrogate_profile_id"] = run_case(root, "coordinate", None)
cases["surrogate_hint"] = run_case(root, "coordinate", "sur\ud800gate")
cases["surrogate_hint_missing_repr"] = run_case(root, "coordinate", "sur\ud800nope")

root = fresh("token-order")
allow(root, [])
write_profile(root, "order-impl", "order-impl", ["implementer"], 50)
write_profile(root, "order-review", "order-review", ["reviewer"], 50)
cases["verb_token_order"] = run_case(root, "review and implement the thing", None)
cases["verb_same_role_first_token"] = run_case(root, "build then implement then code", None)

root = fresh("mixed")
allow(root, [])
write_profile(root, "mix-a", "mix-a", ["implementer"], 50)
write_profile(root, "mix-b", "mix-b", ["implementer"], 50)
write_profile(root, "mix-kw", "mix-kw", ["curator"], 100, keywords=["planet"])
cases["verb_tie_with_keyword_loser"] = run_case(root, "implement planet", None)

root = fresh("subset")
allow(root, ["reviewer-renata", "implementer-ivan"])
cases["activation_subset_verb"] = run_case(root, "implement it", None)
cases["activation_subset_review"] = run_case(root, "review it", None)

root = fresh("language")
(root / ".kittify").mkdir(parents=True, exist_ok=True)
(root / ".kittify" / "config.yaml").write_text("languages: [python]\n", encoding="utf-8")
cases["language_scoped_implement"] = run_case(root, "implement it", None)

print(json.dumps({"cases": cases}))
"#;

fn python_string(value: &Value) -> PythonString {
    let units: Vec<u32> = value
        .as_array()
        .unwrap_or_else(|| panic!("expected a code-point array, got {value}"))
        .iter()
        .map(|item| u32::try_from(item.as_u64().expect("integer code point")).expect("u32"))
        .collect();
    PythonString::from_codepoints(&units).expect("the oracle emits valid code points")
}

fn scalar(value: &Value) -> String {
    python_string(value)
        .to_scalar_string()
        .unwrap_or_else(|code| panic!("expected scalar text, found U+{code:04X}"))
}

fn ranges(value: &Value) -> Vec<(u32, u32)> {
    value
        .as_array()
        .expect("range array")
        .iter()
        .map(|row| {
            let pair = row.as_array().expect("range pair");
            (
                u32::try_from(pair[0].as_u64().expect("start")).expect("u32"),
                u32::try_from(pair[1].as_u64().expect("end")).expect("u32"),
            )
        })
        .collect()
}

fn contains(ranges: &[(u32, u32)], code: u32) -> bool {
    ranges
        .iter()
        .any(|&(start, end)| (start..=end).contains(&code))
}

fn alternatives(value: &Value) -> Vec<AlternativeKey> {
    value
        .as_array()
        .expect("alternatives array")
        .iter()
        .map(|item| {
            (
                python_string(&item["profile_id"]),
                scalar(&item["action"]),
                item["confidence"].as_str().expect("confidence").to_owned(),
                python_string(&item["match_reason"]),
            )
        })
        .collect()
}

fn decision_key(value: &Value) -> DecisionKey {
    (
        python_string(&value["profile_id"]),
        scalar(&value["action"]),
        value["confidence"].as_str().expect("confidence").to_owned(),
        python_string(&value["match_reason"]),
        alternatives(&value["alternatives"]),
    )
}

fn error_key(value: &Value) -> ErrorKey {
    (
        value["code"].as_str().expect("code").to_owned(),
        python_string(&value["request_text"]),
        python_string(&value["suggestion"]),
        python_string(&value["str"]),
        alternatives(&value["candidates"]),
    )
}

#[test]
fn oracle_is_the_pinned_interpreter_with_the_locked_closure() {
    let pin: Value =
        serde_json::from_str(&support::run_python(PIN_SCRIPT, None)).expect("oracle pin facts");
    assert_eq!(pin["implementation"], "cpython");
    assert_eq!(pin["python"], "3.11.15", "the pinned bug-fix release");
    assert_eq!(pin["unidata"], "14.0.0", "the pinned Unicode version");
    assert_eq!(
        pin["requirements"], 53,
        "the verified runtime requirement count"
    );
    assert_eq!(pin["ok"], true);
    assert_eq!(pin["drift"], serde_json::json!([]));
    assert_eq!(pin["missing"], serde_json::json!([]));
}

#[test]
fn reference_tables_match_the_source_modules() {
    let tables: Value =
        serde_json::from_str(&support::run_python(TABLES_SCRIPT, None)).expect("oracle tables");

    let mut expected_stop_words: Vec<Vec<u32>> = tables["stop_words"]
        .as_array()
        .expect("stop words")
        .iter()
        .map(|word| python_string(word).codepoints().to_vec())
        .collect();
    let mut native_stop_words: Vec<Vec<u32>> = STOP_WORDS
        .iter()
        .map(|word| word.chars().map(u32::from).collect())
        .collect();
    expected_stop_words.sort();
    native_stop_words.sort();
    assert_eq!(native_stop_words, expected_stop_words);
    assert_eq!(STOP_WORDS.len(), 45, "the reference stop-word count");

    let expected_verbs: Vec<(Vec<u32>, Vec<u32>, Vec<u32>)> = tables["canonical_verbs"]
        .as_array()
        .expect("canonical verbs")
        .iter()
        .map(|row| {
            (
                python_string(&row[0]).codepoints().to_vec(),
                python_string(&row[1]).codepoints().to_vec(),
                python_string(&row[2]).codepoints().to_vec(),
            )
        })
        .collect();
    let mut native_verbs: Vec<(Vec<u32>, Vec<u32>, Vec<u32>)> = canonical_verb_entries()
        .iter()
        .map(|(token, action, role)| {
            (
                token.chars().map(u32::from).collect(),
                action.chars().map(u32::from).collect(),
                role.chars().map(u32::from).collect(),
            )
        })
        .collect();
    native_verbs.sort();
    assert_eq!(native_verbs, expected_verbs);
    assert_eq!(
        canonical_verb_entries().len(),
        59,
        "the reference verb count"
    );
    for (token, action, role) in canonical_verb_entries() {
        assert_eq!(
            canonical_verb(
                &PythonString::from_codepoints(&token.chars().map(u32::from).collect::<Vec<_>>())
                    .expect("token")
            ),
            Some(CanonicalVerbMatch {
                token,
                action,
                role
            })
        );
    }

    let expected_roles = tables["roles"].as_array().expect("roles");
    assert_eq!(
        expected_roles.len(),
        sk_doctrine::ROLE_CAPABILITIES.len(),
        "eight canonical role rows"
    );
    for (expected, native) in expected_roles
        .iter()
        .zip(sk_doctrine::ROLE_CAPABILITIES.iter())
    {
        assert_eq!(scalar(&expected["role"]), native.role);
        let capabilities: Vec<String> = expected["default_capabilities"]
            .as_array()
            .expect("capabilities")
            .iter()
            .map(scalar)
            .collect();
        assert_eq!(
            capabilities,
            native
                .default_capabilities
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<String>>()
        );
        let verbs: Vec<String> = expected["canonical_verbs"]
            .as_array()
            .expect("verbs")
            .iter()
            .map(scalar)
            .collect();
        assert_eq!(
            verbs,
            native
                .canonical_verbs
                .iter()
                .map(|item| (*item).to_owned())
                .collect::<Vec<String>>()
        );
        assert_eq!(scalar(&expected["description"]), native.description);
        assert_eq!(
            sk_doctrine::role_capabilities(native.role),
            Some(native),
            "the exact-match lookup resolves the same row"
        );
    }
}

/// Every code point, exhaustively: `\w` membership, the `str.lower()` mapping,
/// the `Final_Sigma` context class, and the composed/deterministic sequences.
#[test]
fn every_unicode_code_point_matches_the_pinned_interpreter() {
    let facts: Value = serde_json::from_str(&support::run_python(UNICODE_SCRIPT, None))
        .expect("oracle unicode facts");
    assert_eq!(facts["unidata"], "14.0.0");

    let word = ranges(&facts["word"]);
    assert_eq!(
        facts["word_count"].as_u64().expect("count"),
        word.iter()
            .map(|&(start, end)| u64::from(end - start + 1))
            .sum::<u64>()
    );

    let lower_map: BTreeMap<u32, Vec<u32>> = facts["lower"]
        .as_array()
        .expect("lower map")
        .iter()
        .map(|row| {
            let entry = row.as_array().expect("lower row");
            let code = u32::try_from(entry[0].as_u64().expect("code")).expect("u32");
            let mapped: Vec<u32> = entry[1]
                .as_array()
                .expect("mapped")
                .iter()
                .map(|item| u32::try_from(item.as_u64().expect("mapped code")).expect("u32"))
                .collect();
            (code, mapped)
        })
        .collect();
    assert_eq!(
        lower_map.len() as u64,
        facts["identity_count"]
            .as_u64()
            .map(|identity| 0x110000 - identity)
            .expect("identity count")
    );

    let cased = ranges(&facts["cased"]);
    let ignorable = ranges(&facts["ignorable"]);
    let suffix_final = ranges(&facts["suffix_final"]);

    let mut word_mismatches = Vec::new();
    let mut lower_mismatches = Vec::new();
    let mut context_mismatches = Vec::new();
    for code in 0..0x110000u32 {
        let expected_word = contains(&word, code);
        if router_unicode::is_word(code) != expected_word {
            word_mismatches.push(code);
        }
        let mapped = router_unicode::lower_codepoints(&[code]);
        let expected_lower: &[u32] = lower_map
            .get(&code)
            .map_or(std::slice::from_ref(&code), Vec::as_slice);
        if mapped != expected_lower {
            lower_mismatches.push((code, mapped, expected_lower.to_vec()));
        }
        // These are effective scan classes, not disjoint Unicode properties.
        // Check both directions and a suffix with another cased letter against
        // its own direct Source observation for every code point.
        let expected_cased = contains(&cased, code);
        let expected_ignorable = contains(&ignorable, code);
        let after_probe = router_unicode::lower_codepoints(&[0x0391, 0x03a3, code]);
        let before_probe = router_unicode::lower_codepoints(&[0x0391, code, 0x03a3]);
        let after_is_final = after_probe.get(1).copied() == Some(0x03c2);
        let before_is_final = before_probe.last().copied() == Some(0x03c2);
        let suffix_probe = router_unicode::lower_codepoints(&[0x0391, 0x03a3, code, 0x0391]);
        let suffix_is_final = suffix_probe.get(1).copied() == Some(0x03c2);
        if after_is_final == expected_cased
            || before_is_final != (expected_cased || expected_ignorable)
            || suffix_is_final != contains(&suffix_final, code)
        {
            context_mismatches.push((code, after_is_final, before_is_final, suffix_is_final));
        }
    }
    assert!(
        word_mismatches.is_empty(),
        "word membership diverges at (first 8) {:x?}",
        &word_mismatches[..word_mismatches.len().min(8)]
    );
    assert!(
        lower_mismatches.is_empty(),
        "lower mapping diverges at (first 4) {:x?}",
        &lower_mismatches[..lower_mismatches.len().min(4)]
    );
    assert!(
        context_mismatches.is_empty(),
        "Final_Sigma classification diverges at (first 8) {:x?}",
        &context_mismatches[..context_mismatches.len().min(8)]
    );

    let composed = facts["composed"].as_array().expect("composed sequences");
    assert!(composed.len() >= 2000, "composed coverage is not vacuous");
    let mut composed_mismatches = Vec::new();
    for case in composed {
        let input = python_string(&case[0]);
        let expected = python_string(&case[1]);
        let produced = router_unicode::lower_codepoints(input.codepoints());
        if produced != expected.codepoints() {
            composed_mismatches.push((input, produced, expected));
        }
    }
    assert!(
        composed_mismatches.is_empty(),
        "composed lowercasing diverges (first 3): {:?}",
        &composed_mismatches[..composed_mismatches.len().min(3)]
    );
}

#[test]
fn every_router_case_matches_the_reference_decision_or_error() {
    let oracle: Value = serde_json::from_str(&support::run_python(FIXTURE_SCRIPT, None))
        .expect("oracle router fixtures");
    let cases = oracle["cases"].as_object().expect("cases object");
    assert_eq!(cases.len(), 54, "the fixture matrix is not vacuous");

    for (name, case) in cases {
        let catalogue_ids: Vec<PythonString> = case["catalogue_ids"]
            .as_array()
            .expect("catalogue ids")
            .iter()
            .map(python_string)
            .collect();
        let mut sorted = catalogue_ids.clone();
        sorted.sort();
        assert_eq!(
            catalogue_ids, sorted,
            "{name}: the projected catalogue is in registry (id) order"
        );

        let profiles: Vec<RoutableProfile> = case["snapshot"]
            .as_array()
            .expect("snapshot")
            .iter()
            .map(|row| {
                RoutableProfile::new(
                    python_string(&row["profile_id"]),
                    python_string(&row["role"]),
                    i32::try_from(row["routing_priority"].as_i64().expect("priority"))
                        .expect("i32"),
                    row["domain_keywords"]
                        .as_array()
                        .expect("keywords")
                        .iter()
                        .map(python_string)
                        .collect(),
                    row["canonical_verbs"]
                        .as_array()
                        .expect("verbs")
                        .iter()
                        .map(python_string)
                        .collect(),
                )
            })
            .collect();
        assert_eq!(
            profiles.len(),
            catalogue_ids.len(),
            "{name}: one snapshot row per catalogue entry"
        );

        let request = python_string(&case["request"]);
        let hint = if case["hint"].is_null() {
            None
        } else {
            Some(python_string(&case["hint"]))
        };

        let outcome = ActionRouter::route(&request, hint.as_ref(), &profiles);
        if let Some(expected) = case.get("decision") {
            let decision = outcome
                .unwrap_or_else(|error| panic!("{name}: expected a decision, got {error:?}"));
            let native: DecisionKey = (
                decision.profile_id.clone(),
                decision.action.to_owned(),
                decision.confidence.as_str().to_owned(),
                decision.match_reason.clone(),
                decision
                    .alternatives
                    .iter()
                    .map(|item| {
                        (
                            item.profile_id.clone(),
                            item.action.to_owned(),
                            item.confidence.as_str().to_owned(),
                            item.match_reason.clone(),
                        )
                    })
                    .collect(),
            );
            assert_eq!(&native, &decision_key(expected), "{name}");
            assert_ne!(
                decision.confidence,
                RouterConfidence::GenericFallback,
                "{name}: the router never produces the generic fallback"
            );
        } else {
            let expected = case.get("error").expect("decision or error");
            let error = outcome.expect_err(&format!("{name}: expected an error"));
            let native: ErrorKey = (
                error.code().as_str().to_owned(),
                error.request_text().clone(),
                error.suggestion().clone(),
                error.message(),
                error
                    .candidates()
                    .iter()
                    .map(|item| {
                        (
                            item.profile_id.clone(),
                            item.action.to_owned(),
                            item.confidence.as_str().to_owned(),
                            item.match_reason.clone(),
                        )
                    })
                    .collect(),
            );
            assert_eq!(&native, &error_key(expected), "{name}");
            let code = error.code();
            assert!(
                matches!(
                    code,
                    RouterErrorCode::NoMatch
                        | RouterErrorCode::ProfileNotFound
                        | RouterErrorCode::Ambiguous
                ),
                "{name}"
            );
        }
    }
}

/// The projection contract: the native router reproduces `normalize_tokens` for
/// the reference vectors the fixtures use, including surrogate and NUL input.
#[test]
fn token_normalization_matches_the_reference_vectors() {
    let script = r#"
import json, sys
from specify_cli.invocation.router import _normalize_tokens

vectors = {
    "empty": "",
    "spaces": "   ",
    "underscore": "review_the_change",
    "hyphen": "fix-bug",
    "stop_words": "please kindly implement the widget for me",
    "digits": "123 456",
    "accented": "caf\u00e9 na\u00efve",
    "greek": "\u03a9 omega",
    "istanbul": "\u0130stanbul",
    "emoji": "emoji \U0001f600 deploy",
    "combining": "foo\u0301bar",
    "sigma": "\u0391\u03a3",
    "surrogate": "\ud800implement it",
    "nul": "implement\u0000it",
    "nbsp": "\u00a0nbsp\u00a0split",
}
print(json.dumps({
    name: [[ord(character) for character in token] for token in _normalize_tokens(text)]
    for name, text in vectors.items()
}))
"#;
    let expected: Value =
        serde_json::from_str(&support::run_python(script, None)).expect("oracle token vectors");
    let requests: BTreeMap<&str, &str> = BTreeMap::from([
        ("empty", ""),
        ("spaces", "   "),
        ("underscore", "review_the_change"),
        ("hyphen", "fix-bug"),
        ("stop_words", "please kindly implement the widget for me"),
        ("digits", "123 456"),
        ("accented", "café naïve"),
        ("greek", "Ω omega"),
        ("istanbul", "İstanbul"),
        ("emoji", "emoji \u{1f600} deploy"),
        ("combining", "foo\u{301}bar"),
        ("sigma", "ΑΣ"),
        ("nbsp", "\u{a0}nbsp\u{a0}split"),
    ]);
    for (name, request) in requests {
        let native: Vec<Vec<u32>> = normalize_tokens(&PythonString::from_scalars(request))
            .iter()
            .map(|token| token.codepoints().to_vec())
            .collect();
        let oracle: Vec<Vec<u32>> = expected[name]
            .as_array()
            .expect("tokens")
            .iter()
            .map(|token| python_string(token).codepoints().to_vec())
            .collect();
        assert_eq!(native, oracle, "{name}");
    }
    // Surrogate/NUL vectors cannot be Rust `&str`; drive them through the typed
    // code-point path instead.
    for (name, request) in [
        (
            "surrogate",
            vec![
                0xd800, 0x69, 0x6d, 0x70, 0x6c, 0x65, 0x6d, 0x65, 0x6e, 0x74, 0x20, 0x69, 0x74,
            ],
        ),
        (
            "nul",
            vec![
                0x69, 0x6d, 0x70, 0x6c, 0x65, 0x6d, 0x65, 0x6e, 0x74, 0x00, 0x69, 0x74,
            ],
        ),
    ] {
        let native: Vec<Vec<u32>> =
            normalize_tokens(&PythonString::from_codepoints(&request).expect("typed request"))
                .iter()
                .map(|token| token.codepoints().to_vec())
                .collect();
        let oracle: Vec<Vec<u32>> = expected[name]
            .as_array()
            .expect("tokens")
            .iter()
            .map(|token| python_string(token).codepoints().to_vec())
            .collect();
        assert_eq!(native, oracle, "{name}");
    }
}
