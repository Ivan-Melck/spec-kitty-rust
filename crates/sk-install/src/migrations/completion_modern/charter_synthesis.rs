//! Source charter synthesis/cutover/provenance migration effects.
//!
//! Six effects that rewrite the compiled charter's provenance, activation and
//! project-pack state, ported from the pinned Source upgrade migrations and the
//! charter-layer helpers they delegate to:
//!
//! | id | effect | family |
//! | --- | --- | --- |
//! | `3.2.7_heal_provenance_paths` | absolute built-in `source_path` → portable token, in both provenance carriers | [`provenance`] |
//! | `4_0_0rc5_heal_template_set_provenance` | the same, for the `template_set` catalog rows 3.2.7 skipped | [`provenance`] |
//! | `4.0.0rc8_project_intent_directive_rename` | legacy `mission-type-scope-directive` → `project-intent-directive` | [`legacy_rename`] |
//! | `4.0.0rc8_promote_interview_selections` | answers-only interview selections → the present activation lists | [`promote`] |
//! | `consolidate_charter_bundle_fold` | fold the retired bundle files + config-embedded activation into `charter.yaml` | [`fold`] |
//! | `charter_pack_cutover` | `.kittify/doctrine/**` → the project pack root, and the retired key/path vocabulary | [`cutover`] |
//!
//! Every effect reuses an existing canonical authority rather than a second one:
//! [`sk_charter::canonical_yaml`] for canonical YAML, [`sk_charter::hash_content`]
//! / [`sk_charter::compute_bundle_content_hash`] for the bundle stamp,
//! [`sk_charter::provenance`] for the path normalizer,
//! [`sk_charter::synthesis`] for the manifest model and its `manifest_hash`,
//! [`sk_doctrine::acquire_synthesis_lock`] for the run-wide synthesis lock,
//! [`sk_doctrine::prepare_ledger_write`] / `apply_ledger_write` for the activation
//! ledger, and [`crate::yaml_edit`] (round-trip CST) for in-place YAML edits so
//! authored comments, quotes and key order survive.
//!
//! The parent registry folds [`IMPLEMENTATIONS`] into the one canonical
//! `NATIVE_MIGRATIONS` list; the ids here must never be duplicated there.

pub mod cutover;
pub mod fold;
pub mod legacy_rename;
pub mod promote;
pub mod provenance;

use crate::upgrade::NativeMigration;

/// Every charter synthesis/cutover/provenance migration implemented here.
pub const IMPLEMENTATIONS: &[NativeMigration] = &[
    provenance::IMPLEMENTATIONS[0],
    provenance::IMPLEMENTATIONS[1],
    legacy_rename::IMPLEMENTATIONS[0],
    promote::IMPLEMENTATIONS[0],
    fold::IMPLEMENTATIONS[0],
    cutover::IMPLEMENTATIONS[0],
];
