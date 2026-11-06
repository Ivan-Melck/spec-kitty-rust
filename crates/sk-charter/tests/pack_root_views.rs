//! Native inspection/resource-locator invariants complementary to the paired
//! Source execution tests in sk-doctrine. These are not Source parity probes.
use sk_charter::{BuiltInSearch, CharterError, Limits, PackRoots, PathExpansion};
use std::fs;

#[cfg(unix)]
#[test]
fn lexical_execution_view_does_not_weaken_contained_inspection() {
    let root = tempfile::tempdir().unwrap().keep();
    let project = root.join("project");
    let external = root.join("external");
    fs::create_dir_all(project.join(".kittify")).unwrap();
    fs::create_dir_all(&external).unwrap();
    let lexical = project.join(".kittify/charter-packs");
    std::os::unix::fs::symlink(&external, &lexical).unwrap();
    let config = sk_charter::load_project_config(&project, &Limits::default()).unwrap();
    let expansion = PathExpansion::from_env();
    assert!(PackRoots::resolve(&project, &config, &expansion, None).is_err());
    assert!(PackRoots::resolve_existing(&project, Some(&config), &expansion, None).is_err());
    assert_eq!(
        PackRoots::resolve_source(&project, &config, &expansion, None)
            .unwrap()
            .project,
        Some(lexical.clone())
    );
    assert_eq!(
        PackRoots::resolve_existing_source(&project, None, &expansion, None)
            .unwrap()
            .project,
        Some(lexical)
    );
}

#[test]
fn fallback_resource_anchor_never_hides_an_incomplete_pinned_install() {
    let root = tempfile::tempdir().unwrap().keep();
    let installed = root.join("installed");
    let development = root.join("development");
    fs::create_dir_all(installed.join("bin")).unwrap();
    fs::create_dir_all(development.join("packs/built-in")).unwrap();
    let search = BuiltInSearch::new(installed.join("bin"))
        .with_max_ancestor_hops(2)
        .with_fallback_anchor(&development);
    assert_eq!(
        search.find_root().unwrap(),
        development.join("packs/built-in").canonicalize().unwrap()
    );
    fs::create_dir_all(installed.join("packs")).unwrap();
    fs::write(installed.join("packs/built-in.source.yaml"), b"").unwrap();
    assert!(matches!(
        search.find_root(),
        Err(CharterError::BuiltInPackMissing)
    ));
}
