use crate::schema::HealthError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthBinary {
    Query,
    Supervisor,
    Hook,
}

impl HealthBinary {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Supervisor => "supervisor",
            Self::Hook => "hook",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandAnalysis {
    pub health_binary: Option<HealthBinary>,
    pub executable_token: Option<String>,
    pub extra_arguments: bool,
    pub wrapper: bool,
    pub chaining: bool,
    pub redirect: bool,
}

impl CommandAnalysis {
    pub fn unrelated(&self) -> bool {
        self.health_binary.is_none()
    }

    pub fn is_exact_query(&self) -> bool {
        self.health_binary == Some(HealthBinary::Query)
            && !self.extra_arguments
            && !self.wrapper
            && !self.chaining
            && !self.redirect
    }

    pub fn is_health_invocation(&self) -> bool {
        self.health_binary.is_some()
    }
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

pub fn analyze_command(command: &str) -> CommandAnalysis {
    let (chaining, redirect) = scan_operators(command);
    let tokens = tokenize(command);
    let mut tokens = tokens.as_slice();
    let mut wrapper = false;
    if tokens.first().map(String::as_str) == Some("&") {
        tokens = &tokens[1..];
    }
    if let Some(first) = tokens.first() {
        if is_wrapper(first) {
            wrapper = true;
        }
    }
    let mut health_binary = None;
    let mut executable_token = None;
    for token in tokens {
        if let Some(kind) = health_binary_from_token(token) {
            health_binary = Some(kind);
            executable_token = Some(token.clone());
            break;
        }
    }
    let extra_arguments = match (health_binary, executable_token.as_deref()) {
        (Some(HealthBinary::Query), Some(exe)) => {
            let position = tokens.iter().position(|token| token == exe);
            match position {
                Some(index) => tokens.len() > index + 1,
                None => true,
            }
        }
        (Some(_), _) => tokens.len() > 1,
        _ => false,
    };
    if health_binary.is_none() {
        wrapper = false;
    }
    CommandAnalysis {
        health_binary,
        executable_token,
        extra_arguments,
        wrapper,
        chaining: chaining && health_binary.is_some(),
        redirect: redirect && health_binary.is_some(),
    }
}

pub fn health_binary_from_token(token: &str) -> Option<HealthBinary> {
    let name = filename(token).to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    match stem {
        "parley-health-query" => Some(HealthBinary::Query),
        "parley-health-supervisor" => Some(HealthBinary::Supervisor),
        "parley-health-hook" => Some(HealthBinary::Hook),
        _ => None,
    }
}

pub fn tokenize(command: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in command.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
            }
            '"' if !in_single => {
                in_double = !in_double;
            }
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
    tokens
}

pub fn filename(token: &str) -> String {
    let normalized = token.replace('/', "\\");
    normalized
        .rsplit('\\')
        .next()
        .unwrap_or(&normalized)
        .to_string()
}

fn is_wrapper(token: &str) -> bool {
    let name = filename(token).to_ascii_lowercase();
    let stem = name.strip_suffix(".exe").unwrap_or(&name);
    WRAPPERS.contains(&stem)
}

fn scan_operators(command: &str) -> (bool, bool) {
    let mut chaining = false;
    let mut redirect = false;
    let mut in_single = false;
    let mut in_double = false;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '\'' && !in_double {
            in_single = !in_single;
            i += 1;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            i += 1;
            continue;
        }
        if in_single || in_double {
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        if (ch == '&' && next == Some('&')) || (ch == '|' && next == Some('|')) {
            chaining = true;
            i += 2;
            continue;
        }
        if ch == '|' || ch == ';' {
            chaining = true;
            i += 1;
            continue;
        }
        if ch == '&' {
            if !is_leading_call_operator(&chars, i) {
                chaining = true;
            }
            i += 1;
            continue;
        }
        if ch == '>' || ch == '<' {
            redirect = true;
        }
        i += 1;
    }
    (chaining, redirect)
}

fn is_leading_call_operator(chars: &[char], index: usize) -> bool {
    chars[..index].iter().all(|ch| ch.is_whitespace())
}

pub fn parse_tool_command(tool_name: &str, tool_input: &serde_json::Value) -> Result<Option<String>, HealthError> {
    if !is_shell_tool(tool_name) {
        return Ok(None);
    }
    Ok(extract_command_string(tool_input))
}

pub fn is_shell_tool(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "bash" | "shell" | "powershell" | "pwsh" | "cmd" | "terminal"
    )
}

fn extract_command_string(tool_input: &serde_json::Value) -> Option<String> {
    let object = tool_input.as_object()?;
    for key in ["command", "cmd", "command_line", "commandLine", "script"] {
        if let Some(serde_json::Value::String(text)) = object.get(key) {
            return Some(text.clone());
        }
    }
    None
}
