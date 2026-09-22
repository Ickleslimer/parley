use parley_handoff::schema::PeerActivityDocument;

pub(crate) fn unavailable_activity() -> PeerActivityDocument {
    PeerActivityDocument::unavailable("not_activated")
}
