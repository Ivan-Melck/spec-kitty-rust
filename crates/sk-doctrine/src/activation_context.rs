//! Canonical activation selector vocabulary and the typed-org admission gate.
//!
//! Pinned reference: `src/charter/activation/activations.py` at Source
//! `9aed3a8012cb5eb22c0128b14d525039f95fea7a`. This module is the ONE Native home
//! of the reference's `ALLOWED_ACTIONS`, `REGISTERED_TRIGGERS`,
//! `_ACTION_WILDCARDS` and `ALLOWED_MISSION_TYPES`, plus the
//! `ActivationEntry._validate_context` / `resolve_for_context` facts.
//!
//! # Two action vocabularies, deliberately distinct
//!
//! * [`ALLOWED_ACTIONS`] — the ten mission verbs and charter-loop verbs the
//!   prompt builder emits as action labels (the reference's `ALLOWED_ACTIONS`).
//! * [`REGISTERED_TRIGGERS`] — the strict superset an operator may write in
//!   `activation_context.action`: [`ALLOWED_ACTIONS`] plus the four fine-grained
//!   sub-action tokens in [`FINE_GRAINED_TRIGGERS`] (`write_comment`,
//!   `write_docstring`, `rename_identifier`, `add_dependency`).
//!
//! The fine-grained tokens are ABSENT from the agent bootstrap action list, so
//! [`crate::BOOTSTRAP_ACTIONS`] is **not** this authority: using it would refuse
//! the operator-authorable `action: write_comment` the reference admits.
//!
//! # The mission roster is derived, never copied
//!
//! The reference's `ALLOWED_MISSION_TYPES` is
//! `builtin_mission_type_id_set() | {"any", "generic"}` — the ids of the
//! `mission_types/*.yaml` documents under the **built-in** missions root (not the
//! project or org layers). This module reads that same roster through the
//! existing Native authorities — the canonical built-in pack search and the
//! schema-validating [`crate::scan_mission_types_dir`] — so a mission type shipped
//! in the pinned packs is admitted without editing this file, and no hardcoded
//! known-three list is introduced.
//!
//! # Exact matching
//!
//! Membership is code-point equality, exactly like the reference's `in` on a
//! `frozenset[str]`: there is no strip, case-fold or alias step, so `"
//! implement"`, `"Implement"` and `""` are refused, and a lone-surrogate token
//! can never equal an ASCII vocabulary word.
//!
//! # Failure phase
//!
//! A rejection is an **invalid pack document** (the reference raises
//! `ValidationError` from a model validator), so the typed fold skips that pack
//! while the raw per-field readers stay ungated. See
//! [`crate::load_org_charter_policy`]'s activation-context branch and
//! `org_charter::fold`'s skip decision.

use std::path::{Path, PathBuf};

use sk_kernel::python_repr;

use crate::{DoctrineError, PythonString};

/// The reference's wildcard tokens, accepted on either `activation_context` slot.
pub const ACTION_WILDCARDS: [&str; 2] = ["any", "generic"];

/// The reference's `ALLOWED_ACTIONS`: the mission verbs and charter-loop verbs.
///
/// Sorted, so the refusal message (and [`REGISTERED_TRIGGERS`]) lists them in the
/// reference's `sorted(...)` order.
pub const ALLOWED_ACTIONS: [&str; 10] = [
    "accept",
    "charter.context",
    "charter.generate",
    "charter.interview",
    "implement",
    "merge",
    "plan",
    "review",
    "specify",
    "tasks",
];

/// `REGISTERED_TRIGGERS - ALLOWED_ACTIONS`: the fine-grained sub-action tokens.
///
/// They may occur during any mission verb, so the reference's resolver treats
/// them as wildcards for matching ([`matches_context`]) while the validator
/// accepts them as authored values.
pub const FINE_GRAINED_TRIGGERS: [&str; 4] = [
    "add_dependency",
    "rename_identifier",
    "write_comment",
    "write_docstring",
];

/// The reference's `REGISTERED_TRIGGERS`: `ALLOWED_ACTIONS | {fine-grained}`.
///
/// The union formula is the authority (see the module tests); this constant is
/// its flattened, sorted form so callers need no set allocation.
pub const REGISTERED_TRIGGERS: [&str; 14] = [
    "accept",
    "add_dependency",
    "charter.context",
    "charter.generate",
    "charter.interview",
    "implement",
    "merge",
    "plan",
    "rename_identifier",
    "review",
    "specify",
    "tasks",
    "write_comment",
    "write_docstring",
];

/// The `activation_context` key carrying the mission selector.
const MISSION_TYPE_KEY: &str = "mission_type";

/// The `activation_context` key carrying the action selector.
const ACTION_KEY: &str = "action";

/// The built-in pack tier's missions directory (the reference's
/// `built_in_missions_root()`; the same layout `crate::offering` scans).
const BUILT_IN_MISSIONS: &str = "missions";

/// The mission-type roster directory inside the missions root.
const BUILT_IN_MISSION_TYPES: &str = "mission_types";

/// The admitted `activation_context.mission_type` roster plus the wildcards.
///
/// Built once from the built-in mission-type documents the reference derives
/// `ALLOWED_MISSION_TYPES` from; every consumer borrows this instance instead of
/// re-reading or re-declaring the roster.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationSelectorRegistry {
    /// The built-in mission-type ids plus the wildcard tokens, sorted by code
    /// point (the reference's `sorted(ALLOWED_MISSION_TYPES)`).
    mission_types: Vec<PythonString>,
}

impl ActivationSelectorRegistry {
    /// Build the registry from the canonical built-in pack tier.
    ///
    /// The root comes from the one built-in pack search
    /// (`SPEC_KITTY_PACKS_ROOT`, else the packaged anchor), exactly like the
    /// reference's built-in missions root.
    ///
    /// # Errors
    ///
    /// [`DoctrineError::PackRootUnavailable`] when the built-in tier cannot be
    /// resolved; [`DoctrineError::MissingPinnedContent`] when the tier has no
    /// `missions` root; [`DoctrineError::InvalidGraphDocument`] when a
    /// mission-type document fails the shared schema admission. All three
    /// propagate through the typed fold (they are not "skip this pack"
    /// failures), so a broken roster can never silently drop an org policy.
    pub fn canonical() -> Result<Self, DoctrineError> {
        let built_in = crate::org_charter::built_in_search(Path::new(""))
            .find_root()
            .map_err(|_| DoctrineError::PackRootUnavailable { tier: "built-in" })?;
        Self::from_built_in_root(&built_in)
    }

    /// Build the registry from an explicit built-in pack root (`<packs>/built-in`).
    ///
    /// # Errors
    ///
    /// As [`ActivationSelectorRegistry::canonical`], minus the root resolution.
    pub fn from_built_in_root(built_in: &Path) -> Result<Self, DoctrineError> {
        let missions = built_in.join(BUILT_IN_MISSIONS);
        if !missions.is_dir() {
            return Err(DoctrineError::MissingPinnedContent { path: missions });
        }
        let directory = missions.join(BUILT_IN_MISSION_TYPES);
        let ids = crate::offering::scan_mission_types_dir(&directory).map_err(|error| {
            // No shared roster-level carrier exists and this lane must not add a
            // global error variant; this variant's rendering is the neutral
            // `<path>: <message>` the shared scan already produced.
            DoctrineError::InvalidGraphDocument {
                path: directory.clone(),
                message: error.message().to_owned(),
            }
        })?;
        let mut mission_types = ids;
        for wildcard in ACTION_WILDCARDS {
            mission_types.push(PythonString::from_scalars(wildcard));
        }
        mission_types.sort();
        mission_types.dedup();
        Ok(Self { mission_types })
    }

    /// The admitted mission selectors, sorted by code point (wildcards included).
    #[must_use]
    pub fn allowed_mission_types(&self) -> &[PythonString] {
        &self.mission_types
    }

    /// Whether `token` is one of the admitted mission selectors, exactly.
    #[must_use]
    pub fn admits_mission_type(&self, token: &PythonString) -> bool {
        self.mission_types
            .iter()
            .any(|candidate| candidate == token)
    }

    /// Validate the two closed vocabularies of one `activation_context`.
    ///
    /// Mirrors the reference's `ActivationEntry._validate_context`: it looks only
    /// at the `mission_type` and `action` slots (any other key is carried
    /// verbatim, as the reference's `dict[str, str]` does), checks `mission_type`
    /// first, and rejects the first bad value.
    ///
    /// # Errors
    ///
    /// [`ActivationContextRejection`] naming the offending slot and value; the
    /// caller renders it as the pack's `InvalidOrgCharter` (the reference's
    /// skip phase).
    pub fn validate_context(
        &self,
        context: &[(PythonString, PythonString)],
    ) -> Result<(), ActivationContextRejection> {
        if let Some(declared) = last_declared(context, MISSION_TYPE_KEY)
            && !self.admits_mission_type(declared)
        {
            return Err(ActivationContextRejection::MissionType(declared.clone()));
        }
        if let Some(declared) = last_declared(context, ACTION_KEY)
            && !admits_action(declared)
        {
            return Err(ActivationContextRejection::Action(declared.clone()));
        }
        Ok(())
    }

    /// The admitted mission selectors as the reference renders `sorted(...)`.
    fn mission_type_listing(&self) -> String {
        python_listing(self.mission_types.iter().map(PythonString::repr))
    }
}

/// Whether `token` is an admitted `activation_context.action` value, exactly.
///
/// Admitted values are [`REGISTERED_TRIGGERS`] and the wildcards
/// [`ACTION_WILDCARDS`] — the reference's
/// `action in REGISTERED_TRIGGERS or action in _ACTION_WILDCARDS`.
#[must_use]
pub fn admits_action(token: &PythonString) -> bool {
    vocabulary_token(token).is_some_and(|text| {
        ACTION_WILDCARDS.contains(&text.as_str()) || REGISTERED_TRIGGERS.contains(&text.as_str())
    })
}

/// The reference's `resolve_for_context` slot match for one entry's context.
///
/// * `mission_type` matches when the slot is absent, is a wildcard
///   ([`ACTION_WILDCARDS`]) or equals the runtime mission type.
/// * `action` matches when the slot is absent, is a wildcard, equals the runtime
///   action, **or** is one of the fine-grained triggers — those describe
///   sub-actions that may occur during any mission verb, so the reference treats
///   them as wildcards for matching (while keeping the authored label verbatim).
#[must_use]
pub fn matches_context(
    context: &[(PythonString, PythonString)],
    mission_type: &str,
    action: &str,
) -> bool {
    let mission_type_matches = match last_declared(context, MISSION_TYPE_KEY) {
        None => true,
        Some(declared) => vocabulary_token(declared)
            .is_some_and(|text| ACTION_WILDCARDS.contains(&text.as_str()) || text == mission_type),
    };
    let action_matches = match last_declared(context, ACTION_KEY) {
        None => true,
        Some(declared) => vocabulary_token(declared).is_some_and(|text| {
            ACTION_WILDCARDS.contains(&text.as_str())
                || text == action
                || FINE_GRAINED_TRIGGERS.contains(&text.as_str())
        }),
    };
    mission_type_matches && action_matches
}

/// One `activation_context` slot the reference's model validator refuses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActivationContextRejection {
    /// `activation_context.mission_type` is not in `ALLOWED_MISSION_TYPES`.
    MissionType(PythonString),
    /// `activation_context.action` is not in `REGISTERED_TRIGGERS` and is not a
    /// wildcard.
    Action(PythonString),
}

impl ActivationContextRejection {
    /// The offending `activation_context` key (`mission_type` / `action`).
    #[must_use]
    pub fn field(&self) -> &'static str {
        match self {
            Self::MissionType(_) => MISSION_TYPE_KEY,
            Self::Action(_) => ACTION_KEY,
        }
    }

    /// The authored value that was refused, exactly.
    #[must_use]
    pub fn value(&self) -> &PythonString {
        match self {
            Self::MissionType(value) | Self::Action(value) => value,
        }
    }

    /// The reference-shaped refusal sentence for this value.
    #[must_use]
    pub fn message(&self, registry: &ActivationSelectorRegistry) -> String {
        match self {
            Self::MissionType(value) => format!(
                "activation_context.mission_type={} is not in ALLOWED_MISSION_TYPES=[{}]",
                value.repr(),
                registry.mission_type_listing()
            ),
            Self::Action(value) => format!(
                "activation_context.action={} is not in REGISTERED_TRIGGERS=[{}] \
                 and is not one of the wildcards [{}]",
                value.repr(),
                python_listing(REGISTERED_TRIGGERS.iter().map(|token| python_repr(token))),
                python_listing(ACTION_WILDCARDS.iter().map(|token| python_repr(token)))
            ),
        }
    }
}

/// Python `dict` semantics for the slots this module reads: the LAST authored
/// value of a duplicated key wins (`str` equality, so only scalar keys qualify).
fn last_declared<'a>(
    context: &'a [(PythonString, PythonString)],
    key: &str,
) -> Option<&'a PythonString> {
    context
        .iter()
        .rev()
        .find(|(candidate, _)| vocabulary_token(candidate).is_some_and(|text| text == key))
        .map(|(_, value)| value)
}

/// The Rust text of a Python string, or `None` for a lone-surrogate token (which
/// can never equal an ASCII vocabulary word).
fn vocabulary_token(token: &PythonString) -> Option<String> {
    token.to_scalar_string().ok()
}

/// Render already-represented names as the reference's Python list body.
fn python_listing(items: impl Iterator<Item = String>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

/// The canonical built-in pack root, when a caller needs it without a registry.
///
/// # Errors
///
/// [`DoctrineError::PackRootUnavailable`] when the built-in tier is unavailable.
pub fn canonical_built_in_root() -> Result<PathBuf, DoctrineError> {
    crate::org_charter::built_in_search(Path::new(""))
        .find_root()
        .map_err(|_| DoctrineError::PackRootUnavailable { tier: "built-in" })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str) -> PythonString {
        PythonString::from_scalars(text)
    }

    fn context(pairs: &[(&str, &str)]) -> Vec<(PythonString, PythonString)> {
        pairs
            .iter()
            .map(|(key, value)| (token(key), token(value)))
            .collect()
    }

    fn registry() -> ActivationSelectorRegistry {
        let built_in = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join("packs/built-in");
        ActivationSelectorRegistry::from_built_in_root(&built_in).expect("built-in roster")
    }

    #[test]
    fn registered_triggers_is_the_flattened_union_formula() {
        let mut union: Vec<&str> = ALLOWED_ACTIONS
            .iter()
            .chain(FINE_GRAINED_TRIGGERS.iter())
            .copied()
            .collect();
        union.sort_unstable();
        assert_eq!(union, REGISTERED_TRIGGERS.to_vec());
        for fine in FINE_GRAINED_TRIGGERS {
            assert!(
                !ALLOWED_ACTIONS.contains(&fine),
                "{fine} must be fine-grained, not a mission verb"
            );
        }
        for wildcard in ACTION_WILDCARDS {
            assert!(
                !REGISTERED_TRIGGERS.contains(&wildcard),
                "{wildcard} is layered on top, not a trigger"
            );
        }
    }

    #[test]
    fn the_bootstrap_action_list_is_not_the_operator_vocabulary() {
        // The reason `REGISTERED_TRIGGERS` exists: the agent bootstrap list is a
        // strict subset, so reusing it would refuse `write_comment`.
        for action in crate::BOOTSTRAP_ACTIONS {
            assert!(
                REGISTERED_TRIGGERS.contains(&action),
                "bootstrap action {action} is a registered trigger"
            );
        }
        assert!(
            !crate::BOOTSTRAP_ACTIONS.contains(&"write_comment"),
            "fine-grained triggers are absent from the bootstrap list"
        );
    }

    #[test]
    fn the_registry_derives_the_built_in_roster_plus_wildcards() {
        let registry = registry();
        let names: Vec<String> = registry
            .allowed_mission_types()
            .iter()
            .map(|id| id.to_scalar_string().expect("scalar id"))
            .collect();
        // The four mission types the pinned packs ship, plus the wildcards.
        assert_eq!(
            names,
            [
                "any",
                "documentation",
                "generic",
                "plan",
                "research",
                "software-dev"
            ]
        );
        assert!(registry.admits_mission_type(&token("software-dev")));
        assert!(registry.admits_mission_type(&token("any")));
        assert!(!registry.admits_mission_type(&token("software_dev")));
        assert!(!registry.admits_mission_type(&token("bogus")));
    }

    #[test]
    fn matching_is_exact_and_never_normalises() {
        for accepted in ["implement", "write_comment", "any", "generic"] {
            assert!(admits_action(&token(accepted)), "{accepted} is admitted");
        }
        for refused in [
            "Implement",
            " implement",
            "implement ",
            "",
            "bogus",
            "merge ",
        ] {
            assert!(!admits_action(&token(refused)), "{refused} is refused");
        }
        // A lone surrogate token is not any vocabulary word.
        let mut surrogate = PythonString::from_scalars("implement");
        surrogate.push_codepoint(0xD800).expect("surrogate unit");
        assert!(!admits_action(&surrogate));
    }

    #[test]
    fn validate_context_names_the_first_bad_slot_and_keeps_extra_keys() {
        let registry = registry();
        assert_eq!(
            registry.validate_context(&context(&[
                ("mission_type", "research"),
                ("action", "write_docstring"),
                ("extra", "kept verbatim"),
            ])),
            Ok(())
        );
        assert_eq!(
            registry.validate_context(&context(&[("mission_type", "bogus")])),
            Err(ActivationContextRejection::MissionType(token("bogus")))
        );
        // The mission slot is checked first, exactly like the reference.
        let both = registry
            .validate_context(&context(&[("mission_type", "bogus"), ("action", "bogus")]))
            .expect_err("both slots invalid");
        assert_eq!(both.field(), "mission_type");
        let message = both.message(&registry);
        assert!(
            message.contains("activation_context.mission_type='bogus'")
                && message.contains("ALLOWED_MISSION_TYPES=['any', 'documentation'"),
            "{message}"
        );
        let action = registry
            .validate_context(&context(&[("action", "bogus")]))
            .expect_err("bad action");
        let message = action.message(&registry);
        assert!(
            message.contains("activation_context.action='bogus'")
                && message.contains("REGISTERED_TRIGGERS=['accept', 'add_dependency'")
                && message.contains("wildcards ['any', 'generic']"),
            "{message}"
        );
        assert_eq!(registry.validate_context(&[]), Ok(()));
    }

    #[test]
    fn resolve_facts_treat_fine_grained_triggers_as_wildcards() {
        assert!(matches_context(&[], "software-dev", "implement"));
        assert!(matches_context(
            &context(&[("mission_type", "software-dev"), ("action", "implement")]),
            "software-dev",
            "implement"
        ));
        assert!(!matches_context(
            &context(&[("mission_type", "software-dev")]),
            "research",
            "implement"
        ));
        assert!(matches_context(
            &context(&[("action", "write_comment")]),
            "research",
            "review"
        ));
        assert!(matches_context(
            &context(&[("mission_type", "generic"), ("action", "any")]),
            "documentation",
            "merge"
        ));
        assert!(!matches_context(
            &context(&[("action", "implement")]),
            "software-dev",
            "review"
        ));
    }
}
