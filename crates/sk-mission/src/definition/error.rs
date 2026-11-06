//! Typed failures for canonical Mission definition resolution, plus the exact
//! Source diagnostic rendering.
//!
//! Source raises `MissionError` / `MissionNotFoundError` carrying a rendered
//! message (`mission._format_validation_error`). The message text is part of the
//! operator-visible contract, so it is reconstructed here from structured
//! issues rather than paraphrased: the field path, the pydantic message, the
//! root-field typo hint, and the schema reference are all reproduced.
//!
//! Messages are held as [`PythonString`] because an authored key or value may
//! contain an unpaired surrogate, which Source accepts and embeds verbatim in
//! the message. [`DefinitionError::message_codepoints`] is exact;
//! [`std::fmt::Display`] falls back to Python `repr` escaping for such units
//! instead of silently dropping them.

use std::fmt;
use std::path::{Path, PathBuf};

use sk_kernel::PythonString;

use crate::definition::model::MISSION_ROOT_FIELDS;

/// One component of a pydantic error location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocPart {
    /// A model field or mapping key, rendered as its authored text.
    Field(PythonString),
    /// A list index, rendered as a decimal.
    Index(usize),
    /// pydantic's `[key]` marker for a mapping-key error.
    KeyMarker,
}

/// The pydantic issue kinds `MissionConfig` validation can produce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueKind {
    /// `Field required`
    Missing,
    /// `Extra inputs are not permitted` (`extra="forbid"`)
    Extra,
    /// `Input should be a valid string`
    NotString,
    /// `Input should be a valid list`
    NotList,
    /// `Input should be a valid dictionary`; the model name is appended for a
    /// required object field.
    NotDictionary(Option<&'static str>),
    /// `Input should be a valid boolean, unable to interpret input`
    NotBoolean,
    /// The closed `domain` literal set.
    NotADomain,
    /// `String should match pattern '…'`
    Pattern(&'static str),
    /// `List should have at least N item(s) after validation, not M`
    ListTooShort { min: usize, got: usize },
}

/// One rendered `Detected issues:` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigIssue {
    /// Location components in order.
    pub loc: Vec<LocPart>,
    /// What was wrong.
    pub kind: IssueKind,
}

/// A typed Mission-definition failure. Never a silent default and never a
/// converted success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefinitionError {
    /// No resolver tier has a `mission.yaml` for the name and it is not a
    /// registered mission type (`mission.get_mission_by_name`).
    MissionNotFound {
        /// The requested mission-type name.
        name: PythonString,
        /// Discoverable names, already sorted, as Source lists them.
        available: Vec<PythonString>,
    },
    /// `Mission._load_and_validate_config` reached a directory with no
    /// `mission.yaml`.
    MissionConfigNotFound {
        /// The expected `mission.yaml` path.
        path: PathBuf,
    },
    /// The document is not decodable YAML (`Invalid mission.yaml: {e}`).
    InvalidYaml {
        /// The config file.
        path: PathBuf,
        /// The decoder's message.
        detail: String,
    },
    /// The document decoded but is not a mapping (`mission.yaml` root check).
    NotAMapping {
        /// The config file.
        path: PathBuf,
        /// Python type name of the decoded root value.
        got: &'static str,
    },
    /// `MissionConfig` validation failed; rendering is
    /// `mission._format_validation_error`.
    InvalidConfig {
        /// The config file.
        path: PathBuf,
        /// Issues in pydantic's order.
        issues: Vec<ConfigIssue>,
    },
    /// Reading the file failed.
    ReadFailed {
        /// The path that could not be read.
        path: PathBuf,
        /// Underlying description.
        detail: String,
    },
    /// A filesystem path's text is not UTF-8, so it cannot appear in a
    /// Source-shaped message or a path-convention fact.
    NonUtf8Path {
        /// The offending path.
        path: PathBuf,
    },
    /// An authored path value contains code points a filesystem path cannot
    /// carry here (an unpaired surrogate). Source raises
    /// `UnicodeEncodeError` when it probes such a path; this is the typed form
    /// of that boundary, not a refusal of a working layout.
    UnencodableAuthoredPath {
        /// The authored value.
        value: PythonString,
    },
    /// No Source-faithful YAML safe-load decoder was supplied. The shared
    /// PyYAML dialect primitive is an open cross-cutting gap, so the canonical
    /// loader requires an injected decoder instead of guessing with a different
    /// dialect (see `definition::CORE_SCHEMA_DIVERGENCES`).
    DialectUnavailable {
        /// Which dialect feature/decoder is missing.
        detail: String,
    },
    /// `meta.json` exists but is corrupt or unreadable
    /// (`core.paths.load_meta_fail_closed` raises `MissionMetaReadError`).
    MetaReadFailed {
        /// The `meta.json` that failed.
        path: PathBuf,
        /// Underlying description.
        detail: String,
    },
}

impl DefinitionError {
    /// The exact Source-shaped message, as code points.
    pub fn message_codepoints(&self) -> PythonString {
        match self {
            DefinitionError::MissionNotFound { name, available } => {
                let mut out = PythonString::from_scalars("Mission '");
                out.append(name);
                out.push_scalars("' not found.\nAvailable missions: ");
                if available.is_empty() {
                    out.push_scalars("none");
                } else {
                    let joined = available
                        .iter()
                        .map(PythonString::to_scalar_string)
                        .collect::<Result<Vec<_>, _>>();
                    match joined {
                        Ok(parts) => out.push_scalars(&parts.join(", ")),
                        Err(_) => {
                            for (index, item) in available.iter().enumerate() {
                                if index > 0 {
                                    out.push_scalars(", ");
                                }
                                out.append(item);
                            }
                        }
                    }
                }
                out
            }
            DefinitionError::MissionConfigNotFound { path } => {
                PythonString::from_scalars(&format!(
                    "Mission config not found: {}\nExpected mission.yaml in mission directory",
                    path.display()
                ))
            }
            DefinitionError::InvalidYaml { path: _, detail } => {
                PythonString::from_scalars(&format!("Invalid mission.yaml: {detail}"))
            }
            DefinitionError::NotAMapping { path, got } => PythonString::from_scalars(&format!(
                "Mission config must be a mapping/dictionary in {}, got {got} instead.",
                path.display()
            )),
            DefinitionError::InvalidConfig { path, issues } => render_issues(path, issues),
            DefinitionError::ReadFailed { path, detail } => {
                PythonString::from_scalars(&format!("Cannot read {}: {detail}", path.display()))
            }
            DefinitionError::NonUtf8Path { path } => PythonString::from_scalars(&format!(
                "Path is not valid UTF-8 and has no Source-shaped text: {}",
                path.display()
            )),
            DefinitionError::UnencodableAuthoredPath { value } => {
                let mut out = PythonString::from_scalars(
                    "Authored path contains code points no filesystem path can carry here (unpaired surrogate): ",
                );
                out.append(value);
                out
            }
            DefinitionError::DialectUnavailable { detail } => PythonString::from_scalars(&format!(
                "No Source-faithful YAML safe-load decoder: {detail}"
            )),
            DefinitionError::MetaReadFailed { path, detail } => {
                PythonString::from_scalars(&format!(
                    "Cannot read {}: {detail} — fail-closed (meta.json exists but is corrupt or unreadable)",
                    path.display()
                ))
            }
        }
    }

    /// The message as a Rust `String`; authored surrogate text is rendered with
    /// Python `repr` escaping rather than dropped.
    pub fn message(&self) -> String {
        let codepoints = self.message_codepoints();
        match codepoints.to_scalar_string() {
            Ok(text) => text,
            Err(_) => escape_codepoints(&codepoints),
        }
    }
}

/// Render code points with Python `repr`-style `\uXXXX` / `\UXXXXXXXX`
/// escapes for every unpaired surrogate, keeping every other code point as
/// authored. Used only where a `String` cannot carry the value exactly.
pub(crate) fn escape_codepoints(codepoints: &PythonString) -> String {
    let mut text = String::new();
    for &code in codepoints.codepoints() {
        match char::from_u32(code) {
            Some(character) => text.push(character),
            None if code <= 0xffff => text.push_str(&format!("\\u{code:04x}")),
            None => text.push_str(&format!("\\U{code:08x}")),
        }
    }
    text
}

/// Render `mission._format_validation_error`.
fn render_issues(path: &Path, issues: &[ConfigIssue]) -> PythonString {
    let mut out = PythonString::from_scalars(&format!(
        "Invalid mission configuration in {}:\n\nDetected issues:",
        path.display()
    ));
    for issue in issues {
        out.push_scalars("\n- ");
        out.append(&render_loc(&issue.loc));
        out.push_scalars(": ");
        out.append(&render_kind(issue));
    }
    out.push_scalars(
        "\n\nRefer to kitty-specs/005-refactor-mission-system/data-model.md for the schema definition.",
    );
    out
}

/// `" -> ".join(str(part) for part in loc)`, or `<root>`.
pub fn render_loc(loc: &[LocPart]) -> PythonString {
    if loc.is_empty() {
        return PythonString::from_scalars("<root>");
    }
    let mut out = PythonString::from_scalars("");
    for (index, part) in loc.iter().enumerate() {
        if index > 0 {
            out.push_scalars(" -> ");
        }
        match part {
            LocPart::Field(name) => out.append(name),
            LocPart::Index(position) => out.push_scalars(&position.to_string()),
            LocPart::KeyMarker => out.push_scalars("[key]"),
        }
    }
    out
}

fn render_kind(issue: &ConfigIssue) -> PythonString {
    let base = match &issue.kind {
        IssueKind::Missing => "Field required".to_owned(),
        IssueKind::Extra => {
            let mut message = "Extra inputs are not permitted".to_owned();
            if issue.loc.len() == 1 {
                message.push_str(&format!(
                    " (check for typos; valid root fields: {})",
                    MISSION_ROOT_FIELDS.join(", ")
                ));
            }
            message
        }
        IssueKind::NotString => "Input should be a valid string".to_owned(),
        IssueKind::NotList => "Input should be a valid list".to_owned(),
        IssueKind::NotDictionary(None) => "Input should be a valid dictionary".to_owned(),
        IssueKind::NotDictionary(Some(model)) => {
            format!("Input should be a valid dictionary or instance of {model}")
        }
        IssueKind::NotBoolean => {
            "Input should be a valid boolean, unable to interpret input".to_owned()
        }
        IssueKind::NotADomain => {
            "Input should be 'software', 'research', 'writing', 'seo' or 'other'".to_owned()
        }
        IssueKind::Pattern(pattern) => format!("String should match pattern '{pattern}'"),
        IssueKind::ListTooShort { min, got } => format!(
            "List should have at least {min} item{} after validation, not {got}",
            if *min == 1 { "" } else { "s" }
        ),
    };
    PythonString::from_scalars(&base)
}

impl fmt::Display for DefinitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message())
    }
}

impl std::error::Error for DefinitionError {}
