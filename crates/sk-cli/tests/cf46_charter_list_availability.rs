//! CF46 focused smoke: `charter list` composes a **lenient** roster (a declared
//! but unfetched org pack no longer aborts the listing), the reference's
//! availability rows and its required-floor status cells, while every other
//! charter command keeps its strict pack read.
//!
//! These are scoped behaviour checks against real fixtures and the pinned
//! built-in pack, not a whole-command parity campaign. Deep differential,
//! non-UTF-8, Rich-geometry and whole-`list_cmd` matrix proof is deferred and
//! listed in the lane report.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use sk_cli::compat::charter;

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

fn packs_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packs/built-in")
        .canonicalize()
        .expect("built-in pack tree")
}

/// A project declaring an existing org pack, optionally beside an unfetched sibling.
///
/// The unfetched pack is what used to abort `charter list` through the strict
/// `Roots::load`; the listing must drop it from the roster instead. The floor
/// tests use `missing_org = false` because the native lenient floor read still
/// resolves the registry through the strict pack-root resolver (a lower-helper
/// divergence recorded in the lane report: with an unfetched pack declared, the
/// lenient read yields no floors where Source skips only that pack).
fn fixture(ledger: &str, org_charter: &str, missing_org: bool) -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path();
    let missing = if missing_org {
        "      - name: org-missing\n        local_path: org-not-fetched\n"
    } else {
        ""
    };
    write(
        &root.join(".kittify/config.yaml"),
        &format!(
            "charter_packs:\n  org:\n    packs:\n\
             \x20     - name: org-present\n        local_path: org-present\n{missing}"
        ),
    );
    write(&root.join(".kittify/charter/activation.yaml"), ledger);
    // The strict `context_payload` path runs the Source bundle-freshness gate,
    // which resolves the canonical read root through Git; without a repository the
    // refusal would come from that resolution instead of the unfetched org pack the
    // assertion below is about (a vacuous pass).
    let status = std::process::Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .arg(root)
        .status()
        .expect("git is available");
    assert!(status.success(), "fixture git init failed");
    write(&root.join("org-present/org-charter.yaml"), org_charter);
    // An org-layer directive the built-in pack does not ship.
    write(
        &root.join("org-present/directives/acme-local.directive.yaml"),
        "schema_version: \"1.0\"\nid: DIRECTIVE_ACME_LOCAL\ntitle: Acme local\nintent: test\n",
    );
    // The project layer wins the duplicated template id against an org pack.
    write(
        &root.join(".kittify/charter-packs/missions/shared/templates/same.md"),
        "project shared\n",
    );
    write(
        &root.join("org-present/missions/shared/templates/same.md"),
        "org shared\n",
    );
    temp
}

fn row<'a>(payload: &'a Value, kind: &str) -> &'a Value {
    payload["kinds"]
        .as_array()
        .expect("kinds")
        .iter()
        .find(|row| row["kind"] == kind)
        .unwrap_or_else(|| panic!("missing kind row {kind}: {payload:#}"))
}

fn available_ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("available array")
        .iter()
        .map(|entry| {
            entry
                .get("artifact_id")
                .and_then(Value::as_str)
                .or_else(|| entry.as_str())
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_spec-kitty"))
        .current_dir(root)
        .env("HOME", root.join(".fixture-home"))
        .env("SPEC_KITTY_PACKS_ROOT", packs_root())
        .env_remove("SPECIFY_REPO_ROOT")
        .args(args)
        .output()
        .expect("binary runs")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error}): {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// Every fixture file's bytes, size, mode and modification time.
fn footprint(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, u64, u32, i64)> {
    let mut entries = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(&directory).expect("read dir") {
            let path = entry.expect("dir entry").path();
            let metadata = fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                stack.push(path);
                continue;
            }
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|delta| delta.as_nanos() as i64)
                .unwrap_or(-1);
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                metadata.mode()
            };
            #[cfg(not(unix))]
            let mode = 0;
            entries.insert(
                path.clone(),
                (
                    fs::read(&path).expect("bytes"),
                    metadata.len(),
                    mode,
                    modified,
                ),
            );
        }
    }
    entries
}

const EMPTY_ORG_CHARTER: &str = "schema_version: 2\norg_name: acme\n";
const NO_ALLOW_KEY: &str = "# no activated_directives key\n";

#[test]
fn an_unfetched_org_pack_no_longer_aborts_the_listing() {
    let temp = fixture(
        "activated_directives:\n- 001-architectural-integrity-standard\n",
        EMPTY_ORG_CHARTER,
        true,
    );
    let root = temp.path();

    // The plain listing, with `--show-available` and with `--all`, all succeed
    // even though `org-not-fetched` is declared and absent.
    let plain = charter::list_payload(root, false, false).expect("plain list");
    assert_eq!(plain["result"], "success");
    assert!(plain["templates"].is_null());
    assert!(row(&plain, "directive")["available"].is_null());
    assert_eq!(
        row(&plain, "directive")["activated"],
        serde_json::json!(["001-architectural-integrity-standard"])
    );

    let show = charter::list_payload(root, true, false).expect("show-available");
    let ids = available_ids(&row(&show, "directive")["available"]);
    assert!(
        ids.iter()
            .any(|id| id == "003-decision-documentation-requirement"),
        "{ids:?}"
    );
    // `--show-available` alone is the reference's built-in-only view.
    assert!(!ids.iter().any(|id| id == "acme-local"), "{ids:?}");

    let all = charter::list_payload(root, false, true).expect("list --all");
    let rows = row(&all, "directive")["available"]
        .as_array()
        .expect("layer rows");
    assert!(
        rows.iter().any(|entry| entry["artifact_id"] == "acme-local"
            && entry["layer"] == "org"
            && entry["declared_id"] == "DIRECTIVE_ACME_LOCAL"),
        "{rows:?}"
    );
    assert!(
        rows.iter().any(
            |entry| entry["artifact_id"] == "003-decision-documentation-requirement"
                && entry["layer"] == "built-in"
                && entry["declared_id"] == "DIRECTIVE_003"
        ),
        "{rows:?}"
    );

    // Other charter surfaces keep the strict pack read: the same registry that
    // the listing tolerates still refuses activation and context delivery.
    let strict = charter::activate(root, "directive", "DIRECTIVE_024", false)
        .expect_err("strict activation must refuse the unfetched pack");
    assert!(
        strict.message().contains("org-not-fetched") || strict.message().contains("org-missing"),
        "{}",
        strict.message()
    );
    assert!(
        charter::context_payload(root, "implement", Some("software-dev"), None, false).is_err()
    );
}

#[test]
fn the_required_floor_reports_in_force_and_omitted_ids_like_the_reference() {
    let org_charter = "schema_version: 2\norg_name: acme\n\
                       required_directives:\n\
                       - 003-decision-documentation-requirement\n\
                       - missing-directive-id\n";

    // A PRESENT allow list omits the whole floor: each id is named, even the one
    // no layer provides (the reference never filters `required_not_in_force` by
    // availability).
    let present = fixture(
        "activated_directives:\n- 001-architectural-integrity-standard\n",
        org_charter,
        false,
    );
    let payload = charter::list_payload(present.path(), false, false).expect("list");
    let omitted = payload["required_not_in_force"]["directives"]
        .as_array()
        .expect("required_not_in_force");
    let omitted_ids: Vec<&str> = omitted
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        omitted_ids,
        vec![
            "003-decision-documentation-requirement",
            "missing-directive-id"
        ]
    );
    for entry in omitted {
        assert_eq!(entry["packs"], serde_json::json!(["org-present"]));
    }
    assert!(payload.get("required_in_force").is_none());

    // An ABSENT allow key leaves only the ids some layer provides in force.
    let absent = fixture(NO_ALLOW_KEY, org_charter, false);
    let payload = charter::list_payload(absent.path(), false, false).expect("list");
    let in_force = payload["required_in_force"]["directives"]
        .as_array()
        .expect("required_in_force");
    assert_eq!(in_force.len(), 1, "{in_force:?}");
    assert_eq!(in_force[0]["id"], "003-decision-documentation-requirement");
    assert_eq!(in_force[0]["packs"], serde_json::json!(["org-present"]));
    assert!(payload.get("required_not_in_force").is_none());

    // The human cell carries the same facts, and the absent-key text states the
    // rule the reference states for a non-`skill` kind.
    let listing =
        sk_cli::compat::charter::Listing::load(absent.path(), false, false).expect("listing");
    let directive = listing
        .rows
        .iter()
        .find(|row| row.kind == "directive")
        .expect("directive row");
    assert_eq!(
        directive.activated_text,
        "(All built-ins — no explicit activation)\n\
         003-decision-documentation-requirement (DIRECTIVE_003) — in force: required by 'org-present'"
    );
}

#[test]
fn an_invalid_required_floor_is_reported_not_fatal() {
    // A present allow list reads the floor leniently: a malformed
    // `required_directives` contributes no omission and the listing succeeds.
    let lenient = fixture(
        "activated_directives:\n- 001-architectural-integrity-standard\n",
        "schema_version: 2\nrequired_directives: not-a-list\n",
        false,
    );
    let payload = charter::list_payload(lenient.path(), false, false).expect("lenient list");
    assert!(payload.get("required_not_in_force").is_none());
    // A malformed file still refuses the *strict* per-kind read other absent-key
    // kinds use, so the directives floor is the one this posture does not report
    // unreadable (its allow list is present).
    let unreadable = payload.get("required_floor_unreadable");
    assert!(
        unreadable.is_none_or(|value| !value
            .as_array()
            .expect("array")
            .iter()
            .any(|entry| entry == "directives")),
        "{unreadable:?}"
    );

    // The same malformed floor with an ABSENT allow key is read strictly, so it
    // is reported (never "nothing required") and the lower surfaces are told to
    // run doctor — the listing itself still succeeds.
    let strict = fixture(
        NO_ALLOW_KEY,
        "schema_version: 2\nrequired_directives: not-a-list\n",
        false,
    );
    let payload = charter::list_payload(strict.path(), false, false).expect("strict list");
    let unreadable = payload["required_floor_unreadable"]
        .as_array()
        .expect("required_floor_unreadable");
    assert!(
        unreadable.iter().any(|entry| entry == "directives"),
        "{unreadable:?}"
    );
    assert!(payload.get("required_in_force").is_none());
    let listing =
        sk_cli::compat::charter::Listing::load(strict.path(), false, false).expect("listing");
    assert!(
        listing
            .rows
            .iter()
            .find(|row| row.kind == "directive")
            .expect("directive row")
            .activated_text
            .contains("org required floor unreadable — run `spec-kitty doctor charter-packs`")
    );

    // A missing `org-charter.yaml` requires nothing: no marker, no hint.
    let empty = fixture(NO_ALLOW_KEY, "", false);
    fs::remove_file(empty.path().join("org-present/org-charter.yaml")).expect("remove charter");
    let payload = charter::list_payload(empty.path(), false, false).expect("empty list");
    assert!(payload.get("required_in_force").is_none());
    assert!(payload.get("required_not_in_force").is_none());
    assert!(payload.get("required_floor_unreadable").is_none());
}

#[test]
fn an_unfetched_pack_makes_the_strict_floor_read_report_unreadable() {
    // Source's strict per-kind floor read refuses a declared pack whose root is
    // absent, so an absent allow key reports "unreadable" (plus the doctor hint)
    // rather than "nothing required" — while the listing itself still succeeds.
    let temp = fixture(NO_ALLOW_KEY, EMPTY_ORG_CHARTER, true);
    let payload = charter::list_payload(temp.path(), false, false).expect("list");
    let unreadable = payload["required_floor_unreadable"]
        .as_array()
        .expect("required_floor_unreadable");
    assert!(
        unreadable.iter().any(|entry| entry == "directives"),
        "{unreadable:?}"
    );
    let listing =
        sk_cli::compat::charter::Listing::load(temp.path(), false, false).expect("listing");
    assert!(
        listing
            .rows
            .iter()
            .find(|row| row.kind == "directive")
            .expect("directive row")
            .activated_text
            .contains("org required floor unreadable")
    );
}

#[test]
fn a_malformed_mission_type_file_still_loud_fails() {
    let temp = fixture(NO_ALLOW_KEY, EMPTY_ORG_CHARTER, false);
    write(
        &temp.path().join("org-present/mission_types/broken.yaml"),
        "id: 'not a mapping'\n",
    );
    // `list_available_detailed` raises for a malformed mission-type file; the
    // listing reproduces that instead of silently dropping the row.
    let all = charter::list_payload(temp.path(), false, true);
    assert!(all.is_err(), "expected a loud mission-type refusal");

    // Mission types are the one roster kind with a real built-in roster here.
    let show = charter::list_payload(temp.path(), true, false).expect("built-in roster");
    let ids = available_ids(&row(&show, "mission-type")["available"]);
    assert!(ids.iter().any(|id| id == "software-dev"), "{ids:?}");
}

#[test]
fn the_real_binary_matches_the_payload_and_writes_nothing() {
    let temp = fixture(
        "activated_directives:\n- 001-architectural-integrity-standard\n",
        "schema_version: 2\nrequired_directives:\n- 003-decision-documentation-requirement\n",
        false,
    );
    let before = footprint(temp.path());

    let output = run(temp.path(), &["charter", "list", "--json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload = stdout_json(&output);
    assert_eq!(
        payload,
        charter::list_payload(temp.path(), false, false).expect("in-process list")
    );
    assert!(payload["templates"].is_null());

    let all_output = run(temp.path(), &["charter", "list", "--all", "--json"]);
    assert!(all_output.status.success());
    let all = stdout_json(&all_output);
    assert_eq!(
        all,
        charter::list_payload(temp.path(), false, true).expect("in-process --all")
    );
    // Template rows exist and the duplicated mission-qualified id appears once,
    // from the project layer (first supplied root wins).
    let templates = all["templates"].as_array().expect("templates");
    let shared: Vec<&Value> = templates
        .iter()
        .filter(|entry| entry["template_id"] == "shared/same.md")
        .collect();
    assert_eq!(shared.len(), 1, "{shared:?}");
    assert_eq!(shared[0]["tier"], "override");

    let text = run(temp.path(), &["charter", "list"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).expect("stdout is UTF-8");
    assert!(
        text.contains("directive: 001-architectural-integrity-standard"),
        "{text}"
    );
    assert!(
        text.contains(
            "003-decision-documentation-requirement (DIRECTIVE_003) — required (not in force"
        ),
        "{text}"
    );

    // A read-only listing leaves every fixture file byte-identical.
    assert_eq!(
        before,
        footprint(temp.path()),
        "charter list wrote to the project"
    );
}
