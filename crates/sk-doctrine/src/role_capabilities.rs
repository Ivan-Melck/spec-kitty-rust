//! Canonical role capabilities (`DEFAULT_ROLE_CAPABILITIES`).
//!
//! One native home for the eight canonical role rows the reference ships in
//! `charter/offering/agent_profiles/capabilities.py`. The router's
//! keyword-match action (`DEFAULT_ROLE_CAPABILITIES[role].canonical_verbs[0]`)
//! and its explicit-hint role default both read this table; no crate copies it.
//!
//! Scope: this is the *canonical* (well-known) role vocabulary only. Custom
//! roles are intentionally open in the reference model (`Role` is an open
//! `str` subclass), so a custom role has no row here and the router falls back
//! to `"review"` exactly as the reference does. Lookup is therefore an exact
//! match on an already-normalized role value (the reference's `Role`
//! constructor lowercases; this table does not re-case its input, and there is
//! no second profile loader or case-folding authority here).
//!
//! This table is NOT the canonical *verb map* (`CANONICAL_VERB_MAP`) or the
//! stop-word set used for request tokenization: those are a different domain
//! authority with different contents, and deriving one from the other would be
//! wrong.

/// One canonical role's default capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoleCapabilities {
    /// Canonical role value (already lower-case, matching `Role`).
    pub role: &'static str,
    /// Default tool capabilities.
    pub default_capabilities: &'static [&'static str],
    /// Canonical verbs; the first entry is the role's default action.
    pub canonical_verbs: &'static [&'static str],
    /// Human-readable role description.
    pub description: &'static str,
}

/// The eight canonical rows, in the reference's declaration order.
pub const ROLE_CAPABILITIES: [RoleCapabilities; 8] = [
    RoleCapabilities {
        role: "implementer",
        default_capabilities: &["read", "write", "edit", "bash", "search"],
        canonical_verbs: &["generate", "refine", "implement"],
        description: "Code implementation and feature development",
    },
    RoleCapabilities {
        role: "reviewer",
        default_capabilities: &["read", "search"],
        canonical_verbs: &["audit", "assess", "review"],
        description: "Code review and quality assurance",
    },
    RoleCapabilities {
        role: "architect",
        default_capabilities: &["read", "write", "search", "edit", "bash"],
        canonical_verbs: &["audit", "synthesize", "plan"],
        description: "System design and architectural decisions",
    },
    RoleCapabilities {
        role: "designer",
        default_capabilities: &["read", "write", "search"],
        canonical_verbs: &["synthesize", "draft", "design"],
        description: "UI/UX and interface design",
    },
    RoleCapabilities {
        role: "planner",
        default_capabilities: &["read", "write", "search"],
        canonical_verbs: &["plan", "decompose", "prioritize"],
        description: "Project planning and task decomposition",
    },
    RoleCapabilities {
        role: "researcher",
        default_capabilities: &["read", "search"],
        canonical_verbs: &["analyze", "investigate", "summarize"],
        description: "Research and information gathering",
    },
    RoleCapabilities {
        role: "curator",
        default_capabilities: &["read", "write", "search"],
        canonical_verbs: &["classify", "curate", "validate"],
        description: "Content curation and organization",
    },
    RoleCapabilities {
        role: "manager",
        default_capabilities: &["read", "search"],
        canonical_verbs: &["coordinate", "delegate", "monitor"],
        description: "Project and team management",
    },
];

/// Look up one canonical role row by its already-normalized value.
///
/// Returns `None` for a custom (open-vocabulary) role, matching the reference's
/// `DEFAULT_ROLE_CAPABILITIES.get(role)`; callers apply their own fallback
/// (`"review"` for the router) rather than this function inventing one.
pub fn role_capabilities(role: &str) -> Option<&'static RoleCapabilities> {
    ROLE_CAPABILITIES.iter().find(|row| row.role == role)
}

/// The role's default action: `canonical_verbs[0]`, when the row has one.
pub fn role_default_verb(role: &str) -> Option<&'static str> {
    role_capabilities(role).and_then(|row| row.canonical_verbs.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_has_the_eight_canonical_rows() {
        assert_eq!(ROLE_CAPABILITIES.len(), 8);
        let roles: Vec<&str> = ROLE_CAPABILITIES.iter().map(|row| row.role).collect();
        assert_eq!(
            roles,
            vec![
                "implementer",
                "reviewer",
                "architect",
                "designer",
                "planner",
                "researcher",
                "curator",
                "manager",
            ]
        );
        for row in ROLE_CAPABILITIES.iter() {
            assert!(!row.default_capabilities.is_empty(), "{}", row.role);
            assert!(!row.canonical_verbs.is_empty(), "{}", row.role);
            assert!(!row.description.is_empty(), "{}", row.role);
            assert_eq!(role_capabilities(row.role), Some(row));
        }
    }

    #[test]
    fn keyword_actions_match_the_reference_rows() {
        // The values the pinned reference reports for the three roles the
        // router's keyword/hint fallbacks are verified against.
        assert_eq!(role_default_verb("reviewer"), Some("audit"));
        assert_eq!(role_default_verb("implementer"), Some("generate"));
        assert_eq!(role_default_verb("curator"), Some("classify"));
        assert_eq!(role_default_verb("designer"), Some("synthesize"));
    }

    #[test]
    fn custom_and_unknown_roles_have_no_row() {
        assert_eq!(role_capabilities("senior-tech-lead"), None);
        assert_eq!(
            role_capabilities("Implementer"),
            None,
            "input is already normalized"
        );
        assert_eq!(role_capabilities(""), None);
        assert_eq!(role_default_verb("senior-tech-lead"), None);
    }
}
