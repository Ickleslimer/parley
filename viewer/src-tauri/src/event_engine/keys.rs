pub struct DecodedKey {
    pub source_id: String,
    pub generation: u64,
    pub raw_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyKind {
    Event,
    Exchange,
    Session,
}

#[derive(Clone, Copy)]
pub(crate) struct KeyContext<'a> {
    pub source_id: &'a str,
    pub generation: u64,
    pub path: &'a str,
}

pub fn encode_key(kind: KeyKind, source_id: &str, generation: u64, raw_id: &str) -> String {
    format!(
        "pv1:{}:{}:{source_id}:{generation}:{raw_id}",
        kind.token(),
        source_id.len()
    )
}

pub fn decode_key(key: &str, expected: KeyKind) -> Option<DecodedKey> {
    let rest = key.strip_prefix("pv1:")?;
    let (kind, rest) = rest.split_once(':')?;
    if KeyKind::parse(kind)? != expected {
        return None;
    }
    let (len_str, rest) = rest.split_once(':')?;
    let len: usize = len_str.parse().ok()?;
    if rest.len() < len {
        return None;
    }
    let source_id = rest[..len].to_string();
    let rest = rest.get(len..)?.strip_prefix(':')?;
    let (generation_str, raw_id) = rest.split_once(':')?;
    if raw_id.is_empty() {
        return None;
    }
    Some(DecodedKey {
        source_id,
        generation: generation_str.parse().ok()?,
        raw_id: raw_id.to_string(),
    })
}

impl KeyKind {
    fn token(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::Exchange => "exchange",
            Self::Session => "session",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "event" => Some(Self::Event),
            "exchange" => Some(Self::Exchange),
            "session" => Some(Self::Session),
            _ => None,
        }
    }
}

impl KeyContext<'_> {
    pub(crate) fn event_key(&self, raw_id: &str) -> String {
        encode_key(KeyKind::Event, self.source_id, self.generation, raw_id)
    }

    pub(crate) fn exchange_key(&self, raw_id: &str) -> String {
        encode_key(KeyKind::Exchange, self.source_id, self.generation, raw_id)
    }

    pub(crate) fn session_key(&self, raw_id: &str) -> String {
        encode_key(KeyKind::Session, self.source_id, self.generation, raw_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_source_ids_that_contain_colons_and_rejects_wrong_kinds() {
        let source = r"c:\logs:prod\events.jsonl";
        let key = encode_key(KeyKind::Event, source, 9, "event-1");
        let decoded = decode_key(&key, KeyKind::Event).expect("key should decode");
        assert_eq!(decoded.source_id, source);
        assert_eq!(decoded.generation, 9);
        assert_eq!(decoded.raw_id, "event-1");
        assert!(decode_key(&key, KeyKind::Exchange).is_none());
        assert!(decode_key("event-1", KeyKind::Event).is_none());
        assert!(decode_key("pv1:event:3:abc:1:", KeyKind::Event).is_none());
    }
}
