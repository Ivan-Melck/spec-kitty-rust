//! Canonical Mission templates injected by the caller.
//!
//! Rust does not vendor, fetch, or invent spec/plan/tasks/work-package prose.
//! The charter/template resolver (owned by `sk-charter` and the install layer)
//! resolves the canonical pack template, and the caller injects its bytes here.
//! This module only validates that a template is present, records its
//! provenance, and performs the canonical placeholder substitution the Python
//! task-prompt template expects.

use std::fmt;
use std::path::{Path, PathBuf};

use sk_kernel::WpId;
use thiserror::Error;

use crate::yaml_patch::{decode_mapping, sequence_of, set_top_level_sequence};

/// The canonical planning artifact a template produces.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MissionTemplateKind {
    /// `spec.md`
    Spec,
    /// `plan.md`
    Plan,
    /// `tasks.md`
    Tasks,
    /// One `tasks/WP*.md` prompt
    WorkPackage,
}

impl MissionTemplateKind {
    /// Every template kind.
    pub const ALL: [Self; 4] = [Self::Spec, Self::Plan, Self::Tasks, Self::WorkPackage];

    /// Operator-facing token.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spec => "spec",
            Self::Plan => "plan",
            Self::Tasks => "tasks",
            Self::WorkPackage => "work_package",
        }
    }
}

impl fmt::Display for MissionTemplateKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why an injected canonical template cannot be used.
#[derive(Debug, Error)]
pub enum TemplateError {
    /// The resolver-supplied template id is empty.
    #[error("canonical {kind} template has an empty resolver id")]
    EmptyId { kind: MissionTemplateKind },

    /// The resolved template body is empty.
    #[error("canonical {kind} template {id:?} has no content")]
    EmptyContent {
        kind: MissionTemplateKind,
        id: String,
    },

    /// The caller did not inject the required canonical template.
    #[error("no canonical {kind} template was supplied")]
    Missing { kind: MissionTemplateKind },

    /// A template was used for the wrong artifact kind.
    #[error("canonical template {id:?} is a {actual} template, not a {expected} template")]
    MismatchedKind {
        id: String,
        expected: MissionTemplateKind,
        actual: MissionTemplateKind,
    },

    /// A placeholder has no supplied value.
    #[error("canonical {kind} template {id:?} still contains placeholder {placeholder:?}")]
    UnrenderedPlaceholder {
        kind: MissionTemplateKind,
        id: String,
        placeholder: String,
    },

    /// The supplied work-package metadata is invalid.
    #[error("cannot render work package template: {detail}")]
    InvalidContext { detail: String },

    /// A format-preserving YAML edit failed.
    #[error("cannot render work package template: {detail}")]
    Patch { detail: String },

    /// The rendered document did not decode to the expected metadata.
    #[error("rendered work package template is invalid: {detail}")]
    InvalidRendered { detail: String },
}

/// One caller-supplied canonical template with its resolver provenance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalTemplate {
    kind: MissionTemplateKind,
    id: String,
    source: PathBuf,
    content: String,
}

impl CanonicalTemplate {
    /// Validate and wrap a resolved canonical template.
    pub fn new(
        kind: MissionTemplateKind,
        id: impl Into<String>,
        source: impl Into<PathBuf>,
        content: impl Into<String>,
    ) -> Result<Self, TemplateError> {
        let id = id.into();
        if id.trim().is_empty() {
            return Err(TemplateError::EmptyId { kind });
        }
        let content = content.into();
        if content.trim().is_empty() {
            return Err(TemplateError::EmptyContent { kind, id });
        }
        Ok(Self {
            kind,
            id,
            source: source.into(),
            content,
        })
    }

    /// The artifact kind this template produces.
    pub fn kind(&self) -> MissionTemplateKind {
        self.kind
    }

    /// Resolver id (for example `research/spec-template.md`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Where the resolver read the template from.
    pub fn source(&self) -> &Path {
        &self.source
    }

    /// The canonical template body exactly as resolved.
    pub fn content(&self) -> &str {
        &self.content
    }
}

/// All canonical templates a Mission authoring operation may need.
///
/// Creation needs only [`MissionTemplates::spec`]. Plan/tasks/work-package
/// authoring requires the corresponding template and fails closed when it is
/// absent rather than substituting invented prose.
#[derive(Clone, Debug)]
pub struct MissionTemplates {
    spec: CanonicalTemplate,
    plan: Option<CanonicalTemplate>,
    tasks: Option<CanonicalTemplate>,
    work_package: Option<CanonicalTemplate>,
}

impl MissionTemplates {
    /// Start with the mandatory spec template.
    pub fn new(spec: CanonicalTemplate) -> Self {
        Self {
            spec,
            plan: None,
            tasks: None,
            work_package: None,
        }
    }

    /// Add the plan template.
    pub fn with_plan(mut self, template: CanonicalTemplate) -> Self {
        self.plan = Some(template);
        self
    }

    /// Add the tasks-outline template.
    pub fn with_tasks(mut self, template: CanonicalTemplate) -> Self {
        self.tasks = Some(template);
        self
    }

    /// Add the work-package prompt template.
    pub fn with_work_package(mut self, template: CanonicalTemplate) -> Self {
        self.work_package = Some(template);
        self
    }

    /// The mandatory spec template.
    pub fn spec(&self) -> &CanonicalTemplate {
        &self.spec
    }

    /// The requested template, or a typed missing-template refusal.
    pub fn require(&self, kind: MissionTemplateKind) -> Result<&CanonicalTemplate, TemplateError> {
        let template = match kind {
            MissionTemplateKind::Spec => Some(&self.spec),
            MissionTemplateKind::Plan => self.plan.as_ref(),
            MissionTemplateKind::Tasks => self.tasks.as_ref(),
            MissionTemplateKind::WorkPackage => self.work_package.as_ref(),
        };
        template.ok_or(TemplateError::Missing { kind })
    }
}

/// Metadata used to render one canonical work-package prompt template.
#[derive(Clone, Debug)]
pub struct WorkPackageTemplateContext {
    work_package_id: WpId,
    title: String,
    planning_base_branch: String,
    merge_target_branch: String,
    timestamp: String,
    phase: String,
    dependencies: Vec<WpId>,
    subtasks: Vec<String>,
}

impl WorkPackageTemplateContext {
    /// Build a context with the identity and branch fields every template uses.
    pub fn new(
        work_package_id: WpId,
        title: impl Into<String>,
        planning_base_branch: impl Into<String>,
        merge_target_branch: impl Into<String>,
        timestamp: impl Into<String>,
    ) -> Self {
        Self {
            work_package_id,
            title: title.into(),
            planning_base_branch: planning_base_branch.into(),
            merge_target_branch: merge_target_branch.into(),
            timestamp: timestamp.into(),
            phase: String::new(),
            dependencies: Vec::new(),
            subtasks: Vec::new(),
        }
    }

    /// Set the phase label.
    pub fn with_phase(mut self, phase: impl Into<String>) -> Self {
        self.phase = phase.into();
        self
    }

    /// Set declared dependencies.
    pub fn with_dependencies(mut self, dependencies: impl IntoIterator<Item = WpId>) -> Self {
        self.dependencies = dependencies.into_iter().collect();
        self
    }

    /// Set the subtask ids.
    pub fn with_subtasks(mut self, subtasks: impl IntoIterator<Item = String>) -> Self {
        self.subtasks = subtasks.into_iter().collect();
        self
    }

    /// The canonical branch-strategy sentence stored in WP prompt frontmatter.
    pub fn branch_strategy(&self) -> String {
        format!(
            "Planning artifacts were generated on {}; completed changes must merge back into {}.",
            self.planning_base_branch, self.merge_target_branch
        )
    }

    /// Render `template` for this context.
    ///
    /// Only the canonical placeholders and the canonical template's own
    /// `WPxx`/`Replace with…`/`Txxx` scaffolding are substituted. Every other
    /// byte, including comments and body prose, comes from the injected
    /// template.
    pub fn render(&self, template: &CanonicalTemplate) -> Result<String, TemplateError> {
        if template.kind != MissionTemplateKind::WorkPackage {
            return Err(TemplateError::MismatchedKind {
                id: template.id.clone(),
                expected: MissionTemplateKind::WorkPackage,
                actual: template.kind,
            });
        }
        if self.title.trim().is_empty() {
            return Err(TemplateError::InvalidContext {
                detail: "work package title is empty".to_owned(),
            });
        }
        if self.planning_base_branch.trim().is_empty() || self.merge_target_branch.trim().is_empty()
        {
            return Err(TemplateError::InvalidContext {
                detail: "planning and merge target branches are required".to_owned(),
            });
        }
        if self.timestamp.trim().is_empty() {
            return Err(TemplateError::InvalidContext {
                detail: "a creation timestamp is required".to_owned(),
            });
        }

        let branch_strategy = self.branch_strategy();
        let mut rendered = template.content.clone();
        for (placeholder, value) in [
            ("{{work_package_id}}", self.work_package_id.as_str()),
            ("{{title}}", &self.title),
            (
                "{{planning_base_branch}}",
                self.planning_base_branch.as_str(),
            ),
            ("{{merge_target_branch}}", self.merge_target_branch.as_str()),
            ("{{branch_strategy}}", &branch_strategy),
            ("{{TIMESTAMP}}", &self.timestamp),
        ] {
            rendered = rendered.replace(placeholder, value);
        }

        // Canonical template scaffolding, not invented content: the reference
        // templates use these literal placeholders.
        rendered = rendered.replace(
            "work_package_id: \"WPxx\"",
            &format!("work_package_id: \"{}\"", self.work_package_id),
        );
        rendered = rendered.replace(
            "Replace with work package title",
            &yaml_inner_escape(&self.title),
        );
        let phase = if self.phase.trim().is_empty() {
            "Phase 1".to_owned()
        } else {
            self.phase.clone()
        };
        rendered = rendered.replace(
            "Phase N - Replace with phase name",
            &yaml_inner_escape(&phase),
        );

        if let Some(placeholder) = residual_placeholder(&rendered) {
            return Err(TemplateError::UnrenderedPlaceholder {
                kind: template.kind,
                id: template.id.clone(),
                placeholder,
            });
        }

        let dependencies: Vec<String> = self
            .dependencies
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        if let Some(updated) =
            set_top_level_sequence(&rendered, "dependencies", &dependencies).map_err(patch_error)?
        {
            rendered = updated;
        }
        if let Some(updated) =
            set_top_level_sequence(&rendered, "subtasks", &self.subtasks).map_err(patch_error)?
        {
            rendered = updated;
        }

        self.verify_rendered(&rendered)?;
        Ok(rendered)
    }

    fn verify_rendered(&self, rendered: &str) -> Result<(), TemplateError> {
        let frontmatter = crate::frontmatter::split_frontmatter(rendered).map_err(|error| {
            TemplateError::InvalidRendered {
                detail: error.to_string(),
            }
        })?;
        let mapping = decode_mapping(frontmatter.yaml).map_err(patch_error)?;
        let id = mapping
            .get(serde_yaml_ng::Value::String("work_package_id".to_owned()))
            .and_then(serde_yaml_ng::Value::as_str);
        if id != Some(self.work_package_id.as_str()) {
            return Err(TemplateError::InvalidRendered {
                detail: format!(
                    "work_package_id rendered as {id:?}, expected {:?}",
                    self.work_package_id.as_str()
                ),
            });
        }
        let dependencies = sequence_of(&mapping, "dependencies")
            .map_err(patch_error)?
            .unwrap_or_default();
        let expected: Vec<String> = self
            .dependencies
            .iter()
            .map(|id| id.as_str().to_owned())
            .collect();
        if dependencies != expected {
            return Err(TemplateError::InvalidRendered {
                detail: "dependencies did not render as declared".to_owned(),
            });
        }
        Ok(())
    }
}

fn yaml_inner_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn residual_placeholder(rendered: &str) -> Option<String> {
    let start = rendered.find("{{")?;
    let end = rendered[start + 2..].find("}}")? + start + 4;
    Some(rendered[start..end].to_owned())
}

fn patch_error(error: crate::yaml_patch::PatchError) -> TemplateError {
    TemplateError::Patch {
        detail: error.to_string(),
    }
}

/// Canonical `tasks/README.md` body, rendered with the planning branch.
///
/// Source: the pinned Python reference
/// (`specify_cli/core/mission_creation_scaffold.py::TASKS_README_TEMPLATE`).
/// The content is not invented here: `tests/fixtures/oracle/scaffold/` holds the
/// oracle rendering this constant must reproduce byte-for-byte.
const TASKS_README_TEMPLATE: &str = r#"# Tasks Directory

This directory contains work package (WP) prompt files.

## Directory Structure (v0.9.0+)

```
tasks/
├── WP01-setup-infrastructure.md
├── WP02-user-authentication.md
├── WP03-api-endpoints.md
└── README.md
```

All WP files are stored flat in `tasks/`. Status is tracked in `status.events.jsonl`, not in WP frontmatter.

## Work Package File Format

Each WP file **MUST** use YAML frontmatter:

```yaml
---
work_package_id: "WP01"
title: "Work Package Title"
dependencies: []
planning_base_branch: "{planning_branch}"
merge_target_branch: "{planning_branch}"
branch_strategy: "Planning artifacts were generated on {planning_branch}; completed changes must merge back into {planning_branch}."
subtasks:
  - "T001"
  - "T002"
phase: "Phase 1 - Setup"
assignee: ""
agent: ""
shell_pid: ""
history:
  - timestamp: "2025-01-01T00:00:00Z"
    agent: "system"
    action: "Prompt generated via /spec-kitty.tasks"
---

# Work Package Prompt: WP01 – Work Package Title

[Content follows...]
```

## Status Tracking

Status is tracked via the canonical event log (`status.events.jsonl`), not in WP frontmatter.
Use `spec-kitty agent tasks move-task` to change WP status:

```bash
spec-kitty agent tasks move-task <WPID> --to <lane>
```

Example:
```bash
spec-kitty agent tasks move-task WP01 --to doing
```

## File Naming

- Format: `WP01-kebab-case-slug.md`
- Examples: `WP01-setup-infrastructure.md`, `WP02-user-auth.md`
"#;

/// Render the canonical `tasks/README.md` for `planning_branch`.
pub fn render_tasks_readme(planning_branch: &str) -> String {
    TASKS_README_TEMPLATE.replace("{planning_branch}", planning_branch)
}
