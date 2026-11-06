//! Bundle-schema versioning and compatibility registry.
//!
//! Partial Native inspection of `src/charter/offering/versioning.py`: the
//! integer bundle schema versions, the four-way compatibility verdict and the
//! pure compatibility check a reader uses to decide whether the bundle in
//! `.kittify/charter/` can be read natively or needs `spec-kitty upgrade`.
//!
//! This is the charter-layer owner (the reference module lives under
//! `charter/offering/`). The upgrade-side migration that *writes* this field
//! (`3.2.0rc35_charter_bundle_v2` and friends) keeps its own migration-local
//! copy under `sk-install`; that copy is migration bookkeeping, not the read
//! authority every status reader must consult.
//!
//! Only the read half is ported here. `migrate_v1_to_v2`,
//! `repair_v2_synthesis_manifest_defaults` and `run_migration` are write
//! operations owned by the upgrade crate and are deliberately absent.
//!
//! This is NOT a Source-compatible version reader yet. Source returns Python
//! bools as bools (including `True`/`False` message spelling), accepts arbitrary
//! integers, and propagates read/parse failures despite its 'Never raises'
//! docstring. This bounded i64/serde inspection collapses those failures to
//! absence. Those are open defects, not authorized semantic exceptions; do not
//! wire this helper into the compatibility gate as if they were equivalent.

use std::path::Path;

use crate::limits::Limits;

/// The bundle schema version written by Phase 7 synthesis
/// (`CURRENT_BUNDLE_SCHEMA_VERSION`).
pub const CURRENT_BUNDLE_SCHEMA_VERSION: i64 = 2;
/// Oldest bundle schema version this CLI can read (`MIN_READABLE_BUNDLE_SCHEMA`).
pub const MIN_READABLE_BUNDLE_SCHEMA: i64 = 1;
/// Newest bundle schema version this CLI reads natively
/// (`MAX_READABLE_BUNDLE_SCHEMA`).
pub const MAX_READABLE_BUNDLE_SCHEMA: i64 = 2;

/// Compatibility status of a bundle with this CLI
/// (`BundleCompatibilityStatus`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleCompatibilityStatus {
    /// The bundle can be used without migration.
    Compatible,
    /// An older, migration-upgradable bundle.
    NeedsMigration,
    /// A bundle older than the earliest supported version.
    IncompatibleOld,
    /// A bundle newer than this CLI understands.
    IncompatibleNew,
    /// No `metadata.bundle_schema_version` was recorded.
    MissingVersion,
}

impl BundleCompatibilityStatus {
    /// The reference's enum value (`"COMPATIBLE"`, …).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compatible => "COMPATIBLE",
            Self::NeedsMigration => "NEEDS_MIGRATION",
            Self::IncompatibleOld => "INCOMPATIBLE_OLD",
            Self::IncompatibleNew => "INCOMPATIBLE_NEW",
            Self::MissingVersion => "MISSING_VERSION",
        }
    }
}

/// Result of a bundle compatibility check (`BundleCompatibilityResult`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleCompatibility {
    /// The verdict.
    pub status: BundleCompatibilityStatus,
    /// The integer schema version read from bundle metadata, if any.
    pub bundle_version: Option<i64>,
    /// The oldest readable schema version.
    pub supported_min: i64,
    /// The newest readable schema version.
    pub supported_max: i64,
    /// The operator-facing message (the reference's `message`).
    pub message: String,
    /// The reference's process exit code for this verdict (0 or 1).
    pub exit_code: i32,
}

impl BundleCompatibility {
    /// True if the bundle can be used without migration
    /// (`is_compatible`).
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        self.status == BundleCompatibilityStatus::Compatible
    }

    /// True if the bundle needs migration before use (`needs_migration`).
    #[must_use]
    pub fn needs_migration(&self) -> bool {
        matches!(
            self.status,
            BundleCompatibilityStatus::NeedsMigration | BundleCompatibilityStatus::MissingVersion
        )
    }
}

/// Check whether a bundle schema version is compatible with this CLI.
///
/// Pure function — no filesystem I/O, exactly like
/// `check_bundle_compatibility`.
#[must_use]
pub fn check_bundle_compatibility(bundle_version: Option<i64>) -> BundleCompatibility {
    let supported_min = MIN_READABLE_BUNDLE_SCHEMA;
    let supported_max = MAX_READABLE_BUNDLE_SCHEMA;
    let Some(version) = bundle_version else {
        return BundleCompatibility {
            status: BundleCompatibilityStatus::MissingVersion,
            bundle_version: None,
            supported_min,
            supported_max,
            message: "Bundle schema version not found; treating as v1. Run `spec-kitty upgrade`."
                .to_owned(),
            exit_code: 1,
        };
    };
    if version == CURRENT_BUNDLE_SCHEMA_VERSION {
        return BundleCompatibility {
            status: BundleCompatibilityStatus::Compatible,
            bundle_version: Some(version),
            supported_min,
            supported_max,
            message: format!("Bundle schema version {version} is supported."),
            exit_code: 0,
        };
    }
    if supported_min <= version && version < CURRENT_BUNDLE_SCHEMA_VERSION {
        return BundleCompatibility {
            status: BundleCompatibilityStatus::NeedsMigration,
            bundle_version: Some(version),
            supported_min,
            supported_max,
            message: format!(
                "Bundle schema version {version} requires migration. Run `spec-kitty upgrade`."
            ),
            exit_code: 1,
        };
    }
    if version < supported_min {
        return BundleCompatibility {
            status: BundleCompatibilityStatus::IncompatibleOld,
            bundle_version: Some(version),
            supported_min,
            supported_max,
            message: format!(
                "Bundle schema version {version} predates the earliest supported version \
                 ({supported_min}). Contact support."
            ),
            exit_code: 1,
        };
    }
    BundleCompatibility {
        status: BundleCompatibilityStatus::IncompatibleNew,
        bundle_version: Some(version),
        supported_min,
        supported_max,
        message: format!(
            "Bundle schema version {version} is newer than this CLI supports \
             ({supported_max}). Upgrade your CLI."
        ),
        exit_code: 1,
    }
}

/// Read the bundle schema version from `<charter_dir>/charter.yaml`.
///
/// Re-pointed by `consolidate-charter-bundle` (WP07 / T030) from the retired
/// `<charter_dir>/metadata.yaml` top-level key onto
/// `<charter_dir>/charter.yaml`'s `metadata.bundle_schema_version`, exactly as
/// the reference did. The read is a plain YAML dict-walk, not a model
/// validation. This PARTIAL Native inspection maps absence, non-i64 values and
/// read/parse failure to `None`; unlike Source, it does not preserve bools,
/// arbitrary integers or propagated failures. Compatibility integration is pending.
#[must_use]
pub fn get_bundle_schema_version(charter_dir: &Path) -> Option<i64> {
    // This argument is already .kittify/charter, not the repository root.
    let path = charter_dir.join("charter.yaml");
    let bytes = crate::io::read_bounded(&path, Limits::default().max_charter_file_bytes).ok()?;
    let document: serde_yaml_ng::Value = serde_yaml_ng::from_slice(&bytes).ok()?;
    let metadata = document.get("metadata")?;
    let value = metadata.get("bundle_schema_version")?;
    value.as_i64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_five_verdicts_match_the_reference_registry() {
        let missing = check_bundle_compatibility(None);
        assert_eq!(missing.status, BundleCompatibilityStatus::MissingVersion);
        assert_eq!(missing.bundle_version, None);
        assert!(missing.needs_migration());
        assert!(!missing.is_compatible());
        assert_eq!(missing.exit_code, 1);

        let compatible = check_bundle_compatibility(Some(2));
        assert_eq!(compatible.status, BundleCompatibilityStatus::Compatible);
        assert!(compatible.is_compatible());
        assert_eq!(compatible.message, "Bundle schema version 2 is supported.");
        assert_eq!(compatible.exit_code, 0);

        let migration = check_bundle_compatibility(Some(1));
        assert_eq!(migration.status, BundleCompatibilityStatus::NeedsMigration);
        assert_eq!(
            migration.message,
            "Bundle schema version 1 requires migration. Run `spec-kitty upgrade`."
        );

        let old = check_bundle_compatibility(Some(0));
        assert_eq!(old.status, BundleCompatibilityStatus::IncompatibleOld);
        assert_eq!(
            old.message,
            "Bundle schema version 0 predates the earliest supported version (1). Contact support."
        );

        let new = check_bundle_compatibility(Some(3));
        assert_eq!(new.status, BundleCompatibilityStatus::IncompatibleNew);
        assert_eq!(
            new.message,
            "Bundle schema version 3 is newer than this CLI supports (2). Upgrade your CLI."
        );
        assert_eq!(new.exit_code, 1);
    }

    #[test]
    fn native_bounded_inspection_walks_metadata_and_collapses_errors() {
        let temp = tempfile::tempdir().expect("temp");
        let charter_dir = temp.path().join(".kittify/charter");
        std::fs::create_dir_all(&charter_dir).expect("dir");

        // Absent file.
        assert_eq!(get_bundle_schema_version(&charter_dir), None);

        // Present, but no metadata section.
        std::fs::write(charter_dir.join("charter.yaml"), "schema_version: 2.0.0\n").expect("write");
        assert_eq!(get_bundle_schema_version(&charter_dir), None);

        // Present, metadata without the key.
        std::fs::write(charter_dir.join("charter.yaml"), "metadata:\n  x: 1\n").expect("write");
        assert_eq!(get_bundle_schema_version(&charter_dir), None);

        // A non-integer value is absent, not coerced.
        std::fs::write(
            charter_dir.join("charter.yaml"),
            "metadata:\n  bundle_schema_version: '2'\n",
        )
        .expect("write");
        assert_eq!(get_bundle_schema_version(&charter_dir), None);

        // The real field.
        std::fs::write(
            charter_dir.join("charter.yaml"),
            "metadata:\n  bundle_schema_version: 2\n",
        )
        .expect("write");
        assert_eq!(get_bundle_schema_version(&charter_dir), Some(2));

        // Unparseable YAML is absent, not an error.
        std::fs::write(charter_dir.join("charter.yaml"), "metadata: [\n").expect("write");
        assert_eq!(get_bundle_schema_version(&charter_dir), None);
    }
}
