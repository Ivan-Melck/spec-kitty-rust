//! Source really constructs date/datetime; Pydantic JSON presentation is the distinct phase.
use sk_charter::ArtifactKind;
use sk_doctrine::{
    canonical_artifact_body_exact_path, context_render::include_full_payload_lines_exact,
};

#[test]
fn unstructured_datetime_uses_source_model_json_utc_z_not_python_str_offset() {
    for (authored, expected) in [
        ("2020-01-01T01:02:03Z", "2020-01-01T01:02:03Z"),
        ("!!timestamp 2020-01-01T01:02:03Z", "2020-01-01T01:02:03Z"),
        ("2020-01-01T01:02:03+00:00", "2020-01-01T01:02:03Z"),
        ("2020-01-01T01:02:03.001000Z", "2020-01-01T01:02:03.001000Z"),
        ("2020-01-01T01:02:03+02:30", "2020-01-01T01:02:03+02:30"),
        ("2020-01-01", "2020-01-01"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("fixture.styleguide.yaml");
        std::fs::write(&path, format!("id: source-payload-probe\nschema_version: '1.0'\ntitle: probe\nscope: code\nprinciples: [one]\nstructural_lint_config:\n  stamp: {authored}\n")).unwrap();
        let payload = canonical_artifact_body_exact_path(ArtifactKind::Styleguide, &path).unwrap();
        let rendered = include_full_payload_lines_exact(&payload).join("\n");
        assert!(
            rendered.contains(&format!("\"stamp\": \"{expected}\"")),
            "{authored}: {rendered}"
        );
    }
}
