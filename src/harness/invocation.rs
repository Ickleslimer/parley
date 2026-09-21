use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Invocation {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl Invocation {
    pub(crate) fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            env: BTreeMap::new(),
        }
    }

    pub(crate) fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub(crate) fn to_json(&self) -> String {
        self.to_json_with_prompt_metadata(None)
    }

    pub(crate) fn to_json_with_prompt_metadata(&self, prompt: Option<(usize, &str)>) -> String {
        let args = self
            .args
            .iter()
            .map(|arg| format!("\"{}\"", escape_json(arg)))
            .collect::<Vec<_>>()
            .join(", ");
        let env = self
            .env
            .iter()
            .map(|(key, value)| format!("\"{}\": \"{}\"", escape_json(key), escape_json(value)))
            .collect::<Vec<_>>()
            .join(", ");

        let prompt_metadata = prompt
            .map(|(chars, fingerprint)| {
                format!(
                    ",\n  \"prompt_buffer\": {{\"character_count\": {chars}, \"fingerprint\": \"{}\"}}",
                    escape_json(fingerprint)
                )
            })
            .unwrap_or_default();

        format!(
            "{{\n  \"command\": \"{}\",\n  \"args\": [{}],\n  \"env\": {{{}}}{}\n}}",
            escape_json(&self.command),
            args,
            env,
            prompt_metadata
        )
    }

    pub(crate) fn projected_windows_command_units(&self) -> usize {
        std::iter::once(&self.command)
            .chain(self.args.iter())
            .map(|value| value.encode_utf16().count().saturating_add(3))
            .sum()
    }

    pub(crate) fn replace_single_prompt_with_file(&mut self, path: &str) -> Result<(), String> {
        if self.command != "grok" {
            return Err("prompt-file transport is only supported for Grok".to_string());
        }
        let positions = self
            .args
            .iter()
            .enumerate()
            .filter_map(|(index, value)| (value == "--single").then_some(index))
            .collect::<Vec<_>>();
        if positions.len() != 1 {
            return Err("Grok invocation must contain exactly one --single prompt".to_string());
        }
        let index = positions[0];
        if index + 1 >= self.args.len() {
            return Err("Grok --single prompt is missing its value".to_string());
        }
        self.args.splice(
            index..=index + 1,
            ["--prompt-file".to_string(), path.to_string()],
        );
        Ok(())
    }
}

fn escape_json(value: &str) -> String {
    value
        .chars()
        .flat_map(|char| match char {
            '"' => "\\\"".chars().collect::<Vec<_>>(),
            '\\' => "\\\\".chars().collect::<Vec<_>>(),
            '\n' => "\\n".chars().collect::<Vec<_>>(),
            '\r' => "\\r".chars().collect::<Vec<_>>(),
            '\t' => "\\t".chars().collect::<Vec<_>>(),
            value => vec![value],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_only_the_structured_grok_prompt() {
        let mut invocation = Invocation::new(
            "grok",
            vec![
                "--single".to_string(),
                "task".to_string(),
                "--model".to_string(),
                "grok-4.6".to_string(),
            ],
        );
        invocation
            .replace_single_prompt_with_file(r"C:\Temp\prompt.txt")
            .unwrap();
        assert_eq!(
            invocation.args,
            vec![
                "--prompt-file",
                r"C:\Temp\prompt.txt",
                "--model",
                "grok-4.6"
            ]
        );
        assert!(!invocation.args.iter().any(|arg| arg == "--single"));
    }

    #[test]
    fn prompt_metadata_is_dry_run_only_json() {
        let invocation = Invocation::new("grok", vec!["--prompt-file".into(), "<path>".into()]);
        assert!(!invocation.to_json().contains("prompt_buffer"));
        let json = invocation.to_json_with_prompt_metadata(Some((42, "abcd")));
        assert!(json.contains("\"character_count\": 42"));
        assert!(json.contains("\"fingerprint\": \"abcd\""));
    }
}
