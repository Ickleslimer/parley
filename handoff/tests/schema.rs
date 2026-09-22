use parley_handoff::schema::{HandoffState, PeerActivityDocument, HANDOFF_SCHEMA_VERSION};

#[test]
fn schema_contract_is_closed_and_unavailable_is_non_judgmental() {
    assert_eq!(HANDOFF_SCHEMA_VERSION, 1);
    assert_eq!(HandoffState::AwaitingAck, HandoffState::AwaitingAck);
    let unavailable = PeerActivityDocument::unavailable("missing");
    assert_eq!(unavailable.assessment, "not_inferred");
    assert!(!unavailable.excerpt.available);
}
