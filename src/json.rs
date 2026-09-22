use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(f64),
    Str(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    pub(crate) fn parse(input: &str) -> Result<Self, String> {
        let mut parser = Parser::new(input);
        let value = parser.parse_value()?;
        parser.skip_whitespace();
        Ok(value)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(map) => map.get(key),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub(crate) fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub(crate) fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(arr) => Some(arr),
            _ => None,
        }
    }

    pub(crate) fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Object(map) => Some(map),
            _ => None,
        }
    }

    pub(crate) fn as_number(&self) -> Option<f64> {
        match self {
            Json::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub(crate) fn to_pretty_string(&self) -> String {
        let mut buf = String::new();
        write_pretty(&mut buf, self, 0);
        buf.push('\n');
        buf
    }

    /// Single-line serialization with no embedded newlines. Required for
    /// newline-delimited transports such as MCP's JSON-RPC over stdio.
    pub(crate) fn to_compact_string(&self) -> String {
        let mut buf = String::new();
        write_compact(&mut buf, self);
        buf
    }
}

fn write_compact(buf: &mut String, value: &Json) {
    match value {
        Json::Null => buf.push_str("null"),
        Json::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => {
            if *n == (*n as i64) as f64 {
                buf.push_str(&(*n as i64).to_string());
            } else {
                buf.push_str(&n.to_string());
            }
        }
        Json::Str(s) => {
            buf.push('"');
            buf.push_str(&escape_json(s));
            buf.push('"');
        }
        Json::Array(arr) => {
            buf.push('[');
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                write_compact(buf, item);
            }
            buf.push(']');
        }
        Json::Object(map) => {
            buf.push('{');
            for (i, (key, val)) in map.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                buf.push('"');
                buf.push_str(&escape_json(key));
                buf.push_str("\":");
                write_compact(buf, val);
            }
            buf.push('}');
        }
    }
}

fn write_pretty(buf: &mut String, value: &Json, indent: usize) {
    match value {
        Json::Null => buf.push_str("null"),
        Json::Bool(b) => buf.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => {
            if *n == (*n as i64) as f64 {
                buf.push_str(&(*n as i64).to_string());
            } else {
                buf.push_str(&n.to_string());
            }
        }
        Json::Str(s) => {
            buf.push('"');
            buf.push_str(&escape_json(s));
            buf.push('"');
        }
        Json::Array(arr) => {
            if arr.is_empty() {
                buf.push_str("[]");
                return;
            }
            buf.push_str("[\n");
            for (i, item) in arr.iter().enumerate() {
                push_indent(buf, indent + 1);
                write_pretty(buf, item, indent + 1);
                if i + 1 < arr.len() {
                    buf.push(',');
                }
                buf.push('\n');
            }
            push_indent(buf, indent);
            buf.push(']');
        }
        Json::Object(map) => {
            if map.is_empty() {
                buf.push_str("{}");
                return;
            }
            buf.push_str("{\n");
            let entries: Vec<_> = map.iter().collect();
            for (i, (key, val)) in entries.iter().enumerate() {
                push_indent(buf, indent + 1);
                buf.push('"');
                buf.push_str(&escape_json(key));
                buf.push_str("\": ");
                write_pretty(buf, val, indent + 1);
                if i + 1 < entries.len() {
                    buf.push(',');
                }
                buf.push('\n');
            }
            push_indent(buf, indent);
            buf.push('}');
        }
    }
}

fn push_indent(buf: &mut String, level: usize) {
    for _ in 0..level {
        buf.push_str("  ");
    }
}

pub(crate) fn escape_json(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            c => vec![c],
        })
        .collect()
}

struct Parser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn next_byte(&mut self) -> Option<u8> {
        let byte = self.input.get(self.pos).copied()?;
        self.pos += 1;
        Some(byte)
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\n' || b == b'\r' || b == b'\t' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        self.skip_whitespace();
        match self.next_byte() {
            Some(b) if b == expected => Ok(()),
            Some(b) => Err(format!(
                "expected '{}', got '{}'",
                expected as char, b as char
            )),
            None => Err(format!("expected '{}', got end of input", expected as char)),
        }
    }

    fn parse_value(&mut self) -> Result<Json, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => self.parse_string().map(Json::Str),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'n') => self.parse_null(),
            Some(b) if b == b'-' || b.is_ascii_digit() => self.parse_number(),
            Some(b) => Err(format!("unexpected character: {}", b as char)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn parse_string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            match self.peek() {
                Some(b'\\') => {
                    self.pos += 1;
                    match self.next_byte() {
                        Some(b'"') => s.push('"'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'/') => s.push('/'),
                        Some(b'n') => s.push('\n'),
                        Some(b'r') => s.push('\r'),
                        Some(b't') => s.push('\t'),
                        Some(b'u') => s.push(self.parse_unicode_escape()?),
                        Some(b) => return Err(format!("invalid escape: \\{}", b as char)),
                        None => return Err("unexpected end in string escape".to_string()),
                    }
                }
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(s);
                }
                Some(byte) if byte < 0x20 => {
                    return Err("unescaped control character in string".to_string())
                }
                Some(byte) if byte.is_ascii() => {
                    self.pos += 1;
                    s.push(byte as char);
                }
                Some(_) => {
                    let remaining = std::str::from_utf8(&self.input[self.pos..])
                        .map_err(|_| "invalid UTF-8 in string".to_string())?;
                    let character = remaining
                        .chars()
                        .next()
                        .ok_or_else(|| "unterminated string".to_string())?;
                    self.pos += character.len_utf8();
                    s.push(character);
                }
                None => return Err("unterminated string".to_string()),
            }
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<char, String> {
        let first = self.parse_hex_quad()?;
        let code = if (0xd800..=0xdbff).contains(&first) {
            if self.next_byte() != Some(b'\\') || self.next_byte() != Some(b'u') {
                return Err("high surrogate is not followed by a low surrogate".to_string());
            }
            let second = self.parse_hex_quad()?;
            if !(0xdc00..=0xdfff).contains(&second) {
                return Err("high surrogate is not followed by a low surrogate".to_string());
            }
            0x1_0000 + (((u32::from(first) - 0xd800) << 10) | (u32::from(second) - 0xdc00))
        } else if (0xdc00..=0xdfff).contains(&first) {
            return Err("unpaired low surrogate in unicode escape".to_string());
        } else {
            u32::from(first)
        };
        char::from_u32(code).ok_or_else(|| "invalid unicode scalar in escape".to_string())
    }

    fn parse_hex_quad(&mut self) -> Result<u16, String> {
        let start = self.pos;
        let end = start.saturating_add(4);
        let bytes = self
            .input
            .get(start..end)
            .ok_or_else(|| "incomplete unicode escape".to_string())?;
        if !bytes.iter().all(u8::is_ascii_hexdigit) {
            return Err("invalid unicode escape".to_string());
        }
        self.pos = end;
        let text = std::str::from_utf8(bytes).expect("ASCII hex is UTF-8");
        u16::from_str_radix(text, 16).map_err(|_| "invalid unicode escape".to_string())
    }

    fn parse_object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.expect(b':')?;
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(map));
                }
                _ => return Err("expected ',' or '}' in object".to_string()),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut arr = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(arr));
        }
        loop {
            arr.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                }
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(arr));
                }
                _ => return Err("expected ',' or ']' in array".to_string()),
            }
        }
    }

    fn parse_bool(&mut self) -> Result<Json, String> {
        if self.try_consume(b"true") {
            Ok(Json::Bool(true))
        } else if self.try_consume(b"false") {
            Ok(Json::Bool(false))
        } else {
            Err("expected boolean".to_string())
        }
    }

    fn parse_null(&mut self) -> Result<Json, String> {
        if self.try_consume(b"null") {
            Ok(Json::Null)
        } else {
            Err("expected null".to_string())
        }
    }

    fn parse_number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            self.pos += 1;
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                self.pos += 1;
            }
            while let Some(b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
        }
        let s = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| "invalid utf8 in number")?;
        s.parse::<f64>()
            .map(Json::Number)
            .map_err(|_| format!("invalid number: {s}"))
    }

    fn try_consume(&mut self, literal: &[u8]) -> bool {
        if self.input[self.pos..].starts_with(literal) {
            self.pos += literal.len();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_object() {
        let input = r#"{"key": "value", "num": 42, "flag": true}"#;
        let json = Json::parse(input).unwrap();
        assert_eq!(json.get("key").unwrap().as_str(), Some("value"));
        assert_eq!(json.get("flag").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn parses_nested_mcp_config() {
        let input = r#"{
  "mcpServers": {
    "playwright": {
      "command": "npx",
      "args": ["-y", "@anthropic-ai/mcp-playwright"]
    }
  }
}"#;
        let json = Json::parse(input).unwrap();
        let servers = json.get("mcpServers").unwrap();
        let pw = servers.get("playwright").unwrap();
        assert_eq!(pw.get("command").unwrap().as_str(), Some("npx"));
        let args = pw.get("args").unwrap().as_array().unwrap();
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].as_str(), Some("-y"));
    }

    #[test]
    fn pretty_prints_object() {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Json::Str("test".to_string()));
        map.insert(
            "args".to_string(),
            Json::Array(vec![Json::Str("a".to_string()), Json::Str("b".to_string())]),
        );
        let json = Json::Object(map);
        let output = json.to_pretty_string();
        assert!(output.contains("\"name\": \"test\""));
        assert!(output.contains("\"a\""));
    }

    #[test]
    fn roundtrips_complex_json() {
        let input = r#"{"a": [1, 2, 3], "b": {"nested": true}, "c": null}"#;
        let json = Json::parse(input).unwrap();
        let output = json.to_pretty_string();
        let reparsed = Json::parse(&output).unwrap();
        assert_eq!(json, reparsed);
    }

    #[test]
    fn handles_string_escapes() {
        let input = r#"{"msg": "line1\nline2\ttab"}"#;
        let json = Json::parse(input).unwrap();
        assert_eq!(json.get("msg").unwrap().as_str(), Some("line1\nline2\ttab"));
    }

    #[test]
    fn preserves_literal_utf8_and_combines_surrogate_pairs() {
        let literal = Json::parse(r#"{"msg":"猫 🌍"}"#).unwrap();
        assert_eq!(literal.get("msg").unwrap().as_str(), Some("猫 🌍"));
        let escaped = Json::parse(r#"{"msg":"\ud83c\udf0d"}"#).unwrap();
        assert_eq!(escaped.get("msg").unwrap().as_str(), Some("🌍"));
        assert!(Json::parse(r#"{"msg":"\ud83cX"}"#).is_err());
        assert!(Json::parse(r#"{"msg":"\udf0d"}"#).is_err());
    }

    #[test]
    fn handles_empty_structures() {
        assert_eq!(Json::parse("{}").unwrap(), Json::Object(BTreeMap::new()));
        assert_eq!(Json::parse("[]").unwrap(), Json::Array(Vec::new()));
    }
}
