use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Invocation {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub remove_inherited_env: BTreeSet<String>,
}

impl Invocation {
    pub(crate) fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            env: BTreeMap::new(),
            remove_inherited_env: BTreeSet::new(),
        }
    }

    pub(crate) fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub(crate) fn isolate_handoff_environment(mut self) -> Self {
        self.remove_inherited_env.extend(
            [
                "PARLEY_HANDOFF_STATE_DIR",
                "PARLEY_HANDOFF_JOB_DIR",
                "PARLEY_HANDOFF_JOB_ID",
                "PARLEY_HANDOFF_ID",
                "PARLEY_HANDOFF_CAPABILITY",
                "PARLEY_HANDOFF_SOURCE_SESSION_ID",
                "PARLEY_HANDOFF_TARGET_SESSION_ID",
            ]
            .into_iter()
            .map(str::to_string),
        );
        self
    }

    pub(crate) fn configure_environment(&self, command: &mut Command) {
        for name in &self.remove_inherited_env {
            command.env_remove(name);
        }
        command.envs(&self.env);
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

        let removals = if self.remove_inherited_env.is_empty() {
            String::new()
        } else {
            format!(
                ",\n  \"remove_inherited_env\": [{}]",
                self.remove_inherited_env
                    .iter()
                    .map(|name| format!("\"{}\"", escape_json(name)))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };

        format!(
            "{{\n  \"command\": \"{}\",\n  \"args\": [{}],\n  \"env\": {{{}}}{}{}\n}}",
            escape_json(&self.command),
            args,
            env,
            removals,
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
    fn handoff_isolation_is_explicit_and_preserves_real_overrides() {
        let generic = Invocation::new("other", Vec::new());
        assert!(generic.remove_inherited_env.is_empty());
        let isolated = Invocation::new("grok", Vec::new())
            .isolate_handoff_environment()
            .with_env("PARLEY_HANDOFF_JOB_ID", "real-job");
        assert_eq!(isolated.remove_inherited_env.len(), 7);
        assert!(isolated
            .remove_inherited_env
            .contains("PARLEY_HANDOFF_STATE_DIR"));
        assert_eq!(
            isolated.env.get("PARLEY_HANDOFF_JOB_ID").unwrap(),
            "real-job"
        );
    }

    #[test]
    fn portable_environment_removes_stale_scope_before_real_job_overrides() {
        let isolated = Invocation::new("grok", Vec::new()).isolate_handoff_environment();
        let mut command = Command::new("synthetic");
        for name in &isolated.remove_inherited_env {
            command.env(name, "stale-job-A");
        }
        isolated.configure_environment(&mut command);
        for name in &isolated.remove_inherited_env {
            assert!(command
                .get_envs()
                .any(|(key, value)| key == name.as_str() && value.is_none()));
        }
        let mut bound = isolated.clone();
        for name in &bound.remove_inherited_env {
            bound.env.insert(name.clone(), "genuine-job-B".to_string());
        }
        bound.configure_environment(&mut command);
        for name in &bound.remove_inherited_env {
            assert!(command.get_envs().any(
                |(key, value)| key == name.as_str() && value == Some("genuine-job-B".as_ref())
            ));
        }
        let mut other = Command::new("other");
        other.env("PARLEY_HANDOFF_STATE_DIR", "unchanged");
        Invocation::new("other", Vec::new()).configure_environment(&mut other);
        assert!(other
            .get_envs()
            .any(|(key, value)| key == "PARLEY_HANDOFF_STATE_DIR"
                && value == Some("unchanged".as_ref())));
    }

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
