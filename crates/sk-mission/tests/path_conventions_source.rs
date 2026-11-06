//! Source-first, exact proof of the path-convention / waiver / project-override /
//! definition-resolution authority.
//!
//! Same protocol as `definition_source.rs`: the parent process builds fixtures in
//! a fresh temporary root and runs the pinned Source there; the Native leg is a
//! re-executed separate process that asserts its own sandbox and then compares the
//! Native service against those receipts field by field, including the exact
//! rendered texts and Source's `warnings.warn` channel.

#[path = "../../sk-kernel/tests/support/oracle.rs"]
mod oracle;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use sk_kernel::PythonString;
use sk_mission::DefinitionProviders;
use sk_mission::RegisteredMissionType;
use sk_mission::ResolutionTier;
use sk_mission::definition::{
    CheckoutProviders, MissionDefinition, OrderedMap, PyYamlSafe, RuamelSafe, SafeDocumentDecoder,
    load_mission_definition, resolve_mission_definition,
    resolve_mission_definition_for_mission_dir,
};
use sk_mission::path_conventions::{
    EvaluatePathConventionsInputs, PathConventionError, PathValidationInputs,
    ProjectPathConventions, artifact_tokens_for, build_warnings, evaluate_path_conventions,
    get_deliverables_path, load_project_path_conventions, read_contracts_waiver_from_meta,
    validate_mission_paths,
};

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

fn child(root: &Path, repo: &Path, program: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .current_dir(root)
        .env("PYTHONPATH", repo.join("src"))
        .env("SPEC_KITTY_PACKS_ROOT", repo.join("packs"))
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONUTF8", "1")
        .env("LC_ALL", "C.UTF-8");
    for (key, name) in roots() {
        command.env(key, root.join(name));
    }
    command
}

fn ps(text: &str) -> PythonString {
    PythonString::from_scalars(text)
}

fn u(text: &PythonString) -> serde_json::Value {
    serde_json::json!({ "u": text.codepoints() })
}

fn u_str(text: &str) -> serde_json::Value {
    u(&ps(text))
}

fn units_list(values: &[PythonString]) -> serde_json::Value {
    serde_json::Value::Array(values.iter().map(u).collect())
}

fn units_pairs(map: &OrderedMap<PythonString>) -> serde_json::Value {
    serde_json::Value::Array(
        map.iter()
            .map(|(key, value)| serde_json::json!([u(key), u(value)]))
            .collect(),
    )
}

fn receipt_string(value: &serde_json::Value) -> PythonString {
    let units: Vec<u32> = serde_json::from_value(value["u"].clone()).expect("code-point list");
    PythonString::from_codepoints(&units).expect("code points")
}

fn receipt_strings(value: &serde_json::Value) -> Vec<PythonString> {
    value
        .as_array()
        .expect("string list")
        .iter()
        .map(receipt_string)
        .collect()
}

fn declared_map(pairs: &[(String, String)]) -> OrderedMap<PythonString> {
    OrderedMap::from_pairs(pairs.iter().map(|(key, value)| (ps(key), ps(value))))
}

fn tokens(values: &[String]) -> Vec<PythonString> {
    values.iter().map(|value| ps(value)).collect()
}

#[derive(Clone)]
struct MissionSpec {
    dir: String,
    name: String,
    domain: String,
    paths: Vec<(String, String)>,
    required: Vec<String>,
    optional: Vec<String>,
}

fn mission_spec(
    dir: &str,
    name: &str,
    domain: &str,
    paths: &[(&str, &str)],
    required: &[&str],
    optional: &[&str],
) -> MissionSpec {
    MissionSpec {
        dir: dir.to_owned(),
        name: name.to_owned(),
        domain: domain.to_owned(),
        paths: paths
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect(),
        required: required.iter().map(|item| (*item).to_owned()).collect(),
        optional: optional.iter().map(|item| (*item).to_owned()).collect(),
    }
}

fn software_mission() -> MissionSpec {
    mission_spec(
        "missions/sw",
        "Software Dev Kitty",
        "software",
        &[
            ("workspace", "src/"),
            ("tests", "tests/"),
            ("deliverables", "contracts/"),
            ("documentation", "docs/"),
        ],
        &["spec.md", "plan.md"],
        &["contracts/", "checklists/"],
    )
}

fn research_mission() -> MissionSpec {
    mission_spec(
        "missions/research",
        "Deep Research Kitty",
        "research",
        &[
            ("workspace", "research/"),
            ("data", "data/"),
            ("deliverables", "findings/"),
            ("documentation", "reports/"),
        ],
        &["spec.md"],
        &["findings/"],
    )
}

struct Scenario {
    label: String,
    kind: &'static str,
    mission: Option<MissionSpec>,
    project_root: String,
    feature_dir: Option<String>,
    planning_read_dir: Option<String>,
    repo_root: Option<String>,
    path_prefix: Option<String>,
    candidate_roots: Vec<String>,
    waived: Vec<String>,
    overrides: Option<Vec<(String, String)>>,
    strict: bool,
    strict_metadata: bool,
    optional_missing: Vec<String>,
    meta_dir: Option<String>,
    resolve_name: Option<String>,
    feature_meta: Option<String>,
    dirs: Vec<String>,
    files: Vec<(String, String)>,
    config_yaml: Option<String>,
    meta_json: Option<String>,
}

struct Builder(Scenario);

impl Builder {
    fn new(label: &str, kind: &'static str, project_root: &str) -> Self {
        Builder(Scenario {
            label: label.to_owned(),
            kind,
            mission: None,
            project_root: project_root.to_owned(),
            feature_dir: None,
            planning_read_dir: None,
            repo_root: None,
            path_prefix: None,
            candidate_roots: Vec::new(),
            waived: Vec::new(),
            overrides: None,
            strict: false,
            strict_metadata: true,
            optional_missing: Vec::new(),
            meta_dir: None,
            resolve_name: None,
            feature_meta: None,
            dirs: Vec::new(),
            files: Vec::new(),
            config_yaml: None,
            meta_json: None,
        })
    }

    fn mission(mut self, spec: MissionSpec) -> Self {
        self.0.mission = Some(spec);
        self
    }

    fn feature(mut self, dir: &str) -> Self {
        self.0.feature_dir = Some(dir.to_owned());
        self
    }

    fn planning(mut self, dir: &str) -> Self {
        self.0.planning_read_dir = Some(dir.to_owned());
        self
    }

    fn repo(mut self, dir: &str) -> Self {
        self.0.repo_root = Some(dir.to_owned());
        self
    }

    fn prefix(mut self, prefix: &str) -> Self {
        self.0.path_prefix = Some(prefix.to_owned());
        self
    }

    fn dirs(mut self, dirs: &[&str]) -> Self {
        self.0
            .dirs
            .extend(dirs.iter().map(|item| (*item).to_owned()));
        self
    }

    fn file(mut self, path: &str, body: &str) -> Self {
        self.0.files.push((path.to_owned(), body.to_owned()));
        self
    }

    fn overrides(mut self, pairs: &[(&str, &str)]) -> Self {
        self.0.overrides = Some(
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        );
        self
    }

    fn candidates(mut self, roots: &[&str]) -> Self {
        self.0
            .candidate_roots
            .extend(roots.iter().map(|item| (*item).to_owned()));
        self
    }

    fn waived(mut self, tokens: &[&str]) -> Self {
        self.0
            .waived
            .extend(tokens.iter().map(|item| (*item).to_owned()));
        self
    }

    fn strict(mut self) -> Self {
        self.0.strict = true;
        self
    }

    fn lenient(mut self) -> Self {
        self.0.strict_metadata = false;
        self
    }

    fn optional_missing(mut self, tokens: &[&str]) -> Self {
        self.0
            .optional_missing
            .extend(tokens.iter().map(|item| (*item).to_owned()));
        self
    }

    fn meta_dir(mut self, dir: &str, body: Option<&str>) -> Self {
        self.0.meta_dir = Some(dir.to_owned());
        self.0.meta_json = body.map(str::to_owned);
        self
    }

    fn meta(mut self, body: &str) -> Self {
        self.0.meta_json = Some(body.to_owned());
        self
    }

    fn resolve(mut self, name: &str) -> Self {
        self.0.resolve_name = Some(name.to_owned());
        self
    }

    fn feature_meta(mut self, feature: &str, body: &str) -> Self {
        self.0.feature_dir = Some(feature.to_owned());
        self.0.feature_meta = Some(body.to_owned());
        self
    }

    fn config(mut self, body: Option<&str>) -> Self {
        self.0.config_yaml = body.map(str::to_owned);
        self
    }

    fn build(self) -> Scenario {
        self.0
    }
}

fn scenario_json(scenario: &Scenario) -> serde_json::Value {
    let mut value = serde_json::json!({
        "label": scenario.label,
        "kind": scenario.kind,
        "project_root": scenario.project_root,
        "strict": scenario.strict,
        "strict_metadata": scenario.strict_metadata,
        "candidates": scenario.candidate_roots,
        "waived": scenario.waived,
        "optional_missing": scenario.optional_missing,
        "feature_meta": scenario.feature_meta.is_some(),
    });
    if let Some(mission) = &scenario.mission {
        value["mission"] = serde_json::json!({
            "dir": mission.dir,
            "name": mission.name,
            "domain": mission.domain,
            "paths": mission.paths,
            "required": mission.required,
            "optional": mission.optional,
        });
    }
    if let Some(feature) = &scenario.feature_dir {
        value["feature_dir"] = serde_json::Value::String(feature.clone());
    }
    if let Some(planning) = &scenario.planning_read_dir {
        value["planning_read_dir"] = serde_json::Value::String(planning.clone());
    }
    if let Some(repo) = &scenario.repo_root {
        value["repo_root"] = serde_json::Value::String(repo.clone());
    }
    if let Some(prefix) = &scenario.path_prefix {
        value["path_prefix"] = serde_json::Value::String(prefix.clone());
    }
    if let Some(meta_dir) = &scenario.meta_dir {
        value["meta_dir"] = serde_json::Value::String(meta_dir.clone());
    }
    if let Some(overrides) = &scenario.overrides {
        value["overrides"] = serde_json::json!(
            overrides
                .iter()
                .map(|(key, value)| serde_json::json!([key, value]))
                .collect::<Vec<_>>()
        );
    }
    if let Some(name) = &scenario.resolve_name {
        value["name"] = serde_json::Value::String(name.clone());
        if scenario.kind == "deliverables" {
            value["slug"] = serde_json::Value::String(name.clone());
        }
    }
    value
}

fn yaml_for(mission: &MissionSpec) -> String {
    let paths = mission
        .paths
        .iter()
        .map(|(key, value)| format!("  {key}: \"{value}\""))
        .collect::<Vec<_>>()
        .join("\n");
    let required = mission
        .required
        .iter()
        .map(|item| format!("    - {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    let optional = mission
        .optional
        .iter()
        .map(|item| format!("    - {item}"))
        .collect::<Vec<_>>()
        .join("\n");
    let required_block = if required.is_empty() {
        "  required: []".to_owned()
    } else {
        format!("  required:\n{required}")
    };
    let optional_block = if optional.is_empty() {
        "  optional: []".to_owned()
    } else {
        format!("  optional:\n{optional}")
    };
    let paths_block = if paths.is_empty() {
        "paths: {}".to_owned()
    } else {
        format!("paths:\n{paths}")
    };
    format!(
        "name: \"{}\"\ndescription: d\nversion: \"1.0.0\"\ndomain: {}\nworkflow:\n  phases:\n    - name: p\n      description: d\nartifacts:\n{}\n{}\n{}\n",
        mission.name, mission.domain, required_block, optional_block, paths_block
    )
}

const MISSION_YAML: &str = "name: \"Fixture\"\ndescription: d\nversion: \"1.0.0\"\ndomain: other\nworkflow:\n  phases:\n    - name: p\n      description: d\nartifacts: {}\n";

#[allow(clippy::vec_init_then_push)]
fn scenarios() -> Vec<Scenario> {
    let mut all = Vec::new();

    // ---- validator ----------------------------------------------------------
    all.push(
        Builder::new("v_missing_artifact_and_build", "validate", "p_s1")
            .mission(software_mission())
            .feature("p_s1/kitty-specs/001-m")
            .dirs(&["p_s1/src"])
            .build(),
    );
    all.push(
        Builder::new("v_all_present", "validate", "p_s2")
            .mission(software_mission())
            .feature("p_s2/kitty-specs/001-m")
            .dirs(&[
                "p_s2/src",
                "p_s2/tests",
                "p_s2/docs",
                "p_s2/kitty-specs/001-m/contracts",
            ])
            .build(),
    );
    all.push(
        Builder::new("v_artifact_only_at_root", "validate", "p_s3")
            .mission(software_mission())
            .feature("p_s3/kitty-specs/001-m")
            .dirs(&["p_s3/src", "p_s3/tests", "p_s3/docs", "p_s3/contracts"])
            .build(),
    );
    all.push(
        Builder::new("v_no_paths", "validate", "p_s4")
            .mission(mission_spec(
                "missions/neutral",
                "Neutral",
                "other",
                &[],
                &["spec.md"],
                &[],
            ))
            .feature("p_s4/kitty-specs/001-m")
            .build(),
    );
    all.push(
        Builder::new("v_remap_declared_only", "validate", "p_s5")
            .mission(software_mission())
            .feature("p_s5/kitty-specs/001-m")
            .dirs(&["p_s5/apps", "p_s5/internal/tests"])
            .overrides(&[
                ("workspace", "apps/"),
                ("tests", "internal/tests/"),
                ("data", "data/"),
            ])
            .build(),
    );
    all.push(
        Builder::new("v_remap_artifact_key_ignored", "validate", "p_s6")
            .mission(software_mission())
            .feature("p_s6/kitty-specs/001-m")
            .dirs(&["p_s6/src", "p_s6/tests", "p_s6/docs", "p_s6/elsewhere"])
            .overrides(&[("deliverables", "elsewhere/")])
            .build(),
    );
    all.push(
        Builder::new("v_remap_collides_with_artifact", "validate", "p_s7")
            .mission(software_mission())
            .feature("p_s7/kitty-specs/001-m")
            .dirs(&[
                "p_s7/tests",
                "p_s7/docs",
                "p_s7/kitty-specs/001-m/contracts",
            ])
            .overrides(&[("workspace", "contracts")])
            .build(),
    );
    all.push(
        Builder::new("v_candidate_root_build_only", "validate", "p_s8")
            .mission(software_mission())
            .feature("p_s8/kitty-specs/001-m")
            .candidates(&["p_s8/.worktrees/lane-a"])
            .dirs(&[
                "p_s8/src",
                "p_s8/docs",
                "p_s8/.worktrees/lane-a/tests",
                "p_s8/.worktrees/lane-a/kitty-specs/001-m/contracts",
            ])
            .build(),
    );
    all.push(
        Builder::new("v_waived_artifact", "validate", "p_s9")
            .mission(software_mission())
            .feature("p_s9/kitty-specs/001-m")
            .dirs(&["p_s9/src", "p_s9/tests", "p_s9/docs"])
            .waived(&["contracts"])
            .build(),
    );
    all.push(
        Builder::new("v_strict_refusal", "validate", "p_s10")
            .mission(software_mission())
            .feature("p_s10/kitty-specs/001-m")
            .strict()
            .build(),
    );
    all.push(
        Builder::new("v_research_prefix", "validate", "p_s11")
            .mission(research_mission())
            .feature("p_s11/kitty-specs/001-m")
            .prefix("docs/research/001-m/")
            .dirs(&[
                "p_s11/docs/research/001-m/research",
                "p_s11/docs/research/001-m/data",
            ])
            .build(),
    );
    all.push(
        Builder::new("v_dotted_and_slash_hints", "validate", "p_s12")
            .mission(mission_spec(
                "missions/odd",
                "Odd",
                "other",
                &[("workspace", "lib/thing.md"), ("tests", "tests/")],
                &[],
                &[],
            ))
            .dirs(&["p_s12/tests"])
            .build(),
    );
    all.push(
        Builder::new("v_feature_outside_root", "validate", "p_s13")
            .mission(software_mission())
            .feature("outside_feature")
            .dirs(&["outside_feature/contracts", "outside_feature_abs"])
            .build(),
    );

    // ---- evaluate -----------------------------------------------------------
    all.push(
        Builder::new("e_all_missing_strict", "evaluate", "p_e1")
            .mission(software_mission())
            .feature("p_e1/kitty-specs/001-m")
            .planning("p_e1/kitty-specs/001-m")
            .repo("p_e1")
            .meta("{\"mission_type\":\"software-dev\"}")
            .optional_missing(&["contracts", "checklists"])
            .build(),
    );
    all.push(
        Builder::new("e_all_missing_lenient", "evaluate", "p_e2")
            .mission(software_mission())
            .feature("p_e2/kitty-specs/001-m")
            .planning("p_e2/kitty-specs/001-m")
            .repo("p_e2")
            .lenient()
            .meta("{\"mission_type\":\"software-dev\"}")
            .optional_missing(&["contracts", "checklists"])
            .build(),
    );
    all.push(
        Builder::new("e_waiver_honoured", "evaluate", "p_e3")
            .mission(software_mission())
            .feature("p_e3/kitty-specs/001-m")
            .planning("p_e3/kitty-specs/001-m")
            .repo("p_e3")
            .meta("{\"mission_type\":\"software-dev\",\"contracts\":\"none\",\"contracts_rationale\":\"no API\"}")
            .optional_missing(&["contracts", "checklists"])
            .build(),
    );
    all.push(
        Builder::new("e_waiver_contradicted", "evaluate", "p_e4")
            .mission(software_mission())
            .feature("p_e4/kitty-specs/001-m")
            .planning("p_e4/kitty-specs/001-m")
            .repo("p_e4")
            .dirs(&["p_e4/kitty-specs/001-m/contracts"])
            .meta("{\"mission_type\":\"software-dev\",\"contracts\":\"none\",\"contracts_rationale\":\"no API\"}")
            .optional_missing(&["contracts", "checklists"])
            .build(),
    );
    all.push(
        Builder::new("e_waiver_malformed", "evaluate", "p_e5")
            .mission(software_mission())
            .feature("p_e5/kitty-specs/001-m")
            .planning("p_e5/kitty-specs/001-m")
            .repo("p_e5")
            .meta("{\"mission_type\":\"software-dev\",\"contracts\":\"nope\"}")
            .optional_missing(&["contracts", "checklists"])
            .build(),
    );
    all.push(
        Builder::new("e_waiver_lenient", "evaluate", "p_e6")
            .mission(software_mission())
            .feature("p_e6/kitty-specs/001-m")
            .planning("p_e6/kitty-specs/001-m")
            .repo("p_e6")
            .lenient()
            .meta("{\"mission_type\":\"software-dev\",\"contracts\":\"none\",\"contracts_rationale\":\"no API\"}")
            .optional_missing(&["contracts", "checklists"])
            .build(),
    );
    all.push(
        Builder::new("e_no_paths_mission", "evaluate", "p_e7")
            .mission(mission_spec(
                "missions/neutral",
                "Neutral",
                "other",
                &[],
                &["spec.md"],
                &[],
            ))
            .feature("p_e7/kitty-specs/001-m")
            .planning("p_e7/kitty-specs/001-m")
            .repo("p_e7")
            .optional_missing(&["contracts"])
            .build(),
    );

    // ---- waiver reader ------------------------------------------------------
    for (label, body) in [
        ("w_absent", None),
        ("w_no_field", Some("{\"mission_type\":\"software-dev\"}")),
        (
            "w_valid",
            Some("{\"contracts\":\"none\",\"contracts_rationale\":\"  no API  \"}"),
        ),
        (
            "w_value_other",
            Some("{\"contracts\":\"None\",\"contracts_rationale\":\"r\"}"),
        ),
        (
            "w_value_null",
            Some("{\"contracts\":null,\"contracts_rationale\":\"r\"}"),
        ),
        (
            "w_value_bool",
            Some("{\"contracts\":false,\"contracts_rationale\":\"r\"}"),
        ),
        ("w_rationale_missing", Some("{\"contracts\":\"none\"}")),
        (
            "w_rationale_blank",
            Some("{\"contracts\":\"none\",\"contracts_rationale\":\"   \"}"),
        ),
    ] {
        all.push(
            Builder::new(label, "waiver", "unused")
                .meta_dir(&format!("meta/{label}"), body)
                .build(),
        );
    }

    // ---- project path_conventions reader ------------------------------------
    for (label, yaml) in [
        ("c_absent", None),
        ("c_empty", Some("")),
        ("c_no_project", Some("charter: x\n")),
        ("c_project_not_map", Some("project: [a]\n")),
        ("c_section_absent", Some("project:\n  uuid: abc\n")),
        (
            "c_section_ok",
            Some(
                "project:\n  path_conventions:\n    workspace: apps/\n    tests: internal/tests\n",
            ),
        ),
        (
            "c_section_not_map",
            Some("project:\n  path_conventions: [a]\n"),
        ),
        (
            "c_unknown_key",
            Some("project:\n  path_conventions:\n    srcs: apps/\n"),
        ),
        (
            "c_artifact_key",
            Some("project:\n  path_conventions:\n    deliverables: out/\n    workspace: apps/\n"),
        ),
        (
            "c_nonstring",
            Some("project:\n  path_conventions:\n    workspace: 3\n"),
        ),
        (
            "c_blank",
            Some("project:\n  path_conventions:\n    workspace: '   '\n"),
        ),
        (
            "c_absolute",
            Some("project:\n  path_conventions:\n    workspace: /abs/src/\n"),
        ),
        (
            "c_traversal",
            Some("project:\n  path_conventions:\n    workspace: ../outside/\n"),
        ),
        (
            "c_dot",
            Some("project:\n  path_conventions:\n    workspace: ./src/\n"),
        ),
        (
            "c_strip_spaces",
            Some("project:\n  path_conventions:\n    workspace: '  apps/  '\n"),
        ),
        ("c_corrupt", Some("project: [unclosed\n")),
    ] {
        all.push(
            Builder::new(label, "config", &format!("cfg/{label}"))
                .repo(&format!("cfg/{label}"))
                .config(yaml)
                .build(),
        );
    }

    // ---- deliverables path --------------------------------------------------
    all.push(
        Builder::new("d_meta_value", "deliverables", "unused")
            .planning("deliv/d_meta_value")
            .meta("{\"deliverables_path\":\"out/here/\"}")
            .build(),
    );
    all.push(
        Builder::new("d_research_default", "deliverables", "unused")
            .planning("deliv/d_research_default")
            .meta("{\"mission_type\":\"research\",\"slug\":\"abc\"}")
            .build(),
    );
    all.push(
        Builder::new("d_research_slug_arg", "deliverables", "unused")
            .planning("deliv/d_research_slug_arg")
            .resolve("zzz")
            .meta("{\"mission_type\":\"research\"}")
            .build(),
    );
    all.push(
        Builder::new("d_software", "deliverables", "unused")
            .planning("deliv/d_software")
            .resolve("q")
            .meta("{\"mission_type\":\"software-dev\"}")
            .build(),
    );
    all.push(
        Builder::new("d_no_meta", "deliverables", "unused")
            .planning("deliv/d_no_meta")
            .resolve("qqq")
            .build(),
    );

    // ---- definition resolution ----------------------------------------------
    all.push(
        Builder::new("r_package_builtin", "resolve", "res/package")
            .resolve("software-dev")
            .build(),
    );
    all.push(
        Builder::new("r_global_shadow_guard", "resolve", "res/shadow")
            .resolve("software-dev")
            .file(
                "home/.kittify/missions/software-dev/mission.yaml",
                MISSION_YAML,
            )
            .build(),
    );
    all.push(
        Builder::new("r_global_custom", "resolve", "res/custom")
            .resolve("custom-global")
            .file(
                "home/.kittify/missions/custom-global/mission.yaml",
                MISSION_YAML,
            )
            .build(),
    );
    all.push(
        Builder::new("r_legacy_builtin_name", "resolve", "res/legacy")
            .resolve("software-dev")
            .file(
                "res/legacy/.kittify/missions/software-dev/mission.yaml",
                MISSION_YAML,
            )
            .build(),
    );
    all.push(
        Builder::new("r_override", "resolve", "res/override")
            .resolve("software-dev")
            .file(
                "res/override/.kittify/overrides/missions/software-dev/mission.yaml",
                MISSION_YAML,
            )
            .build(),
    );
    all.push(
        Builder::new("r_missing", "resolve", "res/missing")
            .resolve("nope-nope")
            .build(),
    );

    // ---- feature-recorded mission type --------------------------------------
    for (label, meta, name) in [
        ("f_typeless", "{}", "software-dev"),
        ("f_research", "{\"mission_type\":\"research\"}", "research"),
        (
            "f_unknown",
            "{\"mission_type\":\"no-such-type\"}",
            "no-such-type",
        ),
        ("f_blank", "{\"mission_type\":\"  \"}", "software-dev"),
        (
            "f_legacy_field",
            "{\"mission\":\"research\"}",
            "software-dev",
        ),
        ("f_nonstring", "{\"mission_type\":5}", "software-dev"),
    ] {
        all.push(
            Builder::new(label, "resolve", &format!("feat/{label}"))
                .resolve(name)
                .feature_meta(&format!("feat/{label}/kitty-specs/001-m"), meta)
                .build(),
        );
    }

    all
}

fn build_fixtures(root: &Path) -> Vec<Scenario> {
    let scenarios = scenarios();
    for scenario in &scenarios {
        if let Some(mission) = &scenario.mission {
            let directory = root.join(&mission.dir);
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("mission.yaml"), yaml_for(mission)).unwrap();
        }
        for directory in &scenario.dirs {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        for (path, body) in &scenario.files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }
        if let Some(meta_dir) = &scenario.meta_dir {
            let directory = root.join(meta_dir);
            fs::create_dir_all(&directory).unwrap();
            if let Some(meta) = &scenario.meta_json {
                fs::write(directory.join("meta.json"), meta).unwrap();
            }
        }
        if scenario.kind == "evaluate" {
            let directory = root.join(scenario.planning_read_dir.as_ref().unwrap());
            fs::create_dir_all(&directory).unwrap();
            if let Some(meta) = &scenario.meta_json {
                fs::write(directory.join("meta.json"), meta).unwrap();
            }
        }
        if scenario.kind == "deliverables" {
            let directory = root.join(scenario.planning_read_dir.as_ref().unwrap());
            fs::create_dir_all(&directory).unwrap();
            if let Some(meta) = &scenario.meta_json {
                fs::write(directory.join("meta.json"), meta).unwrap();
            }
        }
        if scenario.kind == "resolve" {
            fs::create_dir_all(root.join(&scenario.project_root).join(".kittify")).unwrap();
        }
        if let Some(meta) = &scenario.feature_meta {
            let directory = root.join(scenario.feature_dir.as_ref().unwrap());
            fs::create_dir_all(&directory).unwrap();
            fs::write(directory.join("meta.json"), meta).unwrap();
        }
        if let Some(repo) = &scenario.repo_root {
            let directory = root.join(repo).join(".kittify");
            fs::create_dir_all(&directory).unwrap();
            if let Some(config) = &scenario.config_yaml {
                fs::write(directory.join("config.yaml"), config).unwrap();
            }
        }
    }
    fs::write(
        root.join("scenarios.json"),
        serde_json::to_vec_pretty(&serde_json::Value::Array(
            scenarios.iter().map(scenario_json).collect(),
        ))
        .unwrap(),
    )
    .unwrap();
    scenarios
}

fn load_definition(root: &Path, mission: &MissionSpec) -> MissionDefinition {
    let path = root.join(&mission.dir).join("mission.yaml");
    // `mission.yaml` is PyYAML's own dialect; `.kittify/config.yaml` is not.
    load_mission_definition(&path, &PyYamlSafe)
        .expect("fixture definition validates")
        .definition
}

fn native_validate(scenario: &Scenario, root: &Path) -> serde_json::Value {
    let mission = scenario.mission.as_ref().expect("mission spec");
    let definition = load_definition(root, mission);
    let artifact_tokens = artifact_tokens_for(&definition);
    let overrides = scenario
        .overrides
        .as_ref()
        .map(|pairs| ProjectPathConventions {
            overrides: declared_map(pairs),
            warnings: Vec::new(),
        });
    let candidate_roots: Vec<PathBuf> = scenario
        .candidate_roots
        .iter()
        .map(|item| root.join(item))
        .collect();
    let waived = tokens(&scenario.waived);
    let prefix = scenario.path_prefix.as_deref().map(ps);
    let feature_dir = scenario.feature_dir.as_ref().map(|item| root.join(item));
    let project_root = root.join(&scenario.project_root);
    let slug = ps(mission
        .dir
        .rsplit('/')
        .next()
        .unwrap_or(mission.dir.as_str()));

    let outcome = validate_mission_paths(PathValidationInputs {
        mission_name: &definition.name,
        declared_paths: definition.paths(),
        artifact_tokens: &artifact_tokens,
        mission_type_slug: Some(&slug),
        project_root: &project_root,
        path_prefix: prefix.as_ref(),
        feature_dir: feature_dir.as_deref(),
        path_overrides: overrides.as_ref(),
        candidate_source_roots: &candidate_roots,
        waived_artifact_tokens: &waived,
        strict: scenario.strict,
    });
    match outcome {
        Ok(outcome) => {
            let result = &outcome.result;
            serde_json::json!({
                "result": {
                    "mission_name": u(&result.mission_name),
                    "required_paths": units_pairs(&result.required_paths),
                    "existing": units_list(&result.existing_paths),
                    "missing": units_list(&result.missing_paths),
                    "missing_artifact_tokens": units_list(&result.missing_artifact_tokens),
                    "warnings": units_list(&result.warnings),
                    "suggestions": units_list(&result.suggestions),
                    "satisfied_by": units_pairs(&result.satisfied_by),
                    "remappable": units_list(&result.remappable_missing_keys),
                    "mission_type": result.mission_type.as_ref().map(u),
                    "is_valid": result.is_valid(),
                    "format_errors": u(&result.format_errors()),
                    "format_warnings": u(&result.format_warnings()),
                },
                "warnings": units_list(&outcome.notices),
            })
        }
        Err(error) => {
            let message = match &error {
                PathConventionError::Refused(refusal) => refusal.message(),
                other => ps(&other.to_string()),
            };
            serde_json::json!({
                "error_type": u_str("PathValidationError"),
                "error": u(&message),
                "warnings": units_list(&[]),
            })
        }
    }
}

fn native_evaluate(
    scenario: &Scenario,
    root: &Path,
    decoder: &dyn SafeDocumentDecoder,
) -> serde_json::Value {
    let mission = scenario.mission.as_ref().expect("mission spec");
    let definition = load_definition(root, mission);
    let repo_root = root.join(scenario.repo_root.as_ref().unwrap());
    let feature_dir = root.join(scenario.feature_dir.as_ref().unwrap());
    let planning = root.join(scenario.planning_read_dir.as_ref().unwrap());
    let prefix = definition
        .domain
        .is_research()
        .then(|| get_deliverables_path(&planning, "001-m"))
        .flatten()
        .map(|value| ps(&value));

    match evaluate_path_conventions(
        EvaluatePathConventionsInputs {
            definition: Some(&definition),
            repo_root: &repo_root,
            feature_dir: &feature_dir,
            planning_read_dir: &planning,
            path_prefix: prefix.as_ref(),
            mission_type_slug: Some(&ps(mission
                .dir
                .rsplit('/')
                .next()
                .unwrap_or(mission.dir.as_str()))),
            strict_metadata: scenario.strict_metadata,
            candidate_source_roots: &[],
            path_overrides: None,
            waiver: None,
        },
        decoder,
    ) {
        Ok(evaluation) => {
            let optional_missing = tokens(&scenario.optional_missing);
            let kept: Vec<PythonString> = optional_missing
                .iter()
                .filter(|item| !evaluation.dedup_tokens.contains(item))
                .cloned()
                .collect();
            let warnings =
                build_warnings(&kept, &evaluation.violations, evaluation.warning.as_ref());
            let mut dedup = evaluation.dedup_tokens.clone();
            dedup.sort_by(|left, right| left.codepoints().cmp(right.codepoints()));
            serde_json::json!({
                "evaluate": {
                    "violations": units_list(&evaluation.violations),
                    "warning": evaluation.warning.as_ref().map(u),
                    "dedup": units_list(&dedup),
                    "build_warnings": units_list(&warnings),
                },
                "warnings": units_list(&evaluation.notices),
            })
        }
        Err(error) => serde_json::json!({
            "error_type": u_str(error_kind(&error)),
            "error": u(&ps(&error.to_string())),
            "warnings": units_list(&[]),
        }),
    }
}

fn error_kind(error: &PathConventionError) -> &'static str {
    match error {
        PathConventionError::Refused(_) => "PathValidationError",
        PathConventionError::Config(_) => "PathConventionsConfigError",
        PathConventionError::Invalid(_) => "DefinitionError",
    }
}

fn native_resolve(
    scenario: &Scenario,
    root: &Path,
    repo: &Path,
    decoder: &dyn SafeDocumentDecoder,
) -> serde_json::Value {
    let project_root = root.join(&scenario.project_root);
    let providers = CheckoutProviders::with_home(repo, Some(root.join("home")));
    let resolved = if scenario.feature_meta.is_some() {
        resolve_mission_definition_for_mission_dir(
            &project_root.join("kitty-specs").join("001-m"),
            &project_root,
            &providers,
            decoder,
        )
    } else {
        resolve_mission_definition(
            scenario.resolve_name.as_deref().unwrap(),
            &project_root,
            &providers,
            decoder,
        )
    };
    match resolved {
        Ok(resolved) => {
            let mut paths = OrderedMap::new();
            for (key, value) in resolved.definition.paths().iter() {
                paths.insert(key.clone(), value.clone());
            }
            let mission_dir = resolved
                .mission_dir
                .to_string_lossy()
                .replace(&repo.to_string_lossy().to_string(), "<repo>");
            serde_json::json!({
                "resolve": {
                    "mission_dir": u_str(&mission_dir),
                    "name": u(&resolved.definition.name),
                    "domain": u_str(resolved.definition.domain.as_str()),
                    "paths": units_pairs(&paths),
                    "required": units_list(resolved.definition.required_artifacts()),
                    "optional": units_list(resolved.definition.optional_artifacts()),
                },
                "tier": resolved.tier.map(|tier| tier.as_str()),
                "warnings": units_list(&resolved.warnings),
            })
        }
        Err(error) => serde_json::json!({
            "error_type": u_str("MissionNotFoundError"),
            "error": u(&error.message_codepoints()),
            "warnings": units_list(&[]),
        }),
    }
}

fn native_scenario(
    scenario: &Scenario,
    root: &Path,
    repo: &Path,
    decoder: &dyn SafeDocumentDecoder,
) -> serde_json::Value {
    match scenario.kind {
        "validate" => native_validate(scenario, root),
        "evaluate" => native_evaluate(scenario, root, decoder),
        "waiver" => {
            let directory = root.join(scenario.meta_dir.as_ref().unwrap());
            match read_contracts_waiver_from_meta(&directory) {
                Ok(waiver) => serde_json::json!({
                    "waiver": {
                        "waived": waiver.waived,
                        "rationale": waiver.rationale.as_ref().map(u),
                        "warning": waiver.warning.as_ref().map(u),
                    },
                    "warnings": units_list(&[]),
                }),
                Err(error) => serde_json::json!({
                    "error_type": u_str("MissionMetaReadError"),
                    "error": u(&ps(&error.to_string())),
                    "warnings": units_list(&[]),
                }),
            }
        }
        "config" => {
            let repo_root = root.join(scenario.repo_root.as_ref().unwrap());
            match load_project_path_conventions(&repo_root, decoder) {
                Ok(overrides) => serde_json::json!({
                    "config": { "overrides": units_pairs(&overrides.overrides) },
                    "warnings": units_list(&overrides.warnings),
                }),
                Err(error) => serde_json::json!({
                    "error_type": u_str("PathConventionsConfigError"),
                    "error": u(&error.message),
                    "warnings": units_list(&[]),
                }),
            }
        }
        "deliverables" => {
            let directory = root.join(scenario.planning_read_dir.as_ref().unwrap());
            let slug = scenario.resolve_name.clone().unwrap_or_default();
            let value = get_deliverables_path(&directory, &slug);
            serde_json::json!({
                "deliverables": value.as_ref().map(|value| u_str(value)),
                "warnings": units_list(&[]),
            })
        }
        "resolve" => native_resolve(scenario, root, repo, decoder),
        other => panic!("unknown scenario kind {other}"),
    }
}

fn native_leg(root: &Path, repo: &Path) -> usize {
    assert_eq!(std::env::current_dir().unwrap(), root);
    for (key, name) in roots() {
        assert_eq!(
            std::env::var_os(key).unwrap(),
            root.join(name),
            "root {key} must be the asserted sandbox directory"
        );
    }
    assert_eq!(std::env::var_os("PYTHONPATH").unwrap(), repo.join("src"));
    assert_eq!(
        std::env::var_os("SPEC_KITTY_PACKS_ROOT").unwrap(),
        repo.join("packs")
    );

    let receipts: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("source.json")).unwrap()).unwrap();
    let rows = receipts["rows"].as_array().unwrap();
    let scenarios = scenarios();
    assert_eq!(rows.len(), scenarios.len(), "one receipt per scenario");
    // The injected decoder is the *project config* one: `.kittify/config.yaml`
    // is read by ruamel's safe loader, never by mission.yaml's PyYAML dialect.
    let decoder = RuamelSafe;
    let mut compared = 0usize;

    for (row, scenario) in rows.iter().zip(scenarios.iter()) {
        assert_eq!(row["label"].as_str().unwrap(), scenario.label);
        let native = native_scenario(scenario, root, repo, &decoder);
        let label = &scenario.label;

        match (row.get("error_type"), native.get("error_type")) {
            (None, None) => {
                for field in [
                    "result",
                    "evaluate",
                    "waiver",
                    "config",
                    "deliverables",
                    "resolve",
                ] {
                    if let Some(expected) = row.get(field) {
                        assert_eq!(
                            native.get(field),
                            Some(expected),
                            "{label}: {field} must match Source\nnative: {native}"
                        );
                    }
                }
                assert_eq!(
                    receipt_strings(&row["warnings"]),
                    receipt_strings(&native["warnings"]),
                    "{label}: the warnings.warn channel must match Source"
                );

                if scenario.kind == "resolve" {
                    let tier = native["tier"].as_str();
                    let expected = match scenario.label.as_str() {
                        "r_package_builtin" | "r_global_shadow_guard" => {
                            Some(ResolutionTier::PackageDefault)
                        }
                        "r_global_custom" => Some(ResolutionTier::GlobalMission),
                        "r_legacy_builtin_name" => Some(ResolutionTier::Legacy),
                        "r_override" => Some(ResolutionTier::Override),
                        "f_typeless" | "f_research" | "f_blank" | "f_legacy_field"
                        | "f_nonstring" => Some(ResolutionTier::PackageDefault),
                        _ => None,
                    };
                    assert_eq!(
                        tier,
                        expected.map(ResolutionTier::as_str),
                        "{label}: resolver tier"
                    );
                    if scenario.label == "r_legacy_builtin_name" {
                        assert!(
                            receipt_strings(&native["warnings"])
                                .iter()
                                .any(|warning| warning
                                    .to_scalar_string()
                                    .is_ok_and(|text| text.starts_with("Legacy asset resolved:"))),
                            "legacy nudge must be reported"
                        );
                    }
                }
            }
            (Some(expected_type), Some(actual_type)) => {
                assert_eq!(
                    receipt_string(expected_type),
                    receipt_string(actual_type),
                    "{label}: failure kind must match Source"
                );
                // Source's validator/reader messages are reproduced exactly. A
                // corrupt meta.json is a Kernel/codec read detail, compared as a
                // kind only (declared in the report).
                if scenario.kind != "waiver" {
                    assert_eq!(
                        receipt_string(&row["error"]),
                        receipt_string(&native["error"]),
                        "{label}: message must match Source"
                    );
                }
            }
            (expected, actual) => panic!(
                "{label}: Source failure {expected:?} vs Native {actual:?} (native: {native})"
            ),
        }
        compared += 1;
    }
    compared
}

#[test]
fn path_authority_matches_actual_source() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();

    if let Some(root) = std::env::var_os("SK_L3_PATHS_NATIVE") {
        let root = PathBuf::from(root);
        let compared = native_leg(&root, &repo);
        println!("{compared} path-authority scenarios compared against actual Source");
        return;
    }

    let root = tempfile::tempdir().unwrap().keep();
    for (_, name) in roots() {
        fs::create_dir_all(root.join(name)).unwrap();
    }
    let scenarios = build_fixtures(&root);
    assert!(scenarios.len() >= 40);
    eprintln!("path evidence: {}", root.display());

    let source = child(&root, &repo, oracle::python())
        .arg("-c")
        .arg(include_str!("oracle/path_conventions.py"))
        .arg(&root)
        .arg(&repo)
        .output()
        .unwrap();
    fs::write(root.join("source.stdout"), &source.stdout).unwrap();
    fs::write(root.join("source.stderr"), &source.stderr).unwrap();
    fs::write(
        root.join("source.status"),
        format!("{:?}\n", source.status.code()),
    )
    .unwrap();
    assert!(
        source.status.success(),
        "Source FIRST: {}",
        String::from_utf8_lossy(&source.stderr)
    );
    fs::write(root.join("source.json"), &source.stdout).unwrap();

    let native = child(&root, &repo, std::env::current_exe().unwrap())
        .args([
            "--exact",
            "path_authority_matches_actual_source",
            "--nocapture",
        ])
        .env("SK_L3_PATHS_NATIVE", &root)
        .output()
        .unwrap();
    fs::write(root.join("native.stdout"), &native.stdout).unwrap();
    fs::write(root.join("native.stderr"), &native.stderr).unwrap();
    fs::write(
        root.join("native.status"),
        format!("{:?}\n", native.status.code()),
    )
    .unwrap();
    assert!(
        native.status.success(),
        "Native: {} {}",
        String::from_utf8_lossy(&native.stdout),
        String::from_utf8_lossy(&native.stderr)
    );
}

#[test]
fn registered_mission_type_identity_is_neutral() {
    struct Stub;
    impl DefinitionProviders for Stub {
        fn org_pack_roots(&self, _project_root: &Path) -> Vec<PathBuf> {
            Vec::new()
        }
        fn registered_mission_type(
            &self,
            _project_root: &Path,
            name: &str,
        ) -> Option<RegisteredMissionType> {
            (name == "acme-audit").then(|| RegisteredMissionType {
                id: ps("acme-audit"),
                display_name: ps("Acme Audit"),
                action_sequence: Some(vec![ps("inspect"), ps("report")]),
                path_conventions: Some(OrderedMap::from_pairs([(ps("workspace"), ps("audit/"))])),
            })
        }
        fn packaged_missions_root(&self, _project_root: &Path) -> Option<PathBuf> {
            None
        }
        fn kittify_home(&self, _project_root: &Path) -> Option<PathBuf> {
            None
        }
    }
    let root = tempfile::tempdir().unwrap().keep();
    fs::create_dir_all(root.join(".kittify")).unwrap();
    let resolved =
        resolve_mission_definition("acme-audit", &root, &Stub, &PyYamlSafe).expect("resolves");
    assert_eq!(resolved.tier, None, "a synthetic identity has no file tier");
    assert_eq!(
        resolved.definition.name.to_scalar_string().unwrap(),
        "Acme Audit"
    );
    assert_eq!(resolved.definition.domain.as_str(), "other");
    assert_eq!(
        resolved.definition.version.to_scalar_string().unwrap(),
        "0.0.0"
    );
    assert!(resolved.definition.required_artifacts().is_empty());
    assert!(resolved.definition.optional_artifacts().is_empty());
    let phases = &resolved.definition.workflow.phases;
    assert_eq!(phases.len(), 1);
    assert_eq!(
        phases[0].name.to_scalar_string().unwrap(),
        "org-mission-type"
    );
    assert_eq!(
        phases[0].description.to_scalar_string().unwrap(),
        "Derived from the mission type's action_sequence: inspect -> report"
    );
    let paths: Vec<(String, String)> = resolved
        .definition
        .paths()
        .iter()
        .map(|(key, value)| {
            (
                key.to_scalar_string().unwrap(),
                value.to_scalar_string().unwrap(),
            )
        })
        .collect();
    assert_eq!(paths, vec![("workspace".to_owned(), "audit/".to_owned())]);
    assert_eq!(
        resolved.mission_dir,
        root.join(".kittify").join("missions").join("acme-audit"),
        "the marker path mirrors Source"
    );
}
