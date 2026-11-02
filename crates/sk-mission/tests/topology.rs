use serde_json::json;
use sk_mission::{MissionTopology, classify_topology, routes_through_coordination};

#[test]
fn classifies_the_whole_two_by_two_grid() {
    assert_eq!(
        classify_topology(None, false),
        MissionTopology::SingleBranch
    );
    assert_eq!(classify_topology(None, true), MissionTopology::Lanes);
    assert_eq!(
        classify_topology(Some("kitty/mission-x"), false),
        MissionTopology::Coord
    );
    assert_eq!(
        classify_topology(Some("kitty/mission-x"), true),
        MissionTopology::LanesWithCoord
    );
}

#[test]
fn flattened_is_never_a_topology() {
    for topology in MissionTopology::ALL {
        assert_ne!(topology.as_str(), "flattened");
    }
    assert_eq!(
        MissionTopology::from_stored(Some("flattened")),
        None,
        "flattened is provenance, not a stored topology shape"
    );
}

#[test]
fn only_coord_shapes_route_through_coordination() {
    assert!(!routes_through_coordination(MissionTopology::SingleBranch));
    assert!(!routes_through_coordination(MissionTopology::Lanes));
    assert!(routes_through_coordination(MissionTopology::Coord));
    assert!(routes_through_coordination(MissionTopology::LanesWithCoord));
}

#[test]
fn stored_topology_degrades_to_none_instead_of_guessing() {
    assert_eq!(
        MissionTopology::from_stored(Some("single_branch")),
        Some(MissionTopology::SingleBranch)
    );
    assert_eq!(
        MissionTopology::from_stored(Some("lanes_with_coord")),
        Some(MissionTopology::LanesWithCoord)
    );
    assert_eq!(MissionTopology::from_stored(Some("Lanes")), None);
    assert_eq!(MissionTopology::from_stored(Some("")), None);
    assert_eq!(MissionTopology::from_stored(None), None);
}

#[test]
fn serde_uses_the_stored_strings() {
    for topology in MissionTopology::ALL {
        let value = serde_json::to_value(topology).expect("serialize topology");
        assert_eq!(value, json!(topology.as_str()));
    }
    let parsed: MissionTopology = serde_json::from_str("\"lanes\"").expect("deserialize");
    assert_eq!(parsed, MissionTopology::Lanes);
}
