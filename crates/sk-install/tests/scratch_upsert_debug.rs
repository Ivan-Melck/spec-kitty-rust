use sk_install::yaml_edit::{InsertPosition, Scalar, YamlEditor};

const MANIFEST: &str = "adapter_id: fixture\nadapter_version: 1.0.0\nartifacts: []\ncreated_at: '2026-10-01T00:00:00+00:00'\nmanifest_hash: zzz\nrun_id: 01JA\nschema_version: '1'\n";

#[test]
fn debug_upsert_variants() {
    let cases: Vec<Vec<(String, Scalar)>> = vec![
        vec![("mission_id".into(), Scalar::Null)],
        vec![
            (
                "synthesizer_version".into(),
                Scalar::Str("(pre-phase7-migration)".into()),
            ),
            ("mission_id".into(), Scalar::Null),
            ("built_in_only".into(), Scalar::Bool(false)),
        ],
        vec![
            (
                "synthesizer_version".into(),
                Scalar::Str("(pre-phase7-migration)".into()),
            ),
            ("mission_id".into(), Scalar::Null),
            ("built_in_only".into(), Scalar::Bool(false)),
            ("schema_version".into(), Scalar::Str("2".into())),
            ("manifest_hash".into(), Scalar::Str("abc".into())),
        ],
    ];
    for (index, entries) in cases.iter().enumerate() {
        let editor = YamlEditor::new("/tmp/manifest.yaml", MANIFEST);
        match editor.upsert_scalars("", entries, InsertPosition::Bottom) {
            Ok(outcome) => println!("case {index} OK:\n{}", outcome.text),
            Err(error) => println!("case {index} ERR: {error}"),
        }
    }
}
