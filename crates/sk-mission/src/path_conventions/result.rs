//! The `PathValidationResult` contract and its exact Source rendering.
//!
//! Field set, field order, the `is_valid` predicate, and both rendered texts are
//! Source `specify_cli.validators.paths`. The two texts are operator-facing
//! contract (they are what a blocked `accept` prints), so they are reproduced
//! character for character, including the `--lenient` sentence variant, the
//! project-remap example block, and the `Required Actions:` list.

use sk_kernel::PythonString;

use crate::definition::value::OrderedMap;

/// The token Source falls back to when nothing more specific is rendered.
pub const PATH_CONVENTIONS_NOT_SATISFIED: &str = "Path conventions not satisfied.";

/// Result of validating a Mission's declared paths against the workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathValidationResult {
    /// The Mission's display name, as Source's `mission.name`.
    pub mission_name: PythonString,
    /// Declared (post-remap, post-prefix) required paths, in declared order.
    pub required_paths: OrderedMap<PythonString>,
    /// Required paths that exist, in declared order.
    pub existing_paths: Vec<PythonString>,
    /// Required paths that do not exist, in declared order.
    pub missing_paths: Vec<PythonString>,
    /// Normalized artifact tokens for missing paths that are declared artifacts.
    pub missing_artifact_tokens: Vec<PythonString>,
    /// One warning per missing path, in declared order.
    pub warnings: Vec<PythonString>,
    /// Remediation hints (empty when nothing is missing).
    pub suggestions: Vec<PythonString>,
    /// Declared path -> candidate source root that satisfied it.
    pub satisfied_by: OrderedMap<PythonString>,
    /// Missing keys a project `path_conventions` override could remap.
    pub remappable_missing_keys: Vec<PythonString>,
    /// Mission-type slug for the project-override route, when derivable.
    pub mission_type: Option<PythonString>,
}

impl PathValidationResult {
    /// True when every required path exists.
    pub fn is_valid(&self) -> bool {
        self.missing_paths.is_empty()
    }

    /// Source `format_warnings`; empty when there are no warnings.
    pub fn format_warnings(&self) -> PythonString {
        if self.warnings.is_empty() {
            return PythonString::from_scalars("");
        }
        let mut out = PythonString::from_scalars("Path Convention Warnings:");
        for warning in &self.warnings {
            out.push_scalars("\n  - ");
            out.append(warning);
        }
        if !self.suggestions.is_empty() {
            out.push_scalars("\n\nSuggestions:");
            for suggestion in &self.suggestions {
                out.push_scalars("\n  - ");
                out.append(suggestion);
            }
        }
        out
    }

    /// Source `format_errors`; empty when the result is valid.
    pub fn format_errors(&self) -> PythonString {
        if self.is_valid() {
            return PythonString::from_scalars("");
        }
        let mut out = PythonString::from_scalars("Path Convention Errors:");
        for warning in &self.warnings {
            out.push_scalars("\n  - ");
            out.append(warning);
        }
        out.push_scalars("\n\nRun `accept --lenient` to treat these as warnings instead of blocking errors for this mission run");
        if self.suggestions.is_empty() {
            out.push_scalars(".");
        } else {
            out.push_scalars(" - or, if you want to adopt the convention, see the commands below:");
        }
        if !self.remappable_missing_keys.is_empty() {
            out.push_scalars(
                "\n\nIf this project's real layout differs from these conventions, remap them in \
                 `.kittify/config.yaml` via `project.path_conventions` (see ADR \
                 project-path-convention-override-precedes-doctrine):\n\n  project:\n    \
                 path_conventions:",
            );
            for key in &self.remappable_missing_keys {
                out.push_scalars("\n      ");
                out.append(key);
                out.push_scalars(": <real-directory>/  # where this project keeps its ");
                out.append(key);
            }
            out.push_scalars(
                "\n\nA key this project has no directory for can instead be dropped by copying the \
                 packaged mission.yaml to `.kittify/overrides/missions/",
            );
            match &self.mission_type {
                Some(slug) => out.append(slug),
                None => out.push_scalars("<mission-type>"),
            }
            out.push_scalars("/mission.yaml` and removing that key's line.");
        }
        if !self.suggestions.is_empty() {
            out.push_scalars("\n\nRequired Actions:");
            for suggestion in &self.suggestions {
                out.push_scalars("\n  - ");
                out.append(suggestion);
            }
        }
        out
    }

    /// The strict-failure text, with Source's fallback applied.
    pub fn error_text(&self) -> PythonString {
        let rendered = self.format_errors();
        if rendered.is_empty() {
            PythonString::from_scalars(PATH_CONVENTIONS_NOT_SATISFIED)
        } else {
            rendered
        }
    }

    /// The lenient-warning text, with Source's fallback applied.
    pub fn warning_text(&self) -> PythonString {
        let rendered = self.format_warnings();
        if rendered.is_empty() {
            PythonString::from_scalars(PATH_CONVENTIONS_NOT_SATISFIED)
        } else {
            rendered
        }
    }
}

/// A strict path-convention refusal (Source `PathValidationError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathConventionRefusal {
    /// The failing result.
    pub result: PathValidationResult,
}

impl PathConventionRefusal {
    /// The rendered message Source raises with.
    pub fn message(&self) -> PythonString {
        self.result.error_text()
    }
}

impl std::fmt::Display for PathConventionRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&crate::definition::error::escape_codepoints(
            &self.result.error_text(),
        ))
    }
}

impl std::error::Error for PathConventionRefusal {}
