//! Single authority for the Charter Pack tier token.
//!
//! Upstream `kernel.pack_tiers` unifies the historical `"built-in"` /
//! `"builtin"` spellings on the hyphenated token, which is also the on-disk
//! `packs/built-in/` directory name. The tuple order is the precedence rank
//! (built-in < org < project), so no separate rank map is maintained.

/// The canonical built-in tier token (matches `packs/built-in/`).
pub const BUILT_IN: &str = "built-in";

/// The org tier token.
pub const ORG: &str = "org";

/// The project tier token.
pub const PROJECT: &str = "project";

/// Charter Pack tiers in precedence order; the index is the rank.
pub const PACK_TIERS: [&str; 3] = [BUILT_IN, ORG, PROJECT];

const PIN_SUFFIX: &str = ".source.yaml";

/// Return the pin lockfile name for `pack` (`built-in` -> `built-in.source.yaml`).
pub fn pin_filename(pack: &str) -> String {
    format!("{pack}{PIN_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_order_is_built_in_first() {
        assert!(
            PACK_TIERS.iter().position(|tier| *tier == BUILT_IN)
                < PACK_TIERS.iter().position(|tier| *tier == ORG)
        );
        assert!(
            PACK_TIERS.iter().position(|tier| *tier == ORG)
                < PACK_TIERS.iter().position(|tier| *tier == PROJECT)
        );
    }

    #[test]
    fn pin_filename_is_the_single_shape() {
        assert_eq!(pin_filename(BUILT_IN), "built-in.source.yaml");
        assert_eq!(pin_filename(ORG), "org.source.yaml");
    }
}
