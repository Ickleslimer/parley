//! Bounded streaming validation for physical rollout records above the normal
//! retention limit. The scanner validates one complete JSON value while
//! retaining only root `type` metadata.

use super::error::ContextError;

const MAX_NESTING: usize = 256;
const CAPTURE_LIMIT: usize = 128;

const EXCLUDED_ROOT_TYPES: &[&str] = &["compacted", "event_msg", "session_meta", "world_state"];

pub(crate) struct OversizedJsonScanner {
    lexer: Lexer,
    parser: Parser,
}

impl OversizedJsonScanner {
    pub(crate) fn new() -> Self {
        Self {
            lexer: Lexer::new(),
            parser: Parser::new(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<(), ContextError> {
        for byte in bytes {
            let mut consumed = false;
            while !consumed {
                let capture = self.parser.capture_mode();
                let step = self.lexer.feed(*byte, capture)?;
                if let Some(token) = step.token {
                    self.parser.accept(token)?;
                }
                consumed = step.consumed;
            }
        }
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), ContextError> {
        if let Some(token) = self.lexer.finish()? {
            self.parser.accept(token)?;
        }
        self.parser.finish()
    }
}

#[derive(Clone, Copy)]
enum CaptureMode {
    None,
    RootKey,
    RootTypeValue,
}

struct LexStep {
    token: Option<Token>,
    consumed: bool,
}

impl LexStep {
    fn consumed(token: Option<Token>) -> Self {
        Self {
            token,
            consumed: true,
        }
    }

    fn reprocess(token: Token) -> Self {
        Self {
            token: Some(token),
            consumed: false,
        }
    }
}

enum Token {
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Colon,
    Comma,
    String(CapturedString),
    Scalar,
}

struct CapturedString {
    value: Option<String>,
    overflowed: bool,
}

enum LexState {
    Ready,
    String(StringLexer),
    Number(NumberState),
    Literal {
        expected: &'static [u8],
        index: usize,
    },
}

struct Lexer {
    state: LexState,
}

impl Lexer {
    fn new() -> Self {
        Self {
            state: LexState::Ready,
        }
    }

    fn feed(&mut self, byte: u8, capture: CaptureMode) -> Result<LexStep, ContextError> {
        match &mut self.state {
            LexState::Ready => match byte {
                b' ' | b'\t' | b'\r' | b'\n' => Ok(LexStep::consumed(None)),
                b'{' => Ok(LexStep::consumed(Some(Token::LeftBrace))),
                b'}' => Ok(LexStep::consumed(Some(Token::RightBrace))),
                b'[' => Ok(LexStep::consumed(Some(Token::LeftBracket))),
                b']' => Ok(LexStep::consumed(Some(Token::RightBracket))),
                b':' => Ok(LexStep::consumed(Some(Token::Colon))),
                b',' => Ok(LexStep::consumed(Some(Token::Comma))),
                b'"' => {
                    self.state = LexState::String(StringLexer::new(capture));
                    Ok(LexStep::consumed(None))
                }
                b'-' => {
                    self.state = LexState::Number(NumberState::Minus);
                    Ok(LexStep::consumed(None))
                }
                b'0' => {
                    self.state = LexState::Number(NumberState::Zero);
                    Ok(LexStep::consumed(None))
                }
                b'1'..=b'9' => {
                    self.state = LexState::Number(NumberState::Integer);
                    Ok(LexStep::consumed(None))
                }
                b't' => {
                    self.state = LexState::Literal {
                        expected: b"true",
                        index: 1,
                    };
                    Ok(LexStep::consumed(None))
                }
                b'f' => {
                    self.state = LexState::Literal {
                        expected: b"false",
                        index: 1,
                    };
                    Ok(LexStep::consumed(None))
                }
                b'n' => {
                    self.state = LexState::Literal {
                        expected: b"null",
                        index: 1,
                    };
                    Ok(LexStep::consumed(None))
                }
                _ => Err(ContextError::malformed(
                    "oversized JSON record contains an invalid token",
                )),
            },
            LexState::String(string) => match string.feed(byte)? {
                StringStep::Continue => Ok(LexStep::consumed(None)),
                StringStep::Complete(value) => {
                    self.state = LexState::Ready;
                    Ok(LexStep::consumed(Some(Token::String(value))))
                }
            },
            LexState::Number(number) => match number.feed(byte)? {
                NumberStep::Continue => Ok(LexStep::consumed(None)),
                NumberStep::Complete => {
                    self.state = LexState::Ready;
                    Ok(LexStep::reprocess(Token::Scalar))
                }
            },
            LexState::Literal { expected, index } => {
                if expected.get(*index) != Some(&byte) {
                    return Err(ContextError::malformed(
                        "oversized JSON record contains an invalid literal",
                    ));
                }
                *index += 1;
                if *index == expected.len() {
                    self.state = LexState::Ready;
                    Ok(LexStep::consumed(Some(Token::Scalar)))
                } else {
                    Ok(LexStep::consumed(None))
                }
            }
        }
    }

    fn finish(&mut self) -> Result<Option<Token>, ContextError> {
        match &self.state {
            LexState::Ready => Ok(None),
            LexState::Number(number) if number.can_end() => {
                self.state = LexState::Ready;
                Ok(Some(Token::Scalar))
            }
            LexState::Number(_) => Err(ContextError::malformed(
                "oversized JSON record ends inside a number",
            )),
            LexState::String(_) => Err(ContextError::malformed(
                "oversized JSON record ends inside a string",
            )),
            LexState::Literal { .. } => Err(ContextError::malformed(
                "oversized JSON record ends inside a literal",
            )),
        }
    }
}

#[derive(Clone, Copy)]
enum NumberState {
    Minus,
    Zero,
    Integer,
    FractionStart,
    Fraction,
    ExponentStart,
    ExponentSign,
    Exponent,
}

enum NumberStep {
    Continue,
    Complete,
}

impl NumberState {
    fn feed(&mut self, byte: u8) -> Result<NumberStep, ContextError> {
        use NumberState::*;
        match (*self, byte) {
            (Minus, b'0') => *self = Zero,
            (Minus, b'1'..=b'9') => *self = Integer,
            (Minus, _) => return number_error(),
            (Zero, b'.') => *self = FractionStart,
            (Zero, b'e' | b'E') => *self = ExponentStart,
            (Zero, b'0'..=b'9') => return number_error(),
            (Integer, b'0'..=b'9') => {}
            (Integer, b'.') => *self = FractionStart,
            (Integer, b'e' | b'E') => *self = ExponentStart,
            (FractionStart, b'0'..=b'9') => *self = Fraction,
            (FractionStart, _) => return number_error(),
            (Fraction, b'0'..=b'9') => {}
            (Fraction, b'e' | b'E') => *self = ExponentStart,
            (ExponentStart, b'+' | b'-') => *self = ExponentSign,
            (ExponentStart, b'0'..=b'9') => *self = Exponent,
            (ExponentStart, _) => return number_error(),
            (ExponentSign, b'0'..=b'9') => *self = Exponent,
            (ExponentSign, _) => return number_error(),
            (Exponent, b'0'..=b'9') => {}
            (state, byte) if state.can_end() && is_value_delimiter(byte) => {
                return Ok(NumberStep::Complete)
            }
            _ => return number_error(),
        }
        Ok(NumberStep::Continue)
    }

    fn can_end(self) -> bool {
        matches!(
            self,
            NumberState::Zero
                | NumberState::Integer
                | NumberState::Fraction
                | NumberState::Exponent
        )
    }
}

fn number_error<T>() -> Result<T, ContextError> {
    Err(ContextError::malformed(
        "oversized JSON record contains an invalid number",
    ))
}

fn is_value_delimiter(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}')
}

enum StringStep {
    Continue,
    Complete(CapturedString),
}

enum StringMode {
    Normal,
    Escape,
    Unicode {
        value: u16,
        digits: u8,
        low_surrogate: bool,
    },
    HighSurrogateBackslash,
    HighSurrogateU,
}

struct Utf8Sequence {
    remaining: u8,
    value: u32,
    minimum: u32,
}

struct StringLexer {
    mode: StringMode,
    utf8: Option<Utf8Sequence>,
    high_surrogate: Option<u16>,
    captured: Option<String>,
    overflowed: bool,
}

impl StringLexer {
    fn new(capture: CaptureMode) -> Self {
        Self {
            mode: StringMode::Normal,
            utf8: None,
            high_surrogate: None,
            captured: if matches!(capture, CaptureMode::None) {
                None
            } else {
                Some(String::new())
            },
            overflowed: false,
        }
    }

    fn feed(&mut self, byte: u8) -> Result<StringStep, ContextError> {
        if let Some(sequence) = &mut self.utf8 {
            if !(0x80..=0xBF).contains(&byte) {
                return string_error("invalid UTF-8 continuation");
            }
            sequence.value = (sequence.value << 6) | u32::from(byte & 0x3F);
            sequence.remaining -= 1;
            if sequence.remaining == 0 {
                let value = sequence.value;
                if value < sequence.minimum
                    || value > 0x10_FFFF
                    || (0xD800..=0xDFFF).contains(&value)
                {
                    return string_error("invalid UTF-8 scalar");
                }
                self.utf8 = None;
                self.capture_char(char::from_u32(value).ok_or_else(|| {
                    ContextError::malformed("oversized JSON string has an invalid UTF-8 scalar")
                })?);
            }
            return Ok(StringStep::Continue);
        }

        match self.mode {
            StringMode::Normal => match byte {
                b'"' => Ok(StringStep::Complete(CapturedString {
                    value: self.captured.take(),
                    overflowed: self.overflowed,
                })),
                b'\\' => {
                    self.mode = StringMode::Escape;
                    Ok(StringStep::Continue)
                }
                0x00..=0x1F => string_error("unescaped control character"),
                0x20..=0x7F => {
                    self.capture_char(byte as char);
                    Ok(StringStep::Continue)
                }
                0xC2..=0xDF => {
                    self.utf8 = Some(Utf8Sequence {
                        remaining: 1,
                        value: u32::from(byte & 0x1F),
                        minimum: 0x80,
                    });
                    Ok(StringStep::Continue)
                }
                0xE0..=0xEF => {
                    self.utf8 = Some(Utf8Sequence {
                        remaining: 2,
                        value: u32::from(byte & 0x0F),
                        minimum: 0x800,
                    });
                    Ok(StringStep::Continue)
                }
                0xF0..=0xF4 => {
                    self.utf8 = Some(Utf8Sequence {
                        remaining: 3,
                        value: u32::from(byte & 0x07),
                        minimum: 0x1_0000,
                    });
                    Ok(StringStep::Continue)
                }
                _ => string_error("invalid UTF-8 leading byte"),
            },
            StringMode::Escape => {
                match byte {
                    b'"' => self.capture_char('"'),
                    b'\\' => self.capture_char('\\'),
                    b'/' => self.capture_char('/'),
                    b'b' => self.capture_char('\u{0008}'),
                    b'f' => self.capture_char('\u{000c}'),
                    b'n' => self.capture_char('\n'),
                    b'r' => self.capture_char('\r'),
                    b't' => self.capture_char('\t'),
                    b'u' => {
                        self.mode = StringMode::Unicode {
                            value: 0,
                            digits: 0,
                            low_surrogate: false,
                        };
                        return Ok(StringStep::Continue);
                    }
                    _ => return string_error("invalid string escape"),
                }
                self.mode = StringMode::Normal;
                Ok(StringStep::Continue)
            }
            StringMode::Unicode {
                ref mut value,
                ref mut digits,
                low_surrogate,
            } => {
                let Some(nibble) = hex_value(byte) else {
                    return string_error("invalid Unicode escape");
                };
                *value = (*value << 4) | u16::from(nibble);
                *digits += 1;
                if *digits < 4 {
                    return Ok(StringStep::Continue);
                }
                let completed = *value;
                if low_surrogate {
                    if !(0xDC00..=0xDFFF).contains(&completed) {
                        return string_error("invalid low surrogate");
                    }
                    let high = self.high_surrogate.take().ok_or_else(|| {
                        ContextError::malformed(
                            "oversized JSON string lost its high surrogate state",
                        )
                    })?;
                    let scalar = 0x1_0000
                        + ((u32::from(high) - 0xD800) << 10)
                        + (u32::from(completed) - 0xDC00);
                    self.capture_char(char::from_u32(scalar).ok_or_else(|| {
                        ContextError::malformed(
                            "oversized JSON string has an invalid surrogate pair",
                        )
                    })?);
                    self.mode = StringMode::Normal;
                } else if (0xD800..=0xDBFF).contains(&completed) {
                    self.high_surrogate = Some(completed);
                    self.mode = StringMode::HighSurrogateBackslash;
                } else if (0xDC00..=0xDFFF).contains(&completed) {
                    return string_error("unexpected low surrogate");
                } else {
                    self.capture_char(char::from_u32(u32::from(completed)).ok_or_else(|| {
                        ContextError::malformed(
                            "oversized JSON string has an invalid Unicode escape",
                        )
                    })?);
                    self.mode = StringMode::Normal;
                }
                Ok(StringStep::Continue)
            }
            StringMode::HighSurrogateBackslash => {
                if byte != b'\\' {
                    return string_error("high surrogate is missing a low surrogate");
                }
                self.mode = StringMode::HighSurrogateU;
                Ok(StringStep::Continue)
            }
            StringMode::HighSurrogateU => {
                if byte != b'u' {
                    return string_error("high surrogate is missing a low surrogate");
                }
                self.mode = StringMode::Unicode {
                    value: 0,
                    digits: 0,
                    low_surrogate: true,
                };
                Ok(StringStep::Continue)
            }
        }
    }

    fn capture_char(&mut self, value: char) {
        let Some(captured) = &mut self.captured else {
            return;
        };
        if self.overflowed {
            return;
        }
        if captured.len().saturating_add(value.len_utf8()) > CAPTURE_LIMIT {
            captured.clear();
            self.overflowed = true;
            return;
        }
        captured.push(value);
    }
}

fn string_error<T>(reason: &str) -> Result<T, ContextError> {
    Err(ContextError::malformed(format!(
        "oversized JSON string is malformed: {reason}"
    )))
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

enum RootState {
    ExpectObject,
    InProgress,
    Done,
}

enum Frame {
    Object(ObjectState),
    Array(ArrayState),
}

enum ObjectState {
    FirstKeyOrEnd,
    Key,
    Colon { root_type: bool },
    Value { root_type: bool },
    CommaOrEnd,
}

enum ArrayState {
    FirstValueOrEnd,
    Value,
    CommaOrEnd,
}

struct Parser {
    root: RootState,
    stack: Vec<Frame>,
    root_type_count: u8,
    root_type: Option<String>,
}

impl Parser {
    fn new() -> Self {
        Self {
            root: RootState::ExpectObject,
            stack: Vec::new(),
            root_type_count: 0,
            root_type: None,
        }
    }

    fn capture_mode(&self) -> CaptureMode {
        match self.stack.last() {
            Some(Frame::Object(ObjectState::FirstKeyOrEnd | ObjectState::Key))
                if self.stack.len() == 1 =>
            {
                CaptureMode::RootKey
            }
            Some(Frame::Object(ObjectState::Value { root_type: true }))
                if self.stack.len() == 1 =>
            {
                CaptureMode::RootTypeValue
            }
            _ => CaptureMode::None,
        }
    }

    fn accept(&mut self, token: Token) -> Result<(), ContextError> {
        if self.stack.is_empty() {
            return match self.root {
                RootState::ExpectObject => match token {
                    Token::LeftBrace => {
                        self.root = RootState::InProgress;
                        self.stack.push(Frame::Object(ObjectState::FirstKeyOrEnd));
                        Ok(())
                    }
                    _ => Err(ContextError::malformed(
                        "oversized JSON record root must be an object",
                    )),
                },
                RootState::InProgress => Err(ContextError::malformed(
                    "oversized JSON parser lost container state",
                )),
                RootState::Done => Err(ContextError::malformed(
                    "oversized JSON record contains trailing data",
                )),
            };
        }

        let depth = self.stack.len();
        match self.stack.last_mut().expect("checked non-empty") {
            Frame::Object(state) => match state {
                ObjectState::FirstKeyOrEnd => match token {
                    Token::RightBrace => self.close_container(true),
                    Token::String(key) => self.accept_key(depth, key),
                    _ => parser_error("object expected a key or closing brace"),
                },
                ObjectState::Key => match token {
                    Token::String(key) => self.accept_key(depth, key),
                    _ => parser_error("object expected a key"),
                },
                ObjectState::Colon { root_type } => match token {
                    Token::Colon => {
                        *state = ObjectState::Value {
                            root_type: *root_type,
                        };
                        Ok(())
                    }
                    _ => parser_error("object expected a colon"),
                },
                ObjectState::Value { root_type } => {
                    let root_type = *root_type;
                    *state = ObjectState::CommaOrEnd;
                    self.accept_value(token, root_type)
                }
                ObjectState::CommaOrEnd => match token {
                    Token::Comma => {
                        *state = ObjectState::Key;
                        Ok(())
                    }
                    Token::RightBrace => self.close_container(true),
                    _ => parser_error("object expected a comma or closing brace"),
                },
            },
            Frame::Array(state) => match state {
                ArrayState::FirstValueOrEnd => match token {
                    Token::RightBracket => self.close_container(false),
                    token => {
                        *state = ArrayState::CommaOrEnd;
                        self.accept_value(token, false)
                    }
                },
                ArrayState::Value => {
                    *state = ArrayState::CommaOrEnd;
                    self.accept_value(token, false)
                }
                ArrayState::CommaOrEnd => match token {
                    Token::Comma => {
                        *state = ArrayState::Value;
                        Ok(())
                    }
                    Token::RightBracket => self.close_container(false),
                    _ => parser_error("array expected a comma or closing bracket"),
                },
            },
        }
    }

    fn accept_key(&mut self, depth: usize, key: CapturedString) -> Result<(), ContextError> {
        let root_type = depth == 1
            && !key.overflowed
            && key.value.as_deref().is_some_and(|value| value == "type");
        if root_type {
            self.root_type_count = self.root_type_count.saturating_add(1);
            if self.root_type_count > 1 {
                return Err(ContextError::malformed(
                    "oversized JSON record has duplicate root type fields",
                ));
            }
        }
        let Some(Frame::Object(state)) = self.stack.last_mut() else {
            return parser_error("object key lost its container");
        };
        *state = ObjectState::Colon { root_type };
        Ok(())
    }

    fn accept_value(&mut self, token: Token, root_type: bool) -> Result<(), ContextError> {
        if root_type {
            let Token::String(value) = token else {
                return Err(ContextError::malformed(
                    "oversized JSON record root type must be a string",
                ));
            };
            if value.overflowed {
                return Err(ContextError::malformed(
                    "oversized JSON record root type exceeds the metadata limit",
                ));
            }
            self.root_type = value.value;
            return Ok(());
        }
        match token {
            Token::String(_) | Token::Scalar => Ok(()),
            Token::LeftBrace => self.push_container(Frame::Object(ObjectState::FirstKeyOrEnd)),
            Token::LeftBracket => self.push_container(Frame::Array(ArrayState::FirstValueOrEnd)),
            _ => parser_error("expected a JSON value"),
        }
    }

    fn push_container(&mut self, frame: Frame) -> Result<(), ContextError> {
        if self.stack.len() >= MAX_NESTING {
            return Err(ContextError::malformed(
                "oversized JSON record exceeds the nesting limit",
            ));
        }
        self.stack.push(frame);
        Ok(())
    }

    fn close_container(&mut self, object: bool) -> Result<(), ContextError> {
        let matches = matches!(
            (self.stack.last(), object),
            (Some(Frame::Object(_)), true) | (Some(Frame::Array(_)), false)
        );
        if !matches {
            return parser_error("mismatched JSON container terminator");
        }
        self.stack.pop();
        if self.stack.is_empty() {
            self.root = RootState::Done;
        }
        Ok(())
    }

    fn finish(self) -> Result<(), ContextError> {
        if !self.stack.is_empty() || !matches!(self.root, RootState::Done) {
            return Err(ContextError::malformed(
                "oversized JSON record ends before its root object",
            ));
        }
        if self.root_type_count != 1 || self.root_type.is_none() {
            return Err(ContextError::malformed(
                "oversized JSON record must contain exactly one root type string",
            ));
        }
        if !EXCLUDED_ROOT_TYPES.contains(&self.root_type.as_deref().unwrap()) {
            return Err(ContextError::malformed(
                "oversized JSON record has a non-excluded root type",
            ));
        }
        Ok(())
    }
}

fn parser_error<T>(reason: &str) -> Result<T, ContextError> {
    Err(ContextError::malformed(format!(
        "oversized JSON record is malformed: {reason}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(parts: &[&[u8]]) -> Result<(), ContextError> {
        let mut scanner = OversizedJsonScanner::new();
        for part in parts {
            scanner.feed(part)?;
        }
        scanner.finish()
    }

    #[test]
    fn accepts_known_excluded_root_types_across_chunks() {
        scan(&[
            br#"{"payload":{"text":"caf"#,
            "é😀".as_bytes(),
            br#"\uD83D\uDE00"},"type":"compacted"}"#,
        ])
        .unwrap();
    }

    #[test]
    fn accepts_escaped_root_type_key() {
        scan(&[br#"{"\u0074ype":"event_msg","payload":[]}"#]).unwrap();
    }

    #[test]
    fn accepts_every_known_excluded_root_type() {
        for record_type in EXCLUDED_ROOT_TYPES {
            let record = format!(r#"{{"type":"{record_type}","payload":{{}}}}"#);
            scan(&[record.as_bytes()]).unwrap();
        }
    }

    #[test]
    fn rejects_duplicate_nested_only_and_eligible_types() {
        assert!(scan(&[br#"{"type":"compacted","type":"compacted"}"#]).is_err());
        assert!(scan(&[br#"{"payload":{"type":"compacted"}}"#]).is_err());
        assert!(scan(&[br#"{"type":"response_item","payload":{}}"#]).is_err());
        assert!(scan(&[br#"{"type":"turn_context","payload":{}}"#]).is_err());
    }

    #[test]
    fn rejects_invalid_utf8_and_malformed_json() {
        assert!(scan(&[b"{\"type\":\"compacted\",\"payload\":\"", &[0xFF], b"\"}"]).is_err());
        assert!(scan(&[br#"{"type":"compacted","payload":[1,]}"#]).is_err());
        assert!(scan(&[br#"{"type":"compacted"} trailing"#]).is_err());
    }
}
