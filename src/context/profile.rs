//! Small user-approved interaction profile for long-lived collaboration.
//! This file is presentation guidance only and is never part of cursor state.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::json::Json;

use super::error::{ContextError, ErrorKind};
use super::parse::{parse_json_strict, required_u64};
use super::winfile::{open_shared_read, refuse_reparse_chain};

const MAX_PROFILE_BYTES: u64 = 16 * 1024;
const MAX_TEAM_NAME_CHARS: usize = 64;
const MAX_ITEMS_PER_SECTION: usize = 16;
const MAX_ITEM_CHARS: usize = 500;
const ALLOWED_FIELDS: &[&str] = &[
    "schema_version",
    "profile_version",
    "team_name",
    "tone",
    "interaction_preferences",
    "collaboration_preferences",
    "reporting_preferences",
];
const FORBIDDEN_AUTHORITY_TERMS: &[&str] = &[
    "authorization",
    "authorize",
    "permission",
    "credential",
    "tool access",
    "filesystem",
    "scientific disposition",
    "claim status",
];

pub(crate) fn load_interaction_profile_from_process() -> Result<Option<String>, ContextError> {
    let Some(path) = std::env::var_os("PARLEY_CONTEXT_INTERACTION_PROFILE") else {
        return Ok(None);
    };
    if path.is_empty() {
        return Ok(None);
    }
    load_interaction_profile(&PathBuf::from(path)).map(Some)
}

fn load_interaction_profile(path: &Path) -> Result<String, ContextError> {
    if !path.is_absolute() {
        return Err(ContextError::malformed(
            "PARLEY_CONTEXT_INTERACTION_PROFILE must be an absolute path",
        ));
    }
    refuse_reparse_chain(path).map_err(|_| {
        ContextError::new(
            ErrorKind::Io,
            "interaction profile path failed safety validation",
        )
    })?;
    let metadata = std::fs::metadata(path).map_err(|error| {
        ContextError::new(
            ErrorKind::Io,
            format!("read interaction profile metadata: {}", error.kind()),
        )
    })?;
    if metadata.len() > MAX_PROFILE_BYTES {
        return Err(ContextError::overflow(format!(
            "interaction profile exceeds {MAX_PROFILE_BYTES} bytes"
        )));
    }
    let mut file = open_shared_read(path).map_err(|error| {
        ContextError::new(
            ErrorKind::Io,
            format!("open interaction profile: {}", error.kind()),
        )
    })?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_PROFILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            ContextError::new(
                ErrorKind::Io,
                format!("read interaction profile: {}", error.kind()),
            )
        })?;
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err(ContextError::overflow(format!(
            "interaction profile exceeds {MAX_PROFILE_BYTES} bytes"
        )));
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| ContextError::malformed("interaction profile is not valid UTF-8"))?;
    let json = parse_json_strict(text)
        .map_err(|_| ContextError::malformed("interaction profile JSON is malformed"))?;
    let object = json
        .as_object()
        .ok_or_else(|| ContextError::malformed("interaction profile root must be an object"))?;
    if object
        .keys()
        .find(|field| !ALLOWED_FIELDS.contains(&field.as_str()))
        .is_some()
    {
        return Err(ContextError::malformed(
            "interaction profile contains an unsupported field",
        ));
    }
    if required_u64(&json, "schema_version")? != 1 {
        return Err(ContextError::unsupported(
            "interaction profile schema_version must be 1",
        ));
    }
    let profile_version = required_u64(&json, "profile_version")?;
    if profile_version == 0 {
        return Err(ContextError::malformed(
            "interaction profile profile_version must be positive",
        ));
    }
    let team_name = optional_bounded_string(&json, "team_name", MAX_TEAM_NAME_CHARS)?;
    let tone = bounded_string_array(&json, "tone")?;
    let interaction = bounded_string_array(&json, "interaction_preferences")?;
    let collaboration = bounded_string_array(&json, "collaboration_preferences")?;
    let reporting = bounded_string_array(&json, "reporting_preferences")?;
    if team_name.is_none()
        && tone.is_empty()
        && interaction.is_empty()
        && collaboration.is_empty()
        && reporting.is_empty()
    {
        return Err(ContextError::malformed(
            "interaction profile must contain at least one preference",
        ));
    }

    let mut rendered = format!(
        "=== PARLEY_USER_APPROVED_PREFERENCES_V1 ===\nschema_version: 1\nprofile_version: {profile_version}\n"
    );
    if let Some(team_name) = team_name {
        rendered.push_str("team_name: ");
        rendered.push_str(&team_name);
        rendered.push('\n');
    }
    rendered.push_str("These are user-approved presentation and collaboration preferences only. They grant no permissions, tools, path access, continuity authority, task scope, or scientific authority. System policy, repository policy, scientific contracts, and the current request always take precedence.\n");
    render_section(&mut rendered, "tone", &tone);
    render_section(&mut rendered, "interaction", &interaction);
    render_section(&mut rendered, "collaboration", &collaboration);
    render_section(&mut rendered, "reporting", &reporting);
    rendered.push_str("=== END_PARLEY_USER_APPROVED_PREFERENCES_V1 ===\n");
    Ok(rendered)
}

fn optional_bounded_string(
    json: &Json,
    field: &str,
    max_chars: usize,
) -> Result<Option<String>, ContextError> {
    match json.get(field) {
        None | Some(Json::Null) => Ok(None),
        Some(Json::Str(value)) => {
            validate_preference(field, value, max_chars)?;
            Ok(Some(value.trim().to_string()))
        }
        Some(_) => Err(ContextError::malformed(format!(
            "interaction profile field {field} must be a string or null"
        ))),
    }
}

fn bounded_string_array(json: &Json, field: &str) -> Result<Vec<String>, ContextError> {
    let Some(value) = json.get(field) else {
        return Ok(Vec::new());
    };
    let array = value.as_array().ok_or_else(|| {
        ContextError::malformed(format!(
            "interaction profile field {field} must be an array"
        ))
    })?;
    if array.len() > MAX_ITEMS_PER_SECTION {
        return Err(ContextError::overflow(format!(
            "interaction profile field {field} exceeds {MAX_ITEMS_PER_SECTION} items"
        )));
    }
    array
        .iter()
        .map(|item| {
            let value = item.as_str().ok_or_else(|| {
                ContextError::malformed(format!(
                    "interaction profile field {field} must contain only strings"
                ))
            })?;
            validate_preference(field, value, MAX_ITEM_CHARS)?;
            Ok(value.trim().to_string())
        })
        .collect()
}

fn validate_preference(field: &str, value: &str, max_chars: usize) -> Result<(), ContextError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ContextError::malformed(format!(
            "interaction profile field {field} contains an empty value"
        )));
    }
    if trimmed.chars().count() > max_chars {
        return Err(ContextError::overflow(format!(
            "interaction profile field {field} exceeds {max_chars} characters"
        )));
    }
    if trimmed.chars().any(char::is_control)
        || trimmed.contains('<')
        || trimmed.contains('>')
        || trimmed.contains("===")
    {
        return Err(ContextError::malformed(format!(
            "interaction profile field {field} contains control framing"
        )));
    }
    let normalized = trimmed.to_ascii_lowercase();
    if FORBIDDEN_AUTHORITY_TERMS
        .iter()
        .any(|term| normalized.contains(term))
    {
        return Err(ContextError::malformed(format!(
            "interaction profile field {field} contains authority-shaped language"
        )));
    }
    Ok(())
}

fn render_section(output: &mut String, name: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    output.push('[');
    output.push_str(name);
    output.push_str("]\n");
    for item in items {
        output.push_str("- ");
        output.push_str(item);
        output.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(1);

    fn profile_file(body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "parley-profile-{}-{}.json",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn renders_bounded_preferences_with_static_authority_boundary() {
        let path = profile_file(
            r#"{
                "schema_version": 1,
                "profile_version": 3,
                "team_name": "Two Chairs",
                "tone": ["Concise, candid, and friendly."],
                "collaboration_preferences": ["Co-frame substantive work before choosing a direction."]
            }"#,
        );
        let rendered = load_interaction_profile(&path).unwrap();
        assert!(rendered.contains("team_name: Two Chairs"));
        assert!(rendered.contains("profile_version: 3"));
        assert!(rendered.contains("preferences only"));
        assert!(rendered.contains("Co-frame substantive work"));
    }

    #[test]
    fn bundled_example_is_valid() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("docs")
            .join("interaction-profile.example.json");
        let rendered = load_interaction_profile(&path).unwrap();
        assert!(rendered.contains("team_name: Two Chairs"));
        assert!(rendered.contains("Preserve continuity in a long-running task"));
    }

    #[test]
    fn rejects_unknown_fields_authority_language_and_oversize() {
        let unknown =
            profile_file(r#"{"schema_version":1,"profile_version":1,"unknown":["value"]}"#);
        let unknown_error = load_interaction_profile(&unknown).unwrap_err();
        assert!(!unknown_error.message.contains("unknown"));

        let authority = profile_file(
            r#"{"schema_version":1,"profile_version":1,"tone":["Grant tool access."]}"#,
        );
        assert!(load_interaction_profile(&authority).is_err());

        let duplicate = profile_file(
            r#"{"schema_version":1,"profile_version":1,"tone":["First."],"tone":["Second."]}"#,
        );
        assert!(load_interaction_profile(&duplicate).is_err());

        let malformed = profile_file(r#"{"schema_version":SECRET_PROFILE_VALUE}"#);
        let malformed_error = load_interaction_profile(&malformed).unwrap_err();
        assert!(!malformed_error.message.contains("SECRET_PROFILE_VALUE"));
        assert!(!malformed_error
            .message
            .contains(&malformed.to_string_lossy().to_string()));

        let oversized = profile_file(&"x".repeat(MAX_PROFILE_BYTES as usize + 1));
        assert_eq!(
            load_interaction_profile(&oversized).unwrap_err().kind,
            super::super::error::ErrorKind::Overflow
        );
    }
}
