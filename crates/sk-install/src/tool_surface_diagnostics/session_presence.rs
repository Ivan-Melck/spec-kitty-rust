//! Session-presence probes: the `context_file`, `rule` and `hook` surfaces.
//!
//! This is the read-only half of the reference provider
//! `src/specify_cli/tool_surface/providers/session_presence.py` on the Native
//! owners:
//!
//! * **Expansion** comes from [`crate::session::session_artefacts`], the single
//!   writer roster ([`crate::session::session_writer`]). This file contains no
//!   harness/path table, so an audit can never invent a second roster.
//! * **Orientation presence and staleness** come from
//!   [`crate::session::assess_orientation`], which scopes the version stamp to the
//!   marker-delimited block exactly like the writer.
//! * **Claude hook state** comes from
//!   [`crate::session::observe_claude_settings`], which reuses the same
//!   `settings.json` walker the writer merges through.
//!
//! Nothing here writes, creates, chmods or locks: the projection writers in
//! [`crate::session`] are never called from an audit.
//!
//! Reference behaviour reproduced verbatim, including its quirks:
//!
//! * a harness whose upstream writer is a `NullWriter` yields exactly one
//!   `research-gap-surface` instance that carries the **context_file**
//!   definition, even when the requested kind was hook/rule;
//! * Claude's two lifecycle hooks are planned as **two** instances of the same
//!   `settings.json` path, so both are probed (and reported) independently;
//! * a missing *rule* file is reported with the `--kind context_file --fix`
//!   hint, because the reference shares one repair hint for both kinds.
//!
//! # Failure phases (CF33, extended by CF37)
//!
//! Three provider failures are *escaping* in the reference, not verdicts, and
//! are propagated here through [`SessionSurfaceFailure`]:
//!
//! * an orientation surface that cannot be decoded as strict UTF-8 — the
//!   reference's `read_text(encoding="utf-8")` raises `UnicodeDecodeError` inside
//!   `expand`/`_instance_present`;
//! * a `.claude/settings.json` read failure other than "absent" (`EACCES`, `EIO`,
//!   a non-regular destination) — `Path.read_bytes` raises `OSError`;
//! * a `.claude/settings.json` whose decoded text makes `json.loads` raise
//!   something that is not a `JSONDecodeError` — CPython's integer-digit ceiling
//!   (`ValueError`) or its recursion ceiling (`RecursionError`). `_load` catches
//!   only `JSONDecodeError`, so the provider has no verdict to report.
//!
//! A *decode* failure of the settings bytes is different: the reference catches
//! `SettingsNotDecodableError` and reports `unsafe`, so that stays a finding.
//! An orientation read `OSError` is likewise caught by the reference and stays
//! `missing`.

use std::path::{Path, PathBuf};

use sk_kernel::COMPAT_CORE_VERSION;

use crate::session::{
    CLAUDE_SETTINGS_REL, ClaudeSettingsShape, OrientationSectionState, SessionArtefactKind,
    assess_orientation, observe_claude_settings, session_artefacts,
};

use super::{
    CONTEXT_FILE_MISSING, RESEARCH_GAP_SENTINEL, RESEARCH_GAP_SURFACE, SESSION_CONTEXT_REPAIR_HINT,
    SESSION_HOOK_REPAIR_HINT, SESSION_PRESENCE_INCOMPLETE, SESSION_PRESENCE_UNDECODABLE,
    SEVERITY_ERROR, SEVERITY_INFO, SEVERITY_WARNING, STALE_GENERATED_SURFACE, STATE_MISSING,
    STATE_NOT_APPLICABLE, STATE_STALE, STATE_UNSAFE, SessionSurfaceFailure, SurfaceDefinition,
    SurfaceFinding, SurfaceInstance, SurfaceKind, SurfaceStatus, path_string, project_of,
    session_presence_definition,
};

/// Expand a session-presence definition into one instance per managed artefact.
///
/// The requested `definition` only carries the kind; each emitted instance gets
/// the definition of the artefact's own kind (the reference's `_definition_for`),
/// and instances whose kind differs from the request are filtered out so `--kind`
/// selection stays exact.
pub(crate) fn expand(
    definition: &SurfaceDefinition,
    tool_key: &str,
    project_root: &Path,
    instances: &mut Vec<SurfaceInstance>,
) {
    let artefacts = session_artefacts(tool_key);
    if artefacts.is_empty() {
        // Upstream `_research_gap_instance` always carries the context_file
        // definition, regardless of the kind that was actually requested.
        instances.push(SurfaceInstance {
            definition: session_presence_definition(SurfaceKind::ContextFile),
            path: PathBuf::from(RESEARCH_GAP_SENTINEL),
            owner: tool_key.to_string(),
            file_hash: None,
            surface_id: None,
        });
        return;
    }
    for artefact in artefacts {
        let kind = surface_kind(artefact.kind);
        if kind != definition.kind {
            continue;
        }
        instances.push(SurfaceInstance {
            definition: session_presence_definition(kind),
            path: project_root.join(artefact.path),
            owner: tool_key.to_string(),
            file_hash: None,
            surface_id: None,
        });
    }
}

fn surface_kind(kind: SessionArtefactKind) -> SurfaceKind {
    match kind {
        SessionArtefactKind::ContextFile => SurfaceKind::ContextFile,
        SessionArtefactKind::Rule => SurfaceKind::Rule,
        SessionArtefactKind::Hook => SurfaceKind::Hook,
    }
}

/// Probe one planned session-presence instance against disk (read-only).
///
/// Returns a [`SessionSurfaceFailure`] only for the read/decode phases the
/// reference lets escape the provider; every health verdict stays a status.
pub(crate) fn probe(instance: &SurfaceInstance) -> Result<SurfaceStatus, SessionSurfaceFailure> {
    if instance.path == Path::new(RESEARCH_GAP_SENTINEL) {
        return Ok(research_gap_status(instance));
    }
    match instance.definition.kind {
        SurfaceKind::Hook => probe_hook(instance),
        _ => probe_orientation(instance),
    }
}

fn research_gap_status(instance: &SurfaceInstance) -> SurfaceStatus {
    let finding = SurfaceFinding::new(
        RESEARCH_GAP_SURFACE,
        SEVERITY_INFO,
        format!(
            "No known session-presence mechanism for {}.",
            instance.owner
        ),
    )
    .with_tool(&instance.owner)
    .with_surface(&instance.id());
    SurfaceStatus::with(instance.clone(), STATE_NOT_APPLICABLE, finding)
}

fn probe_orientation(instance: &SurfaceInstance) -> Result<SurfaceStatus, SessionSurfaceFailure> {
    let assessment = assess_orientation(&instance.path, COMPAT_CORE_VERSION);
    if assessment.present() {
        if assessment.stale {
            let finding = SurfaceFinding::new(
                STALE_GENERATED_SURFACE,
                SEVERITY_WARNING,
                format!(
                    "Orientation version stale for {}: {}",
                    instance.owner,
                    path_string(&instance.path)
                ),
            )
            .with_tool(&instance.owner)
            .with_surface(&instance.id())
            .with_path(&instance.path)
            .with_repair(SESSION_CONTEXT_REPAIR_HINT);
            return Ok(SurfaceStatus::with(instance.clone(), STATE_STALE, finding));
        }
        return Ok(SurfaceStatus::healthy(instance.clone()));
    }
    if let OrientationSectionState::InspectionFailed(detail) = &assessment.state {
        return Err(SessionSurfaceFailure::read(&instance.path, detail.clone()));
    }
    if let OrientationSectionState::Undecodable(error) = assessment.state {
        // The reference lets this `UnicodeDecodeError` escape the read-only
        // probe; reporting `unsafe` here was CF28's disclosed divergence, which
        // CF33 corrects. No health verdict is invented for an undecodable file.
        return Err(SessionSurfaceFailure::decode(
            &instance.path,
            error.to_string(),
        ));
    }
    Ok(missing_orientation_status(instance))
}

fn missing_orientation_status(instance: &SurfaceInstance) -> SurfaceStatus {
    let (code, message) = if instance.definition.kind == SurfaceKind::ContextFile {
        (
            CONTEXT_FILE_MISSING,
            format!(
                "Always-on context file missing for {}: {}",
                instance.owner,
                path_string(&instance.path)
            ),
        )
    } else {
        (
            SESSION_PRESENCE_INCOMPLETE,
            format!(
                "Session-presence rule file missing for {}: {}",
                instance.owner,
                path_string(&instance.path)
            ),
        )
    };
    let finding = SurfaceFinding::new(code, SEVERITY_ERROR, message)
        .with_tool(&instance.owner)
        .with_surface(&instance.id())
        .with_path(&instance.path)
        .with_repair(SESSION_CONTEXT_REPAIR_HINT);
    SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
}

fn probe_hook(instance: &SurfaceInstance) -> Result<SurfaceStatus, SessionSurfaceFailure> {
    let Some(project_root) = project_of(instance, CLAUDE_SETTINGS_REL) else {
        return Ok(missing_hook_status(instance));
    };
    let assessment = observe_claude_settings(&project_root);
    match assessment.shape {
        ClaudeSettingsShape::Undecodable => Ok(undecodable_status(instance)),
        ClaudeSettingsShape::Unreadable => {
            // `Path.read_bytes` raised `OSError` and the reference has no catch
            // for it; this must abort the audit, not become "missing".
            Err(SessionSurfaceFailure::read(
                &instance.path,
                assessment
                    .detail
                    .unwrap_or_else(|| format!("could not read {}", path_string(&instance.path))),
            ))
        }
        ClaudeSettingsShape::UncaughtJson => {
            // `json.loads` raised the integer or recursion ceiling, which is not
            // a `JSONDecodeError`; `_load` has no catch, so this aborts the audit
            // instead of reporting the hooks as missing.
            Err(SessionSurfaceFailure::json_limit(
                &instance.path,
                assessment
                    .detail
                    .unwrap_or_else(|| format!("could not parse {}", path_string(&instance.path))),
            ))
        }
        _ if assessment.registered() => Ok(SurfaceStatus::healthy(instance.clone())),
        // Absent file, invalid JSON, a non-object document and a document
        // missing either entry are all "not present" for the reference, which
        // only ever asks `is_registered`.
        _ => Ok(missing_hook_status(instance)),
    }
}

fn missing_hook_status(instance: &SurfaceInstance) -> SurfaceStatus {
    let finding = SurfaceFinding::new(
        SESSION_PRESENCE_INCOMPLETE,
        SEVERITY_ERROR,
        format!(
            "Session-presence hook entry missing for {}: {}",
            instance.owner,
            path_string(&instance.path)
        ),
    )
    .with_tool(&instance.owner)
    .with_surface(&instance.id())
    .with_path(&instance.path)
    .with_repair(SESSION_HOOK_REPAIR_HINT);
    SurfaceStatus::with(instance.clone(), STATE_MISSING, finding)
}

fn undecodable_status(instance: &SurfaceInstance) -> SurfaceStatus {
    let finding = SurfaceFinding::new(
        SESSION_PRESENCE_UNDECODABLE,
        SEVERITY_ERROR,
        format!(
            "undecodable: re-save {} as UTF-8",
            path_string(&instance.path)
        ),
    )
    .with_tool(&instance.owner)
    .with_surface(&instance.id())
    .with_path(&instance.path);
    SurfaceStatus::with(instance.clone(), STATE_UNSAFE, finding)
}
