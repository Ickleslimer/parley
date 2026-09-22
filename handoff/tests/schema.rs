use std::path::Path;

use parley_handoff::schema::{HandoffState, PeerActivityDocument, HANDOFF_SCHEMA_VERSION};
use parley_handoff::store;

#[test]
fn schema_contract_is_closed_and_unavailable_is_non_judgmental() {
    assert_eq!(HANDOFF_SCHEMA_VERSION, 1);
    assert_eq!(HandoffState::AwaitingAck, HandoffState::AwaitingAck);
    let unavailable = PeerActivityDocument::unavailable("missing");
    assert_eq!(unavailable.assessment, "not_inferred");
    assert!(!unavailable.excerpt.available);
    assert!(!unavailable.assessment.contains("stalled"));
}

#[test]
fn checked_in_fixtures_match_the_frozen_shapes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let activity: PeerActivityDocument =
        serde_json::from_slice(&std::fs::read(root.join("peer-activity-v1.json")).unwrap())
            .unwrap();
    assert_eq!(activity.assessment, "not_inferred");
    assert!(activity.excerpt.available);
    let record = std::fs::read(root.join("handoff-record-v1.json")).unwrap();
    let mut journal = record;
    journal.push(b'\n');
    let parsed = store::parse_journal(&journal).unwrap();
    assert_eq!(parsed.state, "ready");
    assert_eq!(parsed.sequence, 1);
    assert!(parsed.activities.len() == 1);

    let mut unknown: serde_json::Value = serde_json::from_slice(&journal).unwrap();
    unknown["raw_tool_payload"] = serde_json::Value::String("forbidden".to_string());
    let mut unknown = serde_json::to_vec(&unknown).unwrap();
    unknown.push(b'\n');
    assert!(store::parse_journal(&unknown).is_err());
}
