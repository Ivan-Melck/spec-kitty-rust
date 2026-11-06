//! Canonical Mission *definition*: the resolved `mission.yaml` configuration and
//! its resolution service.
//!
//! # What a definition is (and is not)
//!
//! Source keeps three different things apart, and so does this crate:
//!
//! * [`crate::Mission`] — a Mission **instance** discovered under
//!   `kitty-specs/<slug>/`, identified by its `mission_id`. That is planning
//!   identity, not configuration.
//! * [`crate::MissionTemplates`] / [`crate::CanonicalTemplate`] — the canonical
//!   Markdown templates a Mission's artifacts are authored from.
//! * this module's [`MissionDefinition`] — the `mission.yaml` schema
//!   (`MissionConfig` in Source): display identity, `domain`, declared required
//!   and optional artifacts, declared `paths`, validation settings, and the
//!   optional command/task-type/tool metadata.
//!
//! A definition is never read from `meta.json`, and `meta.json` is never treated
//! as configuration: acceptance's declared artifacts and path conventions come
//! from the resolved `mission.yaml` only.
//!
//! # Resolution
//!
//! [`resolve_mission_definition`] reproduces Source `get_mission_by_name`:
//! project override → legacy project → org pack → global runtime cache (with the
//! built-in shadow guard) → packaged built-in (kept on the legacy
//! `specify_cli/missions` source, not `packs/built-in/missions`), then the
//! neutral identity for an activated sparse `mission_types/<id>.yaml`. The org
//! chain, packaged location, global home and registration roster are provider
//! ports ([`DefinitionProviders`]) owned by other surfaces.
//!
//! # Dialect
//!
//! Source decodes `mission.yaml` with PyYAML `safe_load` (YAML 1.1, Python
//! values). The canonical loader therefore takes a [`SafeDocumentDecoder`]; this
//! crate ships [`PyYamlSafe`] (that exact dialect, backed by the shared Kernel
//! decoder), [`RuamelSafe`] for the project `path_conventions` override, and
//! [`CoreSchemaYaml`] only as an explicitly-divergent 1.2-core cross-check.
//! [`require_source_faithful`] returns the decoder's named open cases, so a
//! caller never inherits an unqualified "faithful" claim.
//!
//! # Scope
//!
//! Read-only. Nothing here writes, migrates, backfills, or reserializes authored
//! YAML, and nothing here decides acceptance: it supplies the declared facts that
//! acceptance authority needs.

pub(crate) mod dialect;
pub(crate) mod error;
pub(crate) mod model;
pub(crate) mod resolve;
pub(crate) mod value;

pub use dialect::{
    CORE_SCHEMA_DIVERGENCES, CoreSchemaYaml, DialectFidelity, PyYamlSafe, RuamelSafe,
    SafeDocumentDecoder, require_source_faithful,
};
pub use error::{ConfigIssue, DefinitionError, IssueKind, LocPart};
pub use model::{
    LoadedDefinition, MISSION_COMPAT_IGNORED_FIELDS, MISSION_DOMAINS, MISSION_ROOT_FIELDS,
    MissionArtifacts, MissionCommand, MissionDefinition, MissionDomain, MissionMcpTools,
    MissionTaskMetadata, MissionTaskType, MissionValidation, MissionWorkflow, VALID_PATH_KEYS,
    VERSION_PATTERN, WorkflowPhase, repr_list,
};
pub use resolve::{
    CheckoutProviders, DEFAULT_MISSION_CONFIG_BYTE_LIMIT, DefinitionOrigin, DefinitionProviders,
    ProjectOnlyProviders, RegisteredMissionType, ResolutionTier, ResolvedMissionDefinition,
    available_missions, load_mission_definition, recorded_mission_type, resolve_mission_definition,
    resolve_mission_definition_for_mission_dir,
};
pub use value::{OrderedMap, SourceValue};
