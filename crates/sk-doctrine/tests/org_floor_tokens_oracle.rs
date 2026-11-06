//! Source-first probe of the org required floor for **exact tokens**.
//!
//! `org_required_stems` is the reference's real floor API for `--only`: it
//! unions every pack's `required_<kind>` list and maps each token through the
//! identity index to its *activation stem* (the artifact's file stem), keeping
//! an unknown token as written and warning about it.
//!
//! Two facts matter for agent profiles:
//!
//! * a requirement authored as the profile's **declared id** resolves to the
//!   file stem, and one authored as the **file stem** resolves to itself — the
//!   floor is never a scalar re-projection or a case/whitespace guess;
//! * a requirement authored as a **non-scalar declared id** resolves to the file
//!   stem in the reference, because its `IdentityIndex` is keyed on exact Python
//!   strings. Native's org-charter reader is `serde_yaml`-based, so the
//!   surrogate-bearing *charter document* fails closed (the floor is refused,
//!   never silently shrunk) instead of guessing a match. That bound is asserted
//!   here, not papered over.
//!
//! The reference runs first and its floor is compared token-for-token; the
//! surrogate case is a documented fail-closed native boundary, not a match.

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};
use sk_charter::ArtifactKind;
use sk_doctrine::{IdentityIndex, PythonString, org_required_stems, read_org_required_tokens};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create directory");
    fs::write(path, text).expect("write fixture");
}

fn scalar_stems(stems: Vec<PythonString>) -> Value {
    json!(
        stems
            .into_iter()
            .map(|stem| stem.to_scalar_string().expect("scalar stem"))
            .collect::<Vec<_>>()
    )
}

struct Fixture {
    _temp: tempfile::TempDir,
    packs: PathBuf,
    project: PathBuf,
    org: PathBuf,
}

impl Fixture {
    fn new(required: &str) -> Self {
        let temp = tempfile::tempdir().expect("temporary root");
        let packs = temp.path().join("packs");
        let project = temp.path().join("project");
        let org = packs.join("org");
        copy_tree(
            &support::repository_root().join("packs/built-in"),
            &packs.join("built-in"),
        );
        fs::create_dir_all(&project).expect("project");
        write(
            &org.join("pack.yaml"),
            "pack_id: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: acme\npack_level: org\n",
        );
        write(
            &org.join("drg/fragment.yaml"),
            "pack_name: acme\nnodes: []\nedges: []\n",
        );
        write(
            &org.join("org-charter.yaml"),
            &format!("schema_version: 2\norg_name: Acme\nrequired_agent_profiles: {required}\n"),
        );
        write(
            &org.join("agent_profiles/org-deploy.agent.yaml"),
            "profile-id: deploy-agent\nname: Deploy Agent\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
        );
        write(
            &org.join("agent_profiles/org-audit.agent.yaml"),
            "profile-id: audit-agent\nname: Audit Agent\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
        );
        write(
            &project.join(".kittify/config.yaml"),
            &format!(
                "charter_packs:\n  org:\n    packs:\n      - name: acme\n        local_path: {}\n",
                org.display()
            ),
        );
        Self {
            _temp: temp,
            packs,
            project,
            org,
        }
    }

    fn env(&self) -> Vec<(String, String)> {
        let mut env = support::oracle_env(&self.packs);
        env.push(("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()));
        env.push((
            "HOME".to_owned(),
            self.project.join("home").display().to_string(),
        ));
        env.push(("PYTHONDONTWRITEBYTECODE".to_owned(), "1".to_owned()));
        env
    }

    fn source_stems(&self) -> Value {
        let script = r#"
import json
from charter.activation.org_charter import org_required_stems
warnings = []
stems = org_required_stems(__import__("pathlib").Path(__import__("os").environ["SK_PROJECT"]), "agent_profiles", warnings)
print(json.dumps({"stems": stems, "warnings": warnings}))
"#;
        let output = Command::new(support::python())
            .arg("-c")
            .arg(script)
            .current_dir(&self.project)
            .env_clear()
            .envs(self.env())
            .env("SK_PROJECT", self.project.display().to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run source floor");
        assert!(
            output.status.success(),
            "source floor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("source floor JSON")
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("destination");
    for entry in fs::read_dir(source).expect("read source") {
        let entry = entry.expect("entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy");
        }
    }
}

#[test]
fn org_required_floor_maps_exact_tokens_to_activation_stems() {
    let fixture = Fixture::new("[deploy-agent, org-audit, missing-token]");
    let source = fixture.source_stems();
    assert_eq!(
        source["stems"],
        json!(["org-deploy", "org-audit", "missing-token"]),
        "the reference resolves a declared id and a file stem to stems, keeps an unknown token"
    );

    let identity = IdentityIndex::from_roots(
        &support::repository_root().join("packs/built-in"),
        std::slice::from_ref(&fixture.org),
        None,
    );
    let mut warnings = Vec::new();
    let native = org_required_stems(
        &fixture.project,
        ArtifactKind::AgentProfile,
        &identity,
        &mut warnings,
    )
    .expect("native floor");
    assert_eq!(
        scalar_stems(native),
        source["stems"],
        "native floor stems must match the reference exactly"
    );
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("missing-token")),
        "an unresolved requirement is kept and named: {warnings:?}"
    );
}

#[test]
fn non_scalar_requirement_token_resolves_like_the_reference() {
    // The reference's `IdentityIndex` is keyed on exact Python `str`, so a
    // requirement authored as `"author\ud800"` resolves to the profile whose
    // declared id is the identical code-point sequence. Native carries the same
    // exact token through the same transport/construction phase, so this is a
    // real Source-equality regression — not a fail-closed boundary.
    let fixture = Fixture::new("[\"author\\ud800\"]");
    write(
        &fixture.org.join("agent_profiles/surrogate-file.agent.yaml"),
        "profile-id: \"author\\ud800\"\nname: Surrogate\npurpose: p\nroles: [reviewer]\nspecialization: {primary-focus: x}\n",
    );
    // The reference runs first; its floor is the expectation.
    let source = fixture.source_stems();
    assert_eq!(
        source["stems"],
        json!(["surrogate-file"]),
        "the reference's exact IdentityIndex resolves the id to the file stem"
    );

    let identity = IdentityIndex::from_roots(
        &support::repository_root().join("packs/built-in"),
        std::slice::from_ref(&fixture.org),
        None,
    );
    let mut warnings = Vec::new();
    let native = org_required_stems(
        &fixture.project,
        ArtifactKind::AgentProfile,
        &identity,
        &mut warnings,
    )
    .expect("native floor must resolve the exact token");
    assert_eq!(
        scalar_stems(native),
        source["stems"],
        "native must resolve the non-scalar declared id exactly like the reference"
    );
    assert!(
        warnings.is_empty(),
        "the token resolves, so nothing is warned: {warnings:?}"
    );

    // The authored token is carried exactly (never projected to an escape or a
    // replacement character).
    let tokens = read_org_required_tokens(&fixture.project, ArtifactKind::AgentProfile, false)
        .expect("scalar view");
    assert!(
        tokens.is_empty(),
        "the scalar capability view omits the non-scalar token instead of inventing one: {tokens:?}"
    );
    let exact = sk_doctrine::read_org_required_tokens_exact(
        &fixture.project,
        ArtifactKind::AgentProfile,
        false,
    )
    .expect("exact tokens");
    assert_eq!(
        exact.first().map(|token| token.codepoints().to_vec()),
        Some(vec![0x61, 0x75, 0x74, 0x68, 0x6f, 0x72, 0xd800]),
        "the authored token keeps its exact code points"
    );
}

#[test]
fn unknown_non_scalar_requirement_token_is_kept_exactly() {
    // An unknown requirement is kept as written and warned about, exactly like
    // the reference (`_warn_unresolved`) — including a token with no scalar
    // projection.
    let fixture = Fixture::new("[\"ghost\\ud800\"]");
    let identity = IdentityIndex::from_roots(
        &support::repository_root().join("packs/built-in"),
        std::slice::from_ref(&fixture.org),
        None,
    );
    let mut warnings = Vec::new();
    let native = org_required_stems(
        &fixture.project,
        ArtifactKind::AgentProfile,
        &identity,
        &mut warnings,
    )
    .expect("native floor");
    assert_eq!(native.len(), 1);
    assert_eq!(
        native[0].codepoints(),
        &[0x67, 0x68, 0x6f, 0x73, 0x74, 0xd800],
        "an unknown token is kept as written"
    );
    assert!(
        warnings.iter().any(|warning| warning.contains("ghost")),
        "an unresolved requirement is named: {warnings:?}"
    );
}
