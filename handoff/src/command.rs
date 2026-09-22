use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperKind {
    Peek,
    Wait,
    Alert,
    Hook,
}

impl HelperKind {
    pub fn stem(self) -> &'static str {
        match self {
            Self::Peek => "parley-peer-peek",
            Self::Wait => "parley-handoff-wait",
            Self::Alert => "parley-peer-alert",
            Self::Hook => "parley-handoff-hook",
        }
    }

    pub fn from_stem(stem: &str) -> Option<Self> {
        Some(match stem {
            "parley-peer-peek" => Self::Peek,
            "parley-handoff-wait" => Self::Wait,
            "parley-peer-alert" => Self::Alert,
            "parley-handoff-hook" => Self::Hook,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandClass {
    Unrelated,
    Exact { kind: HelperKind, path: PathBuf },
    Denied(&'static str),
}

const WRAPPERS: &[&str] = &[
    "cmd",
    "powershell",
    "pwsh",
    "bash",
    "sh",
    "wsl",
    "start",
    "start-process",
    "invoke-expression",
    "iex",
    "invoke-command",
    "python",
    "py",
    "node",
    "cscript",
    "wscript",
];

pub fn classify_command(command: &str) -> CommandClass {
    let (tokens, valid) = tokenize_with_validity(command);
    if !valid {
        return if tokens.iter().any(|token| helper_stem(token).is_some())
            || mentions_helper(command)
        {
            CommandClass::Denied("malformed")
        } else {
            CommandClass::Unrelated
        };
    }
    let helper_at = tokens.iter().position(|token| helper_stem(token).is_some());
    let Some(index) = helper_at else {
        return CommandClass::Unrelated;
    };
    let Some((chaining, redirect)) = scan_operators(command) else {
        return CommandClass::Denied("malformed");
    };
    if chaining {
        return CommandClass::Denied("chaining");
    }
    if redirect {
        return CommandClass::Denied("redirect");
    }
    if tokens[..index].iter().any(|token| is_wrapper(token)) || tokens[0].starts_with('&') {
        return CommandClass::Denied("wrapper");
    }
    if tokens.len() != 1 {
        return CommandClass::Denied("arguments");
    }
    let token = &tokens[0];
    let Some(kind) = helper_stem(token) else {
        return CommandClass::Denied("renamed");
    };
    if kind == HelperKind::Hook {
        return CommandClass::Denied("out_of_scope");
    }
    let path = PathBuf::from(token);
    if !path.is_absolute() || filename_stem(token) != kind.stem() {
        return CommandClass::Denied("renamed");
    }
    CommandClass::Exact { kind, path }
}

pub fn is_shell_tool(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "bash" | "shell" | "powershell" | "pwsh" | "cmd" | "terminal" | "run_terminal_command"
    )
}

fn helper_stem(token: &str) -> Option<HelperKind> {
    let stem = filename_stem(token);
    if let Some(kind) = HelperKind::from_stem(&stem) {
        return Some(kind);
    }
    [
        HelperKind::Peek,
        HelperKind::Wait,
        HelperKind::Alert,
        HelperKind::Hook,
    ]
    .into_iter()
    .find(|kind| stem.starts_with(kind.stem()))
}

fn filename_stem(token: &str) -> String {
    let name = token.rsplit(['\\', '/']).next().unwrap_or(token);
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn is_wrapper(token: &str) -> bool {
    let stem = filename_stem(token);
    WRAPPERS
        .iter()
        .any(|wrapper| stem.eq_ignore_ascii_case(wrapper))
}

pub fn tokenize(command: &str) -> Option<Vec<String>> {
    let (tokens, valid) = tokenize_with_validity(command);
    valid.then_some(tokens)
}

pub fn candidate_tokens(command: &str) -> Vec<String> {
    tokenize_with_validity(command).0
}

fn tokenize_with_validity(command: &str) -> (Vec<String>, bool) {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in command.chars() {
        if ch.is_control() && !ch.is_whitespace() {
            return (tokens, false);
        }
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '`' if !in_single => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            ch if ch.is_whitespace() && !in_single && !in_double => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    (tokens, !escaped && !in_single && !in_double)
}

fn mentions_helper(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    [
        HelperKind::Peek,
        HelperKind::Wait,
        HelperKind::Alert,
        HelperKind::Hook,
    ]
    .iter()
    .any(|kind| lower.contains(kind.stem()))
}

fn scan_operators(command: &str) -> Option<(bool, bool)> {
    let mut chaining = false;
    let mut redirect = false;
    let chars: Vec<char> = command.chars().collect();
    let mut index = 0;
    let mut in_single = false;
    let mut in_double = false;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '`' && !in_single {
            index += if index + 1 < chars.len() { 2 } else { 1 };
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            index += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            index += 1;
            continue;
        }
        if in_single || in_double {
            index += 1;
            continue;
        }
        let next = chars.get(index + 1).copied();
        if (ch == '&' && next == Some('&')) || (ch == '|' && next == Some('|')) {
            chaining = true;
            index += 2;
            continue;
        }
        if ch == '|' || ch == ';' || ch == '&' {
            chaining = true;
            index += 1;
            continue;
        }
        if ch == '>' || ch == '<' {
            redirect = true;
        }
        index += 1;
    }
    if in_single || in_double {
        None
    } else {
        Some((chaining, redirect))
    }
}
