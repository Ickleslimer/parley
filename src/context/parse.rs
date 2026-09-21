//! UTF-8-aware JSON parser for transcript records and journal metadata.
//!
//! `crate::json::Json::parse` treats string bytes as Latin-1 code points, so
//! this module parses `&str` scalar-by-scalar and then reuses `Json` for
//! field access.

use std::collections::BTreeMap;

use crate::json::Json;

use super::error::ContextError;

pub(crate) fn parse_json(input: &str) -> Result<Json, ContextError> {
    let mut parser = Parser::new(input);
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos != parser.input.len() {
        return Err(ContextError::malformed("trailing data after JSON value"));
    }
    Ok(value)
}

pub(crate) fn get_str<'a>(object: &'a Json, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Json::as_str)
}

pub(crate) fn required_str(object: &Json, key: &str) -> Result<String, ContextError> {
    get_str(object, key)
        .map(str::to_string)
        .ok_or_else(|| ContextError::malformed(format!("missing string field {key}")))
}

pub(crate) fn optional_str(object: &Json, key: &str) -> Result<Option<String>, ContextError> {
    match object.get(key) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(value)) => Ok(Some(value.clone())),
        Some(_) => Err(ContextError::malformed(format!(
            "field {key} must be a string or null"
        ))),
    }
}

pub(crate) fn required_u64(object: &Json, key: &str) -> Result<u64, ContextError> {
    let value = object
        .get(key)
        .ok_or_else(|| ContextError::malformed(format!("missing number field {key}")))?;
    json_u64(value).ok_or_else(|| ContextError::malformed(format!("field {key} is not a u64")))
}

pub(crate) fn json_u64(value: &Json) -> Option<u64> {
    let number = value.as_number()?;
    if !number.is_finite() || number.fract() != 0.0 || number < 0.0 {
        return None;
    }
    let as_u64 = number as u64;
    if as_u64 as f64 == number {
        Some(as_u64)
    } else {
        None
    }
}

pub(crate) fn required_object<'a>(object: &'a Json, key: &str) -> Result<&'a Json, ContextError> {
    match object.get(key) {
        Some(value @ Json::Object(_)) => Ok(value),
        _ => Err(ContextError::malformed(format!(
            "missing object field {key}"
        ))),
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn rest(&self) -> &'a str {
        &self.input[self.pos..]
    }

    fn peek_byte(&self) -> Option<u8> {
        self.rest().as_bytes().first().copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek_byte() {
            if b == b' ' || b == b'\n' || b == b'\r' || b == b'\t' {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), ContextError> {
        self.skip_whitespace();
        match self.peek_byte() {
            Some(b) if b == expected => {
                self.pos += 1;
                Ok(())
            }
            Some(b) => Err(ContextError::malformed(format!(
                "expected '{}', got '{}'",
                expected as char, b as char
            ))),
            None => Err(ContextError::malformed(format!(
                "expected '{}', got end of input",
                expected as char
            ))),
        }
    }

    fn parse_value(&mut self) -> Result<Json, ContextError> {
        self.skip_whitespace();
        match self.peek_byte() {
            Some(b'"') => self.parse_string().map(Json::Str),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'n') => self.parse_null(),
            Some(b) if b == b'-' || b.is_ascii_digit() => self.parse_number(),
            Some(b) => Err(ContextError::malformed(format!(
                "unexpected character: {}",
                b as char
            ))),
            None => Err(ContextError::malformed("unexpected end of input")),
        }
    }

    fn parse_string(&mut self) -> Result<String, ContextError> {
        self.expect_byte(b'"')?;
        let mut out = String::new();
        loop {
            let rest = self.rest();
            if rest.is_empty() {
                return Err(ContextError::malformed("unterminated string"));
            }
            if rest.starts_with('\\') {
                self.pos += 1;
                let escape = self.rest();
                if escape.is_empty() {
                    return Err(ContextError::malformed("unterminated string escape"));
                }
                let marker = escape.as_bytes()[0];
                match marker {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let first = self.parse_unicode_escape()?;
                        let code = if (0xD800..=0xDBFF).contains(&first) {
                            if !self.rest().starts_with("\\u") {
                                return Err(ContextError::malformed(
                                    "high surrogate is missing a low surrogate",
                                ));
                            }
                            self.pos += 1;
                            let low = self.parse_unicode_escape()?;
                            if !(0xDC00..=0xDFFF).contains(&low) {
                                return Err(ContextError::malformed(
                                    "high surrogate is followed by an invalid low surrogate",
                                ));
                            }
                            0x1_0000 + ((first - 0xD800) << 10) + (low - 0xDC00)
                        } else if (0xDC00..=0xDFFF).contains(&first) {
                            return Err(ContextError::malformed("unexpected low surrogate"));
                        } else {
                            first
                        };
                        out.push(char::from_u32(code).ok_or_else(|| {
                            ContextError::malformed("unicode escape is not a scalar value")
                        })?);
                        continue;
                    }
                    other => {
                        return Err(ContextError::malformed(format!(
                            "invalid escape: \\{}",
                            other as char
                        )));
                    }
                }
                self.pos += 1;
                continue;
            }
            if rest.starts_with('"') {
                self.pos += 1;
                return Ok(out);
            }
            let ch = rest.chars().next().unwrap();
            if (ch as u32) < 0x20 {
                return Err(ContextError::malformed("unescaped control in string"));
            }
            out.push(ch);
            self.pos += ch.len_utf8();
        }
    }

    fn parse_unicode_escape(&mut self) -> Result<u32, ContextError> {
        let escape = self.rest();
        if !escape.starts_with('u') || escape.len() < 5 {
            return Err(ContextError::malformed("incomplete unicode escape"));
        }
        let hex = &escape[1..5];
        if !hex.as_bytes().iter().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ContextError::malformed(format!(
                "invalid unicode escape: \\u{hex}"
            )));
        }
        let code = u32::from_str_radix(hex, 16)
            .map_err(|_| ContextError::malformed(format!("invalid unicode escape: \\u{hex}")))?;
        self.pos += 5;
        Ok(code)
    }

    fn parse_object(&mut self) -> Result<Json, ContextError> {
        self.expect_byte(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_whitespace();
        if self.peek_byte() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string()?;
            self.expect_byte(b':')?;
            let value = self.parse_value()?;
            map.insert(key, value);
            self.skip_whitespace();
            match self.peek_byte() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Object(map));
                }
                _ => return Err(ContextError::malformed("expected ',' or '}' in object")),
            }
        }
    }

    fn parse_array(&mut self) -> Result<Json, ContextError> {
        self.expect_byte(b'[')?;
        let mut arr = Vec::new();
        self.skip_whitespace();
        if self.peek_byte() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Array(arr));
        }
        loop {
            arr.push(self.parse_value()?);
            self.skip_whitespace();
            match self.peek_byte() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Array(arr));
                }
                _ => return Err(ContextError::malformed("expected ',' or ']' in array")),
            }
        }
    }

    fn parse_bool(&mut self) -> Result<Json, ContextError> {
        if self.try_consume("true") {
            Ok(Json::Bool(true))
        } else if self.try_consume("false") {
            Ok(Json::Bool(false))
        } else {
            Err(ContextError::malformed("expected boolean"))
        }
    }

    fn parse_null(&mut self) -> Result<Json, ContextError> {
        if self.try_consume("null") {
            Ok(Json::Null)
        } else {
            Err(ContextError::malformed("expected null"))
        }
    }

    fn parse_number(&mut self) -> Result<Json, ContextError> {
        let start = self.pos;
        if self.peek_byte() == Some(b'-') {
            self.pos += 1;
        }
        while self.peek_byte().is_some_and(|b| b.is_ascii_digit()) {
            self.pos += 1;
        }
        if self.peek_byte() == Some(b'.') {
            self.pos += 1;
            while self.peek_byte().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        if self.peek_byte() == Some(b'e') || self.peek_byte() == Some(b'E') {
            self.pos += 1;
            if self.peek_byte() == Some(b'+') || self.peek_byte() == Some(b'-') {
                self.pos += 1;
            }
            while self.peek_byte().is_some_and(|b| b.is_ascii_digit()) {
                self.pos += 1;
            }
        }
        let raw = &self.input[start..self.pos];
        raw.parse::<f64>()
            .map(Json::Number)
            .map_err(|_| ContextError::malformed(format!("invalid number: {raw}")))
    }

    fn try_consume(&mut self, literal: &str) -> bool {
        if self.rest().starts_with(literal) {
            self.pos += literal.len();
            true
        } else {
            false
        }
    }
}
