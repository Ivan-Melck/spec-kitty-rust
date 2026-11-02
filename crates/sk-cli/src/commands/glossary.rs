//! Read-only glossary lookup with explicit scopes and optional explicit seeds.
//!
//! The default seed location is `.kittify/glossaries/<scope>.yaml`. A caller
//! can override one scope's file with `--seed-file SCOPE=PATH`, which is useful
//! for reviewing a proposed seed without installing it. Nothing is written.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde_json::{Value, json};
use sk_glossary::{
    Glossary, GlossaryError, GlossaryScope, LookupOptions, SCOPE_RESOLUTION_ORDER, TermSense,
    parse_seed,
};
use sk_kernel::{read_bounded, resolve_within};

use crate::commands::path_string;
use crate::commands::project::{optional_root, resolve_root};
use crate::output::{CliError, CommandData};

const MAX_SEED_BYTES: u64 = 1024 * 1024;

/// Resolve one term against the selected scopes.
pub fn lookup(
    explicit_root: Option<&Path>,
    term: &str,
    scope_tokens: &[String],
    seed_files: &[String],
    confidence: f64,
    critical_step: bool,
) -> Result<CommandData, CliError> {
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(CliError::usage(format!(
            "--confidence must be a finite number in 0.0..=1.0, got {confidence}"
        )));
    }

    let mut scopes: Vec<GlossaryScope> = if scope_tokens.is_empty() {
        SCOPE_RESOLUTION_ORDER.to_vec()
    } else {
        Vec::new()
    };
    for token in scope_tokens.iter().flat_map(|token| token.split(',')) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let scope = GlossaryScope::from_str(token)
            .map_err(|error| CliError::usage(format!("--scope is invalid: {error}")))?;
        if !scopes.contains(&scope) {
            scopes.push(scope);
        }
    }

    let overrides = parse_seed_overrides(seed_files, &scopes)?;
    for (scope, _) in &overrides {
        if !scopes.contains(scope) {
            return Err(CliError::usage(format!(
                "--seed-file names scope {scope}, which is not in the selected --scope list"
            )));
        }
    }

    // A project root is only required for scopes that fall back to the default
    // `.kittify/glossaries` location. If every scope has an explicit seed, the
    // lookup works without a project.
    let needs_default_root = scopes
        .iter()
        .any(|scope| !overrides.iter().any(|(overridden, _)| overridden == scope));
    let root = if needs_default_root {
        Some(resolve_root(explicit_root)?)
    } else {
        optional_root(explicit_root)?
    };

    let mut senses: Vec<TermSense> = Vec::new();
    let mut seed_info: Vec<Value> = Vec::new();

    for scope in &scopes {
        let explicit = overrides
            .iter()
            .find(|(overridden, _)| overridden == scope)
            .map(|(_, path)| path.clone());

        let (path, source) = match explicit {
            Some(path) => (path, "explicit"),
            None => {
                let root = root.as_ref().ok_or_else(|| {
                    CliError::domain(format!(
                        "no project root is available for the default {scope} seed; pass --project-root or --seed-file {scope}=PATH"
                    ))
                })?;
                let relative = Path::new(".kittify")
                    .join("glossaries")
                    .join(scope.filename());
                let path = resolve_within(root, &relative).map_err(|error| {
                    CliError::domain(format!(
                        "the default {scope} seed path is not contained by {}: {error}",
                        root.display()
                    ))
                })?;
                (path, "default")
            }
        };

        if source == "default" && !path.exists() {
            seed_info.push(json!({
                "scope": scope.as_str(),
                "path": path_string(&path),
                "source": source,
                "present": false,
                "terms": 0,
            }));
            continue;
        }

        let bytes = read_bounded(&path, MAX_SEED_BYTES).map_err(|error| {
            CliError::domain(format!(
                "could not read the {scope} glossary seed at {}: {error}",
                path.display()
            ))
        })?;
        let text = String::from_utf8(bytes).map_err(|error| {
            CliError::domain(format!(
                "the {scope} glossary seed at {} is not valid UTF-8: {error}",
                path.display()
            ))
        })?;
        let parsed =
            parse_seed(*scope, path.clone(), &text).map_err(|error| seed_error(&path, error))?;
        seed_info.push(json!({
            "scope": scope.as_str(),
            "path": path_string(&path),
            "source": source,
            "present": true,
            "terms": parsed.len(),
        }));
        senses.extend(parsed);
    }

    let glossary = Glossary::from_senses(senses);
    let options = LookupOptions {
        scopes: scopes.clone(),
        confidence,
        critical_step,
    };
    let result = glossary
        .resolve_with(term, &options)
        .map_err(glossary_error)?;

    Ok(CommandData::success(json!({
        "requested_term": term,
        "term": result.surface,
        "scopes": scopes.iter().map(|scope| scope.as_str()).collect::<Vec<_>>(),
        "seeds": seed_info,
        "confidence": confidence,
        "critical_step": critical_step,
        "result": result,
        "read_only": true,
    })))
}

fn parse_seed_overrides(
    seed_files: &[String],
    selected_scopes: &[GlossaryScope],
) -> Result<Vec<(GlossaryScope, PathBuf)>, CliError> {
    let mut overrides: Vec<(GlossaryScope, PathBuf)> = Vec::new();
    for raw in seed_files {
        // Preferred spelling: SCOPE=PATH. Also accept a path whose canonical
        // filename names the scope, or a bare path when exactly one scope was
        // selected, so a proposed seed can be reviewed without renaming it.
        let (scope, path) = match raw.split_once('=') {
            Some((scope_token, path_token))
                if GlossaryScope::from_str(scope_token.trim()).is_ok() =>
            {
                let scope =
                    GlossaryScope::from_str(scope_token.trim()).expect("scope was just validated");
                if path_token.trim().is_empty() {
                    return Err(CliError::usage(format!(
                        "--seed-file {scope} has an empty path"
                    )));
                }
                (scope, PathBuf::from(path_token))
            }
            _ => {
                let path = PathBuf::from(raw);
                let scope = GlossaryScope::from_path(&path)
                    .or_else(|| {
                        (selected_scopes.len() == 1).then(|| selected_scopes[0])
                    })
                    .ok_or_else(|| {
                        CliError::usage(format!(
                            "--seed-file expects SCOPE=PATH, a seed file named <scope>.yaml, or a single --scope; got {raw:?}"
                        ))
                    })?;
                (scope, path)
            }
        };
        if overrides.iter().any(|(existing, _)| *existing == scope) {
            return Err(CliError::usage(format!(
                "--seed-file for scope {scope} was supplied more than once"
            )));
        }
        overrides.push((scope, path));
    }
    Ok(overrides)
}

fn seed_error(path: &Path, error: sk_glossary::SeedFileError) -> CliError {
    CliError::domain(format!(
        "glossary seed {} failed validation: {error}",
        path.display()
    ))
    .with_details(json!({
        "path": path_string(path),
        "validation_errors": error.errors(),
    }))
}

fn glossary_error(error: GlossaryError) -> CliError {
    match error {
        GlossaryError::SeedFile(seed) => CliError::domain(format!(
            "glossary seed failed validation: {seed}"
        ))
        .with_details(json!({
            "path": path_string(seed.file_path()),
            "validation_errors": seed.errors(),
        })),
        other => CliError::domain(format!("glossary lookup failed: {other}")),
    }
}
