#![allow(dead_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivityClass {
    Thought,
    ToolCall,
    ToolCallUpdate,
    Text,
    Usage,
    Plan,
    AvailableCommands,
    End,
    Error,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SafeActivity {
    pub class: ActivityClass,
    pub timestamp_ms: u128,
    pub tool_name: Option<String>,
    pub status: Option<String>,
    pub visible_text: Option<String>,
}

pub(crate) fn decode_stream_line(
    _line: &str,
    _timestamp_ms: u128,
) -> Result<Option<SafeActivity>, String> {
    Ok(None)
}
