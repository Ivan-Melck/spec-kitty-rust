//! CF30 focused smoke: `template:<mission>/<name>` through the canonical
//! `sk-doctrine` include surface.
//!
//! These drive the real `render_include` with real temporary project trees and a
//! real `sk-charter` org-pack declaration, so the override/legacy/org tiers and
//! the packaged default are exercised end to end. The access authority is built from
//! an EMPTY pack root and no language signal: a template selector must not need any
//! artifact document, identity scan, language inference or profile load.
//!
//! Not a deep differential campaign. Deferred proof: `global_mission`/`global`
//! tiers through the real `~/.kittify` home (covered with injected roots in
//! `sk-charter`), non-UTF-8 template bytes, concurrent pack mutation, and the
//! activation deny gate on a selector (owned by `cf32_effective_include.rs`).

use std::fs;
use std::path::Path;

use sk_doctrine::OfferingRoots;
use sk_doctrine::context_include::{
    IncludeAccess, IncludeInputs, parse_include_selector, render_include,
};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// A real project tree with a declared org pack and no artifact content at all.
struct Fixture {
    temp: tempfile::TempDir,
    access: IncludeAccess,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        // An EMPTY built-in pack root: no artifact a selector could read.
        let packs = root.join("packs");
        fs::create_dir_all(&packs).expect("packs");
        let project = root.join("project");
        write(
            &project.join(".kittify/charter-packs/.kittify/config.yaml"),
            "charter_packs:\n  org:\n    packs:\n      - name: fixture-org\n        local_path: org-one\n",
        );
        write(
            &project.join(".kittify/charter-packs/.kittify/overrides/missions/software-dev/templates/override.md"),
            "# Override template\n",
        );
        write(
            &project.join(".kittify/charter-packs/.kittify/templates/legacy.md"),
            "Legacy template body\n\n",
        );
        write(
            &project.join(".kittify/charter-packs/org-one/missions/software-dev/templates/org.md"),
            "Org template body\n",
        );
        write(
            &project.join(
                ".kittify/charter-packs/org-one/missions/software-dev/templates/nested/dir/plan.md",
            ),
            "Nested org template\n",
        );
        let access = IncludeAccess::with_roots(
            root,
            OfferingRoots {
                built_in: Some(packs),
                org_roots: Vec::new(),
                project: Some(project),
            },
        )
        .expect("fixture access");
        Self { temp, access }
    }

    fn project(&self) -> std::path::PathBuf {
        self.temp.path().join("project")
    }

    fn render(&self, selector: &str) -> Result<String, sk_doctrine::context_include::IncludeError> {
        let parsed = parse_include_selector(selector).expect("selector parses");
        let inputs = IncludeInputs {
            project_root: &self.project(),
            access: &self.access,
        };
        render_include(&inputs, &parsed, selector)
    }
}

#[test]
fn template_include_resolves_project_and_org_tiers_with_no_artifact_content() {
    let fixture = Fixture::new();

    assert_eq!(
        fixture
            .render("template:software-dev/override.md")
            .expect("override template"),
        "Template software-dev/override.md (tier: override):\n# Override template"
    );
    assert_eq!(
        fixture
            .render("template:software-dev/legacy.md")
            .expect("legacy template"),
        "Template software-dev/legacy.md (tier: legacy):\nLegacy template body"
    );
    // The org root comes from the real `.kittify/config.yaml` declaration read by
    // `sk-charter`, not from a fixture shortcut.
    assert_eq!(
        fixture
            .render("template:software-dev/org.md")
            .expect("org template"),
        "Template software-dev/org.md (tier: org):\nOrg template body"
    );
    // A nested name keeps its separators through the same tier probe.
    assert_eq!(
        fixture
            .render("template:software-dev/nested/dir/plan.md")
            .expect("nested org template"),
        "Template software-dev/nested/dir/plan.md (tier: org):\nNested org template"
    );
}

#[test]
fn template_include_does_not_need_a_readable_artifact_tree() {
    let fixture = Fixture::new();
    // A syntax-broken artifact file in the same pack root cannot change a template
    // answer: the template branch never builds the identity index's artifact tables.
    write(
        &fixture.temp.path().join("packs/tactics/broken.tactic.yaml"),
        "id: broken-tactic\nsteps: [unclosed\n",
    );
    let text = fixture
        .render("template:software-dev/override.md")
        .expect("template still renders");
    assert!(text.starts_with("Template software-dev/override.md (tier: override):\n"));
    // The same broken artifact is simply not loaded for an artifact selector.
    assert_eq!(
        fixture
            .render("tactic:broken-tactic")
            .unwrap_err()
            .to_string(),
        "No tactic found for selector 'tactic:broken-tactic'."
    );
}

#[test]
fn a_template_on_disk_never_becomes_an_artifact_probe_match() {
    let fixture = Fixture::new();
    // `override.md` exists as a template; templates are not charter-activatable, so the
    // best-effort `artifact:` probe still reports nothing rather than rendering it.
    assert_eq!(
        fixture.render("artifact:override").unwrap_err().to_string(),
        "No artifact found for selector 'artifact:override'."
    );
}

#[test]
fn template_refusals_are_reference_errors_without_a_native_gap_code() {
    let fixture = Fixture::new();
    let missing = fixture
        .render("template:software-dev/no-such-template.md")
        .unwrap_err();
    assert_eq!(missing.code(), None);
    assert!(!missing.is_pending());
    assert_eq!(
        missing.to_string(),
        "No template found for selector 'template:software-dev/no-such-template.md'."
    );

    let malformed = fixture.render("template:software-dev").unwrap_err();
    assert_eq!(malformed.code(), None);
    assert_eq!(
        malformed.to_string(),
        "Malformed template selector 'template:software-dev': template_id 'software-dev' is not \
         of the form '<mission>/<name>'"
    );

    // The package default still resolves for a real built-in template, proving the
    // sixth tier is wired and not a pending refusal.
    let packaged = fixture
        .render("template:software-dev/spec-template.md")
        .expect("packaged template");
    assert!(
        packaged.starts_with("Template software-dev/spec-template.md (tier: package_default):\n"),
        "{packaged}"
    );
}
