//! CF30 focused smoke: the NEW canonical template catalog and the ONE six-tier
//! template resolver in `sk-charter`.
//!
//! These exercise the real module against real temporary trees. The four
//! environment-shaped tiers (`org`, `global_mission`, `global`, and the packaged
//! default) are supplied through the public [`TemplateRoots`] seam as data, which
//! is exactly what the reference's C-008 contract allows; the packaged diagrams
//! below additionally use the live kernel missions root so the real installed
//! default is exercised at least once.
//!
//! Not a deep differential campaign. Deliberately deferred proof: exact Python
//! `repr`/`OSError` text for every path shape, non-UTF-8 path *resolution*,
//! symlink-retarget races, Windows/UNC path semantics, a real multi-gigabyte
//! template, and DRG node minting (`charter list --all`) which belongs to the
//! graph authority in `sk-doctrine`.

use std::fs;
use std::path::{Path, PathBuf};

use sk_charter::template_catalog::{
    COMMAND_SUBDIR, CONTENT_SUBDIR, LiveTemplateRoots, TemplateError, TemplateRef, TemplateRoots,
    TemplateTier, TierRoot, discover_templates, render_template_include,
    resolve_command_with_roots, resolve_template, resolve_template_by_id,
    resolve_template_by_id_with_roots, resolve_template_with_roots, template_id_for, template_urn,
};

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture parent");
    }
    fs::write(path, contents).expect("fixture write");
}

/// Injected root data for the env-shaped tiers.
struct Roots {
    home: Option<PathBuf>,
    package: Result<PathBuf, String>,
    org: Vec<PathBuf>,
}

impl Roots {
    fn new(package: impl Into<PathBuf>) -> Self {
        Self {
            home: None,
            package: Ok(package.into()),
            org: Vec::new(),
        }
    }

    fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home = Some(home.into());
        self
    }

    fn with_org(mut self, root: impl Into<PathBuf>) -> Self {
        self.org.push(root.into());
        self
    }

    fn without_package() -> Self {
        Self {
            home: None,
            package: Err("No pack root found for tier 'built-in'".to_owned()),
            org: Vec::new(),
        }
    }
}

impl TemplateRoots for Roots {
    fn kittify_home(&self) -> Option<PathBuf> {
        self.home.clone()
    }

    fn package_missions_root(&self) -> Result<PathBuf, TemplateError> {
        self.package
            .clone()
            .map_err(|detail| TemplateError::PackageRoot { detail })
    }

    fn org_roots(&self, _project_dir: &Path) -> Result<Vec<PathBuf>, TemplateError> {
        Ok(self.org.clone())
    }
}

/// A roots provider whose org layer is broken, to prove org lookup is lazy.
struct BrokenOrgRoots {
    package: PathBuf,
}

impl TemplateRoots for BrokenOrgRoots {
    fn kittify_home(&self) -> Option<PathBuf> {
        None
    }

    fn package_missions_root(&self) -> Result<PathBuf, TemplateError> {
        Ok(self.package.clone())
    }

    fn org_roots(&self, _project_dir: &Path) -> Result<Vec<PathBuf>, TemplateError> {
        Err(TemplateError::OrgConfig {
            detail: "unresolved path token ${UNSET_PACK_ROOT}".to_owned(),
        })
    }
}

/// One project plus every tier root the resolver can reach, with real files.
struct SixTierFixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    home: PathBuf,
    org_first: PathBuf,
    org_second: PathBuf,
    package: PathBuf,
}

impl SixTierFixture {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().expect("temp");
        let root = temp.path();
        let project = root.join("project");
        let home = root.join("home");
        let org_first = root.join("org-first");
        let org_second = root.join("org-second");
        let package = root.join("package/packs/built-in/missions");

        write(
            &project.join(".kittify/overrides/missions/software-dev/templates/spec-template.md"),
            "mission override body\n",
        );
        write(
            &project
                .join(".kittify/overrides/missions/software-dev/command-templates/implement.md"),
            "mission override command\n",
        );
        write(
            &project.join(".kittify/overrides/templates/plan-template.md"),
            "global override body\n",
        );
        write(
            &project.join(".kittify/templates/tasks-template.md"),
            "legacy body\n",
        );
        write(
            &org_first.join("missions/software-dev/templates/org-only.md"),
            "org first\n",
        );
        write(
            &org_second.join("missions/software-dev/templates/org-only.md"),
            "org second\n",
        );
        write(
            &home.join("missions/software-dev/templates/global-mission.md"),
            "global mission body\n",
        );
        write(&home.join("templates/global.md"), "global body\n");
        write(
            &package.join("software-dev/templates/package.md"),
            "package body\n",
        );
        write(
            &package.join("software-dev/templates/nested/dir/plan.md"),
            "nested package body\n",
        );
        write(
            &package.join("software-dev/command-templates/step.md"),
            "package command body\n",
        );
        write(
            &package.join("mission-steps/software-dev/prompted/prompt.md"),
            "mission-step prompt body\n",
        );

        Self {
            _temp: temp,
            project,
            home,
            org_first,
            org_second,
            package,
        }
    }

    fn roots(&self) -> Roots {
        Roots::new(self.package.clone())
            .with_home(self.home.clone())
            .with_org(self.org_first.clone())
            .with_org(self.org_second.clone())
    }

    fn resolve(
        &self,
        name: &str,
    ) -> Result<sk_charter::template_catalog::ResolutionResult, TemplateError> {
        resolve_template_with_roots(name, &self.project, "software-dev", &self.roots())
    }
}

#[test]
fn every_tier_wins_in_the_reference_precedence_order() {
    let fixture = SixTierFixture::new();
    let roots = fixture.roots();

    // Tier 1a — mission-scoped override.
    let result = fixture.resolve("spec-template.md").expect("override");
    assert_eq!(result.tier, TemplateTier::Override);
    assert!(
        result
            .path
            .ends_with(".kittify/overrides/missions/software-dev/templates/spec-template.md")
    );

    // Tier 1b — the backward-compatible global override shape.
    let result = fixture
        .resolve("plan-template.md")
        .expect("global override");
    assert_eq!(result.tier, TemplateTier::Override);
    assert!(
        result
            .path
            .ends_with(".kittify/overrides/templates/plan-template.md")
    );

    // Tier 2 — legacy.
    let result = fixture.resolve("tasks-template.md").expect("legacy");
    assert_eq!(result.tier, TemplateTier::Legacy);

    // Tier 3 — org, first declared match wins.
    let result = fixture.resolve("org-only.md").expect("org");
    assert_eq!(result.tier, TemplateTier::Org);
    assert_eq!(
        result.path,
        fixture
            .org_first
            .join("missions/software-dev/templates/org-only.md")
    );

    // Tier 4 — global mission.
    let result = fixture
        .resolve("global-mission.md")
        .expect("global mission");
    assert_eq!(result.tier, TemplateTier::GlobalMission);

    // Tier 5 — global.
    let result = fixture.resolve("global.md").expect("global");
    assert_eq!(result.tier, TemplateTier::Global);

    // Tier 6 — packaged default, including a nested name.
    let result = fixture.resolve("package.md").expect("package");
    assert_eq!(result.tier, TemplateTier::PackageDefault);
    let result = fixture
        .resolve("nested/dir/plan.md")
        .expect("nested package");
    assert_eq!(result.tier, TemplateTier::PackageDefault);

    // Command templates use the same six tiers.
    let result =
        resolve_command_with_roots("implement.md", &fixture.project, "software-dev", &roots)
            .expect("command override");
    assert_eq!(result.tier, TemplateTier::Override);

    // The tier spellings are the reference's enum values.
    assert_eq!(TemplateTier::GlobalMission.as_str(), "global_mission");
    assert_eq!(TemplateTier::PackageDefault.as_str(), "package_default");
    assert_eq!(
        TemplateTier::ORDER.map(TemplateTier::as_str),
        [
            "override",
            "legacy",
            "org",
            "global_mission",
            "global",
            "package_default"
        ]
    );
}

#[test]
fn command_templates_also_fall_back_to_the_canonical_mission_step_prompt() {
    let fixture = SixTierFixture::new();
    let roots = fixture.roots();
    let result =
        resolve_command_with_roots("prompted.md", &fixture.project, "software-dev", &roots)
            .expect("mission-step prompt");
    assert_eq!(result.tier, TemplateTier::PackageDefault);
    assert!(
        result
            .path
            .ends_with("mission-steps/software-dev/prompted/prompt.md")
    );

    // `<name>.md` is stripped exactly once before the step-prompt join.
    let result = resolve_command_with_roots("prompted", &fixture.project, "software-dev", &roots)
        .expect("mission-step prompt without the md suffix");
    assert!(
        result
            .path
            .ends_with("mission-steps/software-dev/prompted/prompt.md")
    );
}

#[test]
fn org_configuration_is_only_read_after_the_project_tiers_miss() {
    let fixture = SixTierFixture::new();
    let broken = BrokenOrgRoots {
        package: fixture.package.clone(),
    };

    // An override hit never touches the broken org registry.
    let result = resolve_template_with_roots(
        "spec-template.md",
        &fixture.project,
        "software-dev",
        &broken,
    )
    .expect("override wins before org");
    assert_eq!(result.tier, TemplateTier::Override);

    // The legacy tier is also above org.
    let result = resolve_template_with_roots(
        "tasks-template.md",
        &fixture.project,
        "software-dev",
        &broken,
    )
    .expect("legacy wins before org");
    assert_eq!(result.tier, TemplateTier::Legacy);

    // A miss reaches org, where the reference's ValueError family surfaces.
    let error =
        resolve_template_with_roots("org-only.md", &fixture.project, "software-dev", &broken)
            .expect_err("org is consulted lazily and fails closed");
    assert!(
        matches!(error, TemplateError::OrgConfig { .. }),
        "{error:?}"
    );
    assert!(error.is_reference_value_error());
}

#[test]
fn an_unavailable_home_skips_both_global_tiers() {
    let fixture = SixTierFixture::new();
    let roots = Roots::new(fixture.package.clone()).with_org(fixture.org_first.clone());
    let result =
        resolve_template_with_roots("package.md", &fixture.project, "software-dev", &roots)
            .expect("package still resolves without a home");
    assert_eq!(result.tier, TemplateTier::PackageDefault);

    let result =
        resolve_template_with_roots("org-only.md", &fixture.project, "software-dev", &roots)
            .expect("org still resolves without a home");
    assert_eq!(result.tier, TemplateTier::Org);
}

#[test]
fn miss_malformed_id_and_missing_package_root_are_distinct_failures() {
    let fixture = SixTierFixture::new();

    let error = fixture.resolve("nowhere.md").expect_err("miss");
    assert_eq!(
        error.to_string(),
        format!(
            "Asset 'nowhere.md' not found in any resolution tier (subdir='templates', \
             mission='software-dev', project={})",
            fixture.project.display()
        )
    );
    assert!(!error.is_reference_value_error());

    // A missing installed missions root is the reference's uncaught exception
    // class, not "not found".
    let error = resolve_template_with_roots(
        "package.md",
        &fixture.project,
        "software-dev",
        &Roots::without_package(),
    )
    .expect_err("package root");
    assert!(
        matches!(error, TemplateError::PackageRoot { .. }),
        "{error:?}"
    );
    assert!(!error.is_reference_value_error());

    for malformed in ["no-slash", "/name-only", "mission-only/", ""] {
        let error = resolve_template_by_id(malformed, &[]).expect_err("malformed id");
        assert_eq!(
            error.to_string(),
            format!(
                "template_id {} is not of the form '<mission>/<name>'",
                sk_kernel::python_repr(malformed)
            ),
            "{malformed}"
        );
        assert!(error.is_reference_value_error());
    }

    // The id is split exactly once: a nested name keeps its separators.
    let result = resolve_template_by_id_with_roots(
        "software-dev/nested/dir/plan.md",
        &[
            TierRoot::new(TemplateTier::PackageDefault, fixture.package.clone())
                .with_project_dir(fixture.project.clone()),
        ],
        &fixture.roots(),
    )
    .expect("nested id");
    assert_eq!(result.tier, TemplateTier::PackageDefault);
    assert!(
        result
            .path
            .ends_with("software-dev/templates/nested/dir/plan.md")
    );
    // The project directory came from the supplied tier root.
    assert_eq!(
        resolve_template_by_id_with_roots(
            "software-dev/spec-template.md",
            &[
                TierRoot::new(TemplateTier::PackageDefault, fixture.package.clone())
                    .with_project_dir(fixture.project.clone())
            ],
            &fixture.roots(),
        )
        .expect("override via the tier root project dir")
        .tier,
        TemplateTier::Override
    );
}

#[test]
fn the_live_packaged_default_resolves_a_real_built_in_template() {
    // Real kernel authorities: the installed `packs/built-in/missions` root and
    // the real `~/.kittify` availability fact (absent in the isolated test home).
    let temp = tempfile::TempDir::new().expect("temp");
    let result = resolve_template("spec-template.md", temp.path(), "software-dev")
        .expect("built-in spec template");
    assert_eq!(result.tier, TemplateTier::PackageDefault);
    assert!(
        result
            .path
            .ends_with("missions/software-dev/templates/spec-template.md")
    );
    let command =
        sk_charter::template_catalog::resolve_command("implement", temp.path(), "software-dev")
            .expect("built-in implement step prompt");
    assert_eq!(command.tier, TemplateTier::PackageDefault);
    assert!(
        command
            .path
            .ends_with("mission-steps/software-dev/implement/prompt.md")
    );

    // And the include renders that real template with the reference heading.
    let include = render_template_include(
        temp.path(),
        "software-dev/spec-template.md",
        "template:software-dev/spec-template.md",
    )
    .expect("live include");
    assert_eq!(include.tier, TemplateTier::PackageDefault);
    assert!(
        include
            .text
            .starts_with("Template software-dev/spec-template.md (tier: package_default):\n"),
        "{}",
        include.text
    );
    assert!(!include.text.ends_with('\n'));
    assert!(include.text.len() > 100);
}

#[test]
fn discovery_walks_both_subdirs_recursively_excludes_readme_and_dedupes_tiers() {
    let temp = tempfile::TempDir::new().expect("temp");
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    write(&first.join("missions/alpha/templates/one.md"), "one\n");
    write(
        &first.join("missions/alpha/templates/README.md"),
        "not a template\n",
    );
    write(
        &first.join("missions/alpha/templates/nested/deep/two.md"),
        "two\n",
    );
    write(
        &first.join("missions/alpha/command-templates/three.md"),
        "three\n",
    );
    write(
        &second.join("missions/alpha/templates/one.md"),
        "shadowed\n",
    );
    write(&second.join("missions/beta/templates/one.md"), "beta\n");
    // A mission directory without template subdirs contributes nothing.
    write(
        &second.join("missions/beta/mission.yaml"),
        "mission_type: software-dev\n",
    );

    let refs = discover_templates(&[
        TierRoot::new(TemplateTier::Override, first.join("missions")),
        TierRoot::new(TemplateTier::Legacy, second.join("missions")),
        TierRoot::new(TemplateTier::PackageDefault, temp.path().join("missing")),
    ])
    .expect("discovery");

    let ids: Vec<&str> = refs.iter().map(|item| item.template_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "alpha/one.md",
            "alpha/three.md",
            "alpha/two.md",
            "beta/one.md"
        ],
        "{ids:?}"
    );
    // Discovery identity is `mission + basename` (the reference uses `path.name`), so a
    // deeply nested file is still discovered and keeps its nested path.
    let nested = refs
        .iter()
        .find(|item| item.template_id == "alpha/two.md")
        .expect("nested template");
    assert_eq!(nested.name, "two.md");
    assert_eq!(nested.tier, TemplateTier::Override);
    assert!(
        nested.path.ends_with("templates/nested/deep/two.md"),
        "{}",
        nested.path.display()
    );
    let alpha_one = refs
        .iter()
        .find(|item| item.template_id == "alpha/one.md")
        .expect("alpha/one.md");
    let TemplateRef {
        mission,
        name,
        tier,
        path,
        ..
    } = alpha_one;
    assert_eq!(mission, "alpha");
    assert_eq!(name, "one.md");
    assert_eq!(*tier, TemplateTier::Override, "first supplied tier wins");
    assert_eq!(path, &first.join("missions/alpha/templates/one.md"));
    assert_eq!(refs[3].tier, TemplateTier::Legacy);
    assert!(
        refs.iter().all(|item| item.name != "README.md"),
        "README.md is not a template"
    );

    // Identity helpers are the reference's spellings.
    assert_eq!(template_id_for("alpha", "one.md"), "alpha/one.md");
    assert_eq!(template_urn("alpha/one.md"), "template:alpha/one.md");
    // Constants stay the reference's directory names.
    assert_eq!(CONTENT_SUBDIR, "templates");
    assert_eq!(COMMAND_SUBDIR, "command-templates");
}

#[cfg(unix)]
#[test]
fn a_non_utf8_directory_entry_is_refused_instead_of_lossily_named() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let temp = tempfile::TempDir::new().expect("temp");
    let root = temp.path().join("root");
    let mission = root
        .join("missions")
        .join(OsString::from_vec(b"mis\xffsion".to_vec()));
    fs::create_dir_all(mission.join("templates")).expect("mission");
    fs::write(mission.join("templates/one.md"), "one\n").expect("template");

    let error = discover_templates(&[TierRoot::new(TemplateTier::Override, root.join("missions"))])
        .expect_err("non-UTF-8 mission name");
    assert!(
        matches!(error, TemplateError::NonUtf8Name { .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("not valid UTF-8"), "{error}");
}

#[test]
fn a_missing_missions_root_contributes_no_references() {
    let temp = tempfile::TempDir::new().expect("temp");
    let refs = discover_templates(&[TierRoot::new(
        TemplateTier::Org,
        temp.path().join("does-not-exist"),
    )])
    .expect("missing root is empty");
    assert!(refs.is_empty());
}

#[test]
fn the_include_heading_strips_trailing_newlines_and_translates_crlf() {
    let temp = tempfile::TempDir::new().expect("temp");
    let project = temp.path();
    write(
        &project.join(".kittify/overrides/missions/software-dev/templates/crlf.md"),
        "Line one\r\nLine two\n\n\n",
    );
    let include = render_template_include(
        project,
        "software-dev/crlf.md",
        "template:software-dev/crlf.md",
    )
    .expect("override include");
    assert_eq!(include.tier, TemplateTier::Override);
    assert_eq!(
        include.text,
        "Template software-dev/crlf.md (tier: override):\nLine one\nLine two"
    );

    // A UTF-8 BOM stays part of the text: the reference reads with encoding
    // "utf-8", not "utf-8-sig".
    write(
        &project.join(".kittify/overrides/missions/software-dev/templates/bom.md"),
        "\u{feff}bom body\n",
    );
    let include = render_template_include(
        project,
        "software-dev/bom.md",
        "template:software-dev/bom.md",
    )
    .expect("bom include");
    assert_eq!(
        include.text,
        "Template software-dev/bom.md (tier: override):\n\u{feff}bom body"
    );
}

#[test]
fn the_include_refusal_family_matches_the_reference_wording() {
    let temp = tempfile::TempDir::new().expect("temp");
    let project = temp.path();

    let error = render_template_include(
        project,
        "software-dev/nowhere.md",
        "template:software-dev/nowhere.md",
    )
    .expect_err("miss");
    assert_eq!(
        error.to_string(),
        "No template found for selector 'template:software-dev/nowhere.md'."
    );

    let error =
        render_template_include(project, "nowhere", "template:nowhere").expect_err("malformed");
    assert_eq!(
        error.to_string(),
        "Malformed template selector 'template:nowhere': template_id 'nowhere' is not of the \
         form '<mission>/<name>'"
    );
}

#[cfg(unix)]
#[test]
fn an_unreadable_resolved_template_is_a_read_failure() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::TempDir::new().expect("temp");
    let project = temp.path();
    let path = project.join(".kittify/overrides/missions/software-dev/templates/locked.md");
    write(&path, "locked body\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");

    let outcome = render_template_include(
        project,
        "software-dev/locked.md",
        "template:software-dev/locked.md",
    );
    let restore = fs::set_permissions(&path, fs::Permissions::from_mode(0o644));
    assert!(restore.is_ok(), "restore the fixture mode");

    match outcome {
        Ok(include) => {
            // A privileged runner can read a 0o000 file; that is a harness fact,
            // not a product pass. The assertion below still holds.
            assert!(
                include
                    .text
                    .starts_with("Template software-dev/locked.md (tier: override):")
            );
        }
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.starts_with(
                    "Template for selector 'template:software-dev/locked.md' could not be read: "
                ),
                "{message}"
            );
            assert!(message.contains("locked.md"), "{message}");
        }
    }
}

#[test]
fn the_live_roots_provider_reads_the_kernel_authorities() {
    // `LiveTemplateRoots` must be the real default: a home directory fact from
    // the kernel (present or absent, never invented) and a real packaged root.
    let roots = LiveTemplateRoots;
    assert!(roots.package_missions_root().is_ok());
    // No assertion on `kittify_home()`: it is an availability fact that depends
    // on the isolated test home, and both outcomes are supported.
    let _ = roots.kittify_home();
    // A project without `.kittify/config.yaml` has no org layer.
    let temp = tempfile::TempDir::new().expect("temp");
    assert!(
        roots
            .org_roots(temp.path())
            .expect("lenient registry")
            .is_empty(),
        "an absent registry declares no org packs"
    );
}
