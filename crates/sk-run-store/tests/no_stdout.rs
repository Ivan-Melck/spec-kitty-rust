//! Guard: the run store reports failures as typed values, never by printing.
//!
//! A CLI is responsible for rendering errors on whichever stream it chooses;
//! a library that prints would corrupt machine-readable stdout and make errors
//! untestable through the API.

use std::fs;
use std::path::Path;

#[test]
fn the_library_sources_contain_no_direct_output_or_debug_macros() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut checked = 0usize;

    fn walk(dir: &Path, checked: &mut usize) {
        for entry in fs::read_dir(dir).expect("read src") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                walk(&path, checked);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                *checked += 1;
                let text = fs::read_to_string(&path).expect("utf8 source");
                for forbidden in ["println!", "print!", "eprintln!", "eprint!", "dbg!"] {
                    assert!(
                        !text.contains(forbidden),
                        "{} contains {forbidden}; library errors must be returned, not printed",
                        path.display()
                    );
                }
            }
        }
    }

    walk(&source_root, &mut checked);
    assert!(checked >= 5, "expected the library modules to be scanned");
}

#[test]
fn error_display_is_stable_and_actionable() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let error_rs = fs::read_to_string(source_root.join("error.rs")).expect("error.rs");
    for code in [
        "RUN_STATE_MISSING",
        "RUN_DIR_OUTSIDE_REPO",
        "TORN_EVENT_TAIL",
        "STALE_ADVANCE_PLAN",
        "AUDIT_OWNER_REQUIRED",
        "LLM_DELEGATION_REQUIRED",
        "FROZEN_TEMPLATE_DRIFT",
        "RECOVERY_REQUIRED",
    ] {
        assert!(
            error_rs.contains(code),
            "stable error code {code} is missing from the typed error surface"
        );
    }
}
