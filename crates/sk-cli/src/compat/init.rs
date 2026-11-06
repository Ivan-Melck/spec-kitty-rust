//! `spec-kitty init`: real project scaffolding through `sk-install`.
//!
//! The command is human-output only upstream (there is no `init --json`), and
//! this route matches that: machine callers get the same exit classes and a
//! non-zero status on refusal.
//!
//! This is the **full** native delivery path. It discovers the verified asset
//! set (template tree, charter skill catalog, agent-profile catalog and the
//! built-in activation preset), then calls
//! [`sk_install::initialize_with_details`] so the project receives everything
//! upstream `init` delivers — not just the command/config core. A build that
//! cannot locate the assets refuses instead of silently scaffolding less.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_install::{
    InitDetails, InitOutcome, InitRequest, ProjectAssets, ProjectEnvironment,
    initialize_with_details,
};

use super::support::{CompatError, parse_agents, path_string};

/// Run initialization in `project_name` (or the current directory).
pub fn init(
    project_name: Option<&str>,
    ai_assistant: Option<&str>,
    non_interactive: bool,
) -> Result<(), CompatError> {
    let here = match project_name {
        None => true,
        Some(name) if name.trim() == "." || name.trim().is_empty() => true,
        Some(_) => false,
    };

    let agents =
        parse_agents(ai_assistant).map_err(|error| CompatError::domain(error.message()))?;
    if !here && agents.is_empty() {
        return Err(CompatError::domain("--ai is required for a new project"));
    }

    let project_path = if here {
        env::current_dir().map_err(|error| {
            CompatError::domain(format!("Cannot access current directory: {error}"))
        })?
    } else {
        let name = project_name.expect("checked above");
        let path = PathBuf::from(name);
        let path = if path.is_absolute() {
            path
        } else {
            env::current_dir()
                .map_err(|error| {
                    CompatError::domain(format!("Cannot access current directory: {error}"))
                })?
                .join(path)
        };
        if path.exists() {
            return Err(CompatError::domain(format!(
                "Directory '{}' already exists\nPlease choose a different project name or remove the existing directory.",
                name
            )));
        }
        fs::create_dir_all(&path).map_err(|error| {
            CompatError::domain(format!("could not create {}: {error}", path.display()))
        })?;
        fs::canonicalize(&path).unwrap_or(path)
    };

    let project_path = fs::canonicalize(&project_path).unwrap_or(project_path);
    let name = project_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_owned());

    let config_present = project_path.join(".kittify/config.yaml").is_file();
    if agents.is_empty() {
        if config_present {
            // Refresh the saved configuration exactly as upstream does.
        } else if non_interactive || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return Err(CompatError::domain(
                "--ai is required in non-interactive mode",
            ));
        } else {
            return Err(CompatError::domain(
                "--ai is required: this native CLI does not render the interactive agent picker",
            ));
        }
    }

    let assets = discover_assets()?;

    let request = InitRequest::new(&project_path, agents.clone(), super::CORE_VERSION)
        .with_project_assets(assets)
        .with_core_version(super::CORE_VERSION)
        .with_environment(ProjectEnvironment::detect())
        .with_lock_wait(Duration::from_secs(10));

    println!("Specify Project Setup");
    println!();
    println!("{:<15} {name}", "Project:");
    println!(
        "{:<15} {}",
        "Working Path:",
        env::current_dir().unwrap_or_default().display()
    );
    if !here {
        println!("{:<15} {}", "Target Path:", project_path.display());
    }
    if !agents.is_empty() {
        println!();
        println!("Selected AI assistant(s): {}", agents.join(", "));
    }

    let (report, details) = initialize_with_details(&request)
        .map_err(|error| CompatError::domain(format!("initialization failed: {error}")))?;

    println!();
    match report.outcome {
        InitOutcome::Initialized => {
            println!(
                "✓ Initialized Spec Kitty project in {}",
                project_path.display()
            );
        }
        InitOutcome::AlreadyInitialized => {
            // Do not claim a template/configuration verification: a refresh
            // intentionally never re-scaffolds, so an absent template tree is
            // reported separately as a warning rather than covered by this line.
            println!(
                "Already initialized. Missing per-agent skill surfaces were restored on this re-run. Run `spec-kitty upgrade` to migrate to the latest version."
            );
        }
        InitOutcome::ResumedDelivery => {
            println!("Resumed and completed the interrupted delivery.");
        }
    }
    println!(
        "   files: {} created, {} updated, {} adopted, {} kept, {} preserved",
        report.summary.created,
        report.summary.updated,
        report.summary.adopted,
        report.summary.kept,
        report.summary.preserved
    );
    if report.config_written {
        println!(
            "   config: {}",
            path_string(&project_path.join(".kittify/config.yaml"))
        );
    }
    if report.metadata_written {
        println!(
            "   metadata: {}",
            path_string(&project_path.join(".kittify/metadata.yaml"))
        );
    }
    print_details(&details);
    for warning in &report.warnings {
        println!("   warning: {warning}");
    }
    for notice in &details.security_notice {
        println!(
            "   note: {} stores project data in {}; review what that folder shares before committing",
            notice.display, notice.folder
        );
    }
    Ok(())
}

fn print_details(details: &InitDetails) {
    if let Some(mode) = details.template_mode {
        println!("   templates: {}", mode.as_str());
    }
    for copied in &details.template_copied {
        println!("   copied: {copied}");
    }
    for backup in &details.backups {
        println!("   preserved operator content in {backup}");
    }
    let skills = details.skills_written.len();
    if skills > 0 {
        println!(
            "   skills: {skills} installed, {} adopted, {} drifted, {} restored",
            details.skills_adopted.len(),
            details.skills_drifted.len(),
            details.skills_restored.len()
        );
    }
    let profiles = details.profiles_written.len();
    if profiles > 0 {
        println!(
            "   agent profiles: {profiles} projected, {} current, {} preserved",
            details.profiles_kept.len(),
            details.profiles_preserved.len()
        );
    }
    for path in &details.profiles_preserved {
        println!("   preserved operator-edited profile: {path}");
    }
    if !details.orientation_written.is_empty() {
        println!(
            "   orientation: {} written",
            details.orientation_written.join(", ")
        );
    }
    if details.activation_provisioned {
        println!("   mission-type activations provisioned from the default preset");
    }
    if let Some(identity) = &details.identity
        && let Some(uuid) = identity.project_uuid
    {
        println!("   identity: {uuid}");
    }
    if details.event_written {
        println!("   recorded ProjectInitialized");
    }
    if !details.ledger_seeded.is_empty() {
        println!(
            "   seeded migration ledger ({} records)",
            details.ledger_seeded.len()
        );
    }
    if let Some(wiring) = &details.merge_driver_wiring
        && !wiring.installed.is_empty()
    {
        println!(
            "   configured clone-local merge-driver settings ({} keys)",
            wiring.installed.len()
        );
    }
}

/// Locate the verified asset set the full delivery needs.
///
/// A source checkout keeps the offering templates/skills beside `src/`; the
/// mission/additional packs may live at `packs/built-in` or at an external
/// `SPEC_KITTY_PACKS_ROOT`. An installed layout that cannot provide the assets
/// is a refusal, never a silently reduced scaffold.
fn discover_assets() -> Result<ProjectAssets, CompatError> {
    let pack = super::support::pack_root();
    let checkout = checkout_root(pack.as_deref()).ok_or_else(|| {
        CompatError::domain(
            "the native installer could not locate the built-in template/skill assets; \
             set SPEC_KITTY_TEMPLATE_ROOT to a Spec Kitty checkout or reinstall the CLI",
        )
    })?;
    let assets = ProjectAssets::discover(&checkout).map_err(|error| {
        CompatError::domain(format!("the built-in assets are not usable: {error}"))
    })?;
    Ok(assets)
}

fn checkout_root(pack: Option<&Path>) -> Option<PathBuf> {
    let absolute = |path: PathBuf| fs::canonicalize(&path).unwrap_or(path);
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(value) = env::var_os("SPEC_KITTY_TEMPLATE_ROOT") {
        candidates.push(absolute(PathBuf::from(value)));
    }
    if let Some(value) = env::var_os("SPEC_KITTY_PACKS_ROOT") {
        // Two source-backed shapes are accepted: `$PWD/packs` (the value the
        // oracle convention documents) and `$PWD/packs/built-in`.
        let configured = absolute(PathBuf::from(value));
        if configured.join("built-in/missions").is_dir() {
            if let Some(root) = configured.parent() {
                candidates.push(absolute(root.to_path_buf()));
            }
        } else if configured.join("missions").is_dir()
            && let Some(root) = configured.parent().and_then(Path::parent)
        {
            candidates.push(absolute(root.to_path_buf()));
        }
    }
    if let Some(pack) = pack {
        // `<checkout>/packs/built-in` → `<checkout>`.
        if let Some(root) = pack.parent().and_then(Path::parent) {
            candidates.push(absolute(root.to_path_buf()));
        }
    }
    candidates.push(absolute(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
    ));
    if let Ok(current) = env::current_dir() {
        let mut cursor = Some(current.as_path());
        while let Some(directory) = cursor {
            candidates.push(absolute(directory.to_path_buf()));
            cursor = directory.parent();
        }
    }
    candidates.into_iter().find(|root| {
        root.join("src/charter/offering/templates").is_dir()
            && root.join("src/charter/offering/skills").is_dir()
    })
}

/// Refuse an init target that is not a directory.
pub fn require_directory(path: &Path) -> Result<(), CompatError> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(CompatError::domain(format!(
            "{} is not a directory",
            path.display()
        )))
    }
}
