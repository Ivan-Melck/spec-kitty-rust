//! `spec-kitty init`: real project scaffolding through `sk-install`.
//!
//! The command is human-output only upstream (there is no `init --json`), and
//! this route matches that: machine callers get the same exit classes and a
//! non-zero status on refusal.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sk_install::{InitOutcome, InitRequest, TemplateSource, initialize};

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

    let templates = super::support::pack_root()
        .map(|pack| TemplateSource::from_pack_root(&pack).with_mission_type("software-dev"));
    if let Some(source) = &templates {
        source.ensure_available().map_err(|error| {
            CompatError::domain(format!("the built-in pack is not usable: {error}"))
        })?;
    }

    let mut request = InitRequest::new(&project_path, agents.clone(), super::CORE_VERSION);
    if let Some(source) = templates {
        request = request.with_templates(source);
    }
    request = request.with_lock_wait(Duration::from_secs(10));

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

    let report = initialize(&request)
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
            println!(
                "Already initialized. Agent skill surfaces were verified on this re-run. Run `spec-kitty upgrade` to migrate to the latest version."
            );
        }
        InitOutcome::ResumedDelivery => {
            println!("Resumed and completed the interrupted command delivery.");
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
    for warning in &report.warnings {
        println!("   warning: {warning}");
    }
    Ok(())
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
