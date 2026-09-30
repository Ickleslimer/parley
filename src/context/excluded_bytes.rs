//! Raw-byte classification for malformed UTF-8 records that are never shared.

use std::collections::BTreeSet;

use super::error::ContextError;

const MAX_DEPTH: usize = 256;
const MAX_KEYS: usize = 16_384;
const MAX_KEY_CHARS: usize = 256;
const SKIPPABLE_RESPONSE_TYPES: &[&str] = &["custom_tool_call_output"];
const MESSAGE_LIKE_KEYS: &[&str] = &["message", "content", "text", "role", "input", "parts"];

pub(super) fn is_skippable_invalid_utf8_record(bytes: &[u8]) -> bool {
    RawJsonParser::new(bytes)
        .parse()
        .is_ok_and(|metadata| metadata.is_skippable())
}

#[derive(Default)]
struct Metadata {
    root_type: Option<String>,
    payload_type: Option<String>,
    message_like_seen: bool,
    payload_object_seen: bool,
}

impl Metadata {
    fn is_skippable(&self) -> bool {
        self.root_type.as_deref() == Some("response_item")
            && self.payload_object_seen
            && self
                .payload_type
                .as_deref()
                .is_some_and(|kind| SKIPPABLE_RESPONSE_TYPES.contains(&kind))
            && !self.message_like_seen
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObjectScope {
    Root,
    Payload,
    Other,
}

struct RawJsonParser<'a> {
    bytes: &'a [u8],
    offset: usize,
    total_keys: usize,
    metadata: Metadata,
}

impl<'a> RawJsonParser<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            offset: 0,
            total_keys: 0,
            metadata: Metadata::default(),
        }
    }

    fn parse(mut self) -> Result<Metadata, ContextError> {
        self.skip_whitespace();
        self.parse_object(ObjectScope::Root, 1)?;
        self.skip_whitespace();
        if self.offset != self.bytes.len() {
            return malformed();
        }
        Ok(self.metadata)
    }

    fn parse_object(&mut self, scope: ObjectScope, depth: usize) -> Result<(), ContextError> {
        self.check_depth(depth)?;
        self.expect(b'{')?;
        self.skip_whitespace();
        let mut keys = BTreeSet::new();
        if self.consume(b'}') {
            return Ok(());
        }
        loop {
            let key = self.parse_captured_string()?;
            self.total_keys = self.total_keys.saturating_add(1);
            if self.total_keys > MAX_KEYS || !keys.insert(key.clone()) {
                return malformed();
            }
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            if MESSAGE_LIKE_KEYS.contains(&key.as_str())
                || (scope != ObjectScope::Root && key == "payload")
            {
                self.metadata.message_like_seen = true;
            }
            match (scope, key.as_str()) {
                (ObjectScope::Root, "type") => {
                    self.metadata.root_type = Some(self.parse_captured_string()?);
                }
                (ObjectScope::Root, "payload") => {
                    if self.peek() != Some(b'{') {
                        return malformed();
                    }
                    self.metadata.payload_object_seen = true;
                    self.parse_object(ObjectScope::Payload, depth + 1)?;
                }
                (ObjectScope::Payload, "type") => {
                    self.metadata.payload_type = Some(self.parse_captured_string()?);
                }
                (ObjectScope::Payload, "payload") => {
                    self.parse_value(depth + 1)?;
                }
                _ => self.parse_value(depth + 1)?,
            }
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(());
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<(), ContextError> {
        self.check_depth(depth)?;
        self.expect(b'[')?;
        self.skip_whitespace();
        if self.consume(b']') {
            return Ok(());
        }
        loop {
            self.parse_value(depth + 1)?;
            self.skip_whitespace();
            if self.consume(b']') {
                return Ok(());
            }
            self.expect(b',')?;
            self.skip_whitespace();
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<(), ContextError> {
        self.check_depth(depth)?;
        self.skip_whitespace();
        match self.peek() {
            Some(b'{') => self.parse_object(ObjectScope::Other, depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => self.parse_opaque_string(),
            Some(b't') => self.parse_literal(b"true"),
            Some(b'f') => self.parse_literal(b"false"),
            Some(b'n') => self.parse_literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => malformed(),
        }
    }

    fn parse_captured_string(&mut self) -> Result<String, ContextError> {
        self.expect(b'"')?;
        let mut value = String::new();
        loop {
            let byte = self.next().ok_or_else(malformed_error)?;
            match byte {
                b'"' => return Ok(value),
                b'\\' => {
                    let decoded = self.parse_escape()?;
                    if !decoded.is_ascii() || decoded.is_control() {
                        return malformed();
                    }
                    if value.chars().count() >= MAX_KEY_CHARS {
                        return malformed();
                    }
                    value.push(decoded);
                }
                0x00..=0x1f | 0x80..=0xff => return malformed(),
                _ => {
                    if value.chars().count() >= MAX_KEY_CHARS {
                        return malformed();
                    }
                    value.push(byte as char);
                }
            }
        }
    }

    fn parse_opaque_string(&mut self) -> Result<(), ContextError> {
        self.expect(b'"')?;
        loop {
            let byte = self.next().ok_or_else(malformed_error)?;
            match byte {
                b'"' => return Ok(()),
                b'\\' => {
                    self.parse_escape()?;
                }
                0x00..=0x1f => return malformed(),
                _ => {}
            }
        }
    }

    fn parse_escape(&mut self) -> Result<char, ContextError> {
        match self.next().ok_or_else(malformed_error)? {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'b' => Ok('\u{0008}'),
            b'f' => Ok('\u{000c}'),
            b'n' => Ok('\n'),
            b'r' => Ok('\r'),
            b't' => Ok('\t'),
            b'u' => self.parse_unicode_escape(),
            _ => malformed(),
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, ContextError> {
        let high = self.parse_hex_quad()?;
        let scalar = if (0xd800..=0xdbff).contains(&high) {
            if self.next() != Some(b'\\') || self.next() != Some(b'u') {
                return malformed();
            }
            let low = self.parse_hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&low) {
                return malformed();
            }
            0x1_0000 + ((u32::from(high) - 0xd800) << 10) + (u32::from(low) - 0xdc00)
        } else if (0xdc00..=0xdfff).contains(&high) {
            return malformed();
        } else {
            u32::from(high)
        };
        char::from_u32(scalar).ok_or_else(malformed_error)
    }

    fn parse_hex_quad(&mut self) -> Result<u16, ContextError> {
        let mut value = 0_u16;
        for _ in 0..4 {
            let digit = match self.next().ok_or_else(malformed_error)? {
                byte @ b'0'..=b'9' => byte - b'0',
                byte @ b'a'..=b'f' => byte - b'a' + 10,
                byte @ b'A'..=b'F' => byte - b'A' + 10,
                _ => return malformed(),
            };
            value = (value << 4) | u16::from(digit);
        }
        Ok(value)
    }

    fn parse_literal(&mut self, expected: &[u8]) -> Result<(), ContextError> {
        if self
            .bytes
            .get(self.offset..self.offset.saturating_add(expected.len()))
            != Some(expected)
        {
            return malformed();
        }
        self.offset += expected.len();
        self.require_value_boundary()
    }

    fn parse_number(&mut self) -> Result<(), ContextError> {
        if self.consume(b'-') && self.peek().is_none() {
            return malformed();
        }
        match self.peek() {
            Some(b'0') => {
                self.offset += 1;
                if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    return malformed();
                }
            }
            Some(b'1'..=b'9') => {
                self.offset += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.offset += 1;
                }
            }
            _ => return malformed(),
        }
        if self.consume(b'.') {
            if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return malformed();
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.offset += 1;
            }
        }
        if self.peek().is_some_and(|byte| matches!(byte, b'e' | b'E')) {
            self.offset += 1;
            if self.peek().is_some_and(|byte| matches!(byte, b'+' | b'-')) {
                self.offset += 1;
            }
            if !self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                return malformed();
            }
            while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                self.offset += 1;
            }
        }
        self.require_value_boundary()
    }

    fn require_value_boundary(&self) -> Result<(), ContextError> {
        if self
            .peek()
            .is_none_or(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}'))
        {
            Ok(())
        } else {
            malformed()
        }
    }

    fn check_depth(&self, depth: usize) -> Result<(), ContextError> {
        if depth > MAX_DEPTH {
            malformed()
        } else {
            Ok(())
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.offset += 1;
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), ContextError> {
        if self.consume(expected) {
            Ok(())
        } else {
            malformed()
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.offset += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.offset).copied()
    }

    fn next(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.offset += 1;
        Some(byte)
    }
}

fn malformed<T>() -> Result<T, ContextError> {
    Err(malformed_error())
}

fn malformed_error() -> ContextError {
    ContextError::malformed("invalid UTF-8 excluded-record classifier rejected the record")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid_tool_output() -> Vec<u8> {
        let mut bytes = br#"{"timestamp":"x","type":"response_item","payload":{"type":"custom_tool_call_output","id":"one","output":"before "#.to_vec();
        bytes.push(0xf8);
        bytes.extend_from_slice(br#" after"}}"#);
        bytes
    }

    #[test]
    fn accepts_only_the_closed_excluded_tool_output_shape() {
        let bytes = invalid_tool_output();
        let mut parser = RawJsonParser::new(&bytes);
        parser.skip_whitespace();
        let parsed = parser.parse_object(ObjectScope::Root, 1);
        assert!(
            parsed.is_ok(),
            "classifier rejected at byte {} of {}",
            parser.offset,
            bytes.len()
        );
        parser.skip_whitespace();
        assert_eq!(parser.offset, bytes.len());
        assert!(parser.metadata.is_skippable());
        assert!(is_skippable_invalid_utf8_record(&bytes));

        let mut visible = br#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":""#.to_vec();
        visible.push(0xf8);
        visible.extend_from_slice(br#""}]}}"#);
        assert!(!is_skippable_invalid_utf8_record(&visible));
    }

    #[test]
    fn rejects_polyglots_duplicates_unknowns_and_structural_high_bytes() {
        for bytes in [
            br#"{"type":"response_item","payload":{"type":"custom_tool_call_output","content":"hidden"}}"#.to_vec(),
            br#"{"type":"response_item","type":"event_msg","payload":{"type":"custom_tool_call_output","output":"x"}}"#.to_vec(),
            br#"{"type":"response_item","payload":{"type":"future_output","output":"x"}}"#.to_vec(),
            br#"{"type":"response_item","payload":{"type":"custom_tool_call_output","payload":{}}}"#.to_vec(),
            br#"{"type":"response_item","payload":{"type":"custom_tool_call_output","output":{"content":"hidden"}}}"#.to_vec(),
            br#"{"type":"response_item","payload":{"type":"custom_tool_call_output","output":[{"nested":{"text":"hidden"}}]}}"#.to_vec(),
            br#"{"type":"response_item","payload":{"type":"custom_tool_call_output","output":{"payload":{"value":"hidden"}}}}"#.to_vec(),
        ] {
            assert!(!is_skippable_invalid_utf8_record(&bytes));
        }

        let mut structural = br#"{"type":"response_item","payload":"#.to_vec();
        structural.push(0xf8);
        structural.extend_from_slice(br#"{"type":"custom_tool_call_output"}}"#);
        assert!(!is_skippable_invalid_utf8_record(&structural));
    }

    #[test]
    fn escaped_duplicate_keys_and_invalid_metadata_fail_closed() {
        let duplicate = br#"{"type":"response_item","payload":{"type":"custom_tool_call_output","\u0074ype":"custom_tool_call_output","output":"x"}}"#;
        assert!(!is_skippable_invalid_utf8_record(duplicate));

        let mut metadata = br#"{"type":"response_item","payload":{"ty"#.to_vec();
        metadata.push(0xf8);
        metadata.extend_from_slice(br#"pe":"custom_tool_call_output","output":"x"}}"#);
        assert!(!is_skippable_invalid_utf8_record(&metadata));
    }
}
