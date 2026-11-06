//! Actual compatibility-root wiring; Native smoke, not Source conformance.

use clap::Parser;
use sk_cli::compat::{Cli, Command, capabilities, charter, dispatch, doctor};

#[test]
fn root_routes_charter_to_the_delivered_tree_not_native_inspection() {
    let cli = Cli::try_parse_from(["spec-kitty", "charter", "list", "--json"])
        .expect("actual compatibility root");
    assert!(matches!(
        cli.command,
        Command::Charter {
            command: charter::CharterCommand::List(_)
        }
    ));
    let cli = Cli::try_parse_from([
        "spec-kitty",
        "charter",
        "context",
        "--action",
        "review",
        "--json",
    ])
    .expect("actual context route");
    assert!(matches!(
        cli.command,
        Command::Charter {
            command: charter::CharterCommand::Context(_)
        }
    ));
}

#[test]
fn root_routes_doctor_and_bare_doctor_is_not_a_successful_health_check() {
    let cli = Cli::try_parse_from(["spec-kitty", "doctor", "identity", "--json"])
        .expect("actual diagnostic root");
    assert!(matches!(
        cli.command,
        Command::Doctor(doctor::DoctorArgs {
            command: Some(doctor::DoctorCommand::Identity { .. })
        })
    ));
    let cli = Cli::try_parse_from(["spec-kitty", "doctor"]).expect("bare doctor");
    assert_eq!(dispatch(cli.command).expect("typed empty-group refusal"), 2);
}

#[test]
fn capability_rows_are_partial_and_do_not_remain_globally_unsupported() {
    let value = capabilities::matrix();
    for name in ["accept", "charter", "doctor"] {
        let rows: Vec<_> = value["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .filter(|row| row["name"] == name)
            .collect();
        assert_eq!(rows.len(), 1, "one truthful row for {name}");
        assert_eq!(rows[0]["state"], "partial");
        assert!(!rows[0]["gaps"].as_array().expect("real gaps").is_empty());
        assert!(
            !value["unsupported"]
                .as_array()
                .expect("unsupported")
                .iter()
                .any(|row| row["name"] == name)
        );
    }
    assert_eq!(
        value["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .find(|row| row["name"] == "doctor")
            .expect("doctor")["implemented_leaves"],
        serde_json::json!(doctor::IMPLEMENTED_LEAVES)
    );
}
