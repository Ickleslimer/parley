//! Grok sessions: `~/.grok/sessions/<encoded-cwd>/<session-id>/`.
//! Directory encoding is deliberately ignored; `summary.json` is authoritative.

use std::env;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use super::{canonical, file_mtime_ms, home_dir, SessionRef, SessionStore, Turn};
use crate::harness::Invocation;
use crate::json::Json;

#[derive(Default)]
pub(crate) struct GrokSessions {
    root_override: Option<PathBuf>,
}

impl SessionStore for GrokSessions {
    fn harness(&self) -> &'static str {
        "grok"
    }

    fn list(&self, cwd: &Path) -> Result<Vec<SessionRef>, String> {
        let root = match self.root() {
            Some(root) if root.is_dir() => root,
            _ => return Ok(Vec::new()),
        };
        let mut summaries = Vec::new();
        collect_named(&root, "summary.json", &mut summaries);
        let mut sessions = summaries
            .iter()
            .filter_map(|path| read_session(path, cwd))
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_ms));
        Ok(sessions)
    }

    fn resume_invocation(&self, id: &str, cwd: &Path, yolo: bool) -> Result<Invocation, String> {
        let mut args = vec![
            "--no-auto-update".to_string(),
            "--cwd".to_string(),
            cwd.to_string_lossy().to_string(),
        ];
        if id.is_empty() {
            args.push("--continue".to_string());
        } else {
            args.extend(["--resume".to_string(), id.to_string()]);
        }
        if yolo {
            args.push("--always-approve".to_string());
        }
        Ok(Invocation::new("grok", args))
    }

    fn transcript(&self, id: &str, cwd: &Path) -> Result<Vec<Turn>, String> {
        let summary = self
            .find_summary(id, cwd)
            .ok_or_else(|| format!("grok session {id} not found for {}", cwd.display()))?;
        let chat = summary
            .parent()
            .ok_or_else(|| format!("invalid Grok session path: {}", summary.display()))?
            .join("chat_history.jsonl");
        let file =
            File::open(&chat).map_err(|error| format!("open {}: {error}", chat.display()))?;
        let mut turns = Vec::new();
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let json = match Json::parse(line.trim()) {
                Ok(json) => json,
                Err(_) => continue,
            };
            if json.get("synthetic_reason").is_some() {
                continue;
            }
            let role = match json.get("type").and_then(Json::as_str) {
                Some(role @ ("user" | "assistant")) => role,
                _ => continue,
            };
            if let Some(text) = content_text(&json) {
                let text = text.trim();
                if !text.is_empty() {
                    turns.push(Turn {
                        role: role.to_string(),
                        text: text.to_string(),
                    });
                }
            }
        }
        Ok(turns)
    }
}

impl GrokSessions {
    pub(crate) fn validate_image_session(&self, id: &str, cwd: &Path) -> Result<(), String> {
        if id.len() != 36
            || !id.bytes().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
        {
            return Err("image_session_id_invalid".to_string());
        }
        let requested =
            fs::canonicalize(cwd).map_err(|_| "image_session_cwd_unavailable".to_string())?;
        let root = self.root().ok_or("image_session_store_unavailable")?;
        let root =
            fs::canonicalize(root).map_err(|_| "image_session_store_unavailable".to_string())?;
        let entries =
            fs::read_dir(&root).map_err(|_| "image_session_store_unavailable".to_string())?;
        let mut summaries = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|_| "image_session_store_unavailable".to_string())?;
            if !entry
                .file_type()
                .map_err(|_| "image_session_store_unavailable".to_string())?
                .is_dir()
            {
                continue;
            }
            let directory = entry.path().join(id);
            if directory
                .try_exists()
                .map_err(|_| "image_session_summary_unavailable".to_string())?
            {
                crate::context::refuse_reparse_chain(&directory)
                    .map_err(|_| "image_session_summary_unsafe".to_string())?;
                summaries.push(directory.join("summary.json"));
            }
        }
        let summary = match summaries.as_slice() {
            [] => return Err("image_session_not_found".to_string()),
            [summary] => summary,
            _ => return Err("image_session_ambiguous".to_string()),
        };
        crate::context::refuse_reparse_chain(summary)
            .map_err(|_| "image_session_summary_unsafe".to_string())?;
        let metadata =
            fs::metadata(summary).map_err(|_| "image_session_summary_unavailable".to_string())?;
        if !metadata.is_file() || metadata.len() > 256 * 1024 {
            return Err("image_session_summary_invalid".to_string());
        }
        let raw = fs::read_to_string(summary)
            .map_err(|_| "image_session_summary_unavailable".to_string())?;
        let record = Json::parse(&raw).map_err(|_| "image_session_summary_invalid".to_string())?;
        if summary_id(&record) != Some(id) {
            return Err("image_session_summary_identity_mismatch".to_string());
        }
        let stored = record
            .get("info")
            .and_then(|info| info.get("cwd"))
            .and_then(Json::as_str)
            .ok_or("image_session_summary_invalid")?;
        let stored = fs::canonicalize(stored)
            .map_err(|_| "image_session_stored_cwd_unavailable".to_string())?;
        if !paths_match(&stored, &requested) {
            return Err("image_session_cwd_mismatch".to_string());
        }
        Ok(())
    }

    fn root(&self) -> Option<PathBuf> {
        self.root_override.clone().or_else(|| {
            let home = env::var_os("GROK_HOME").map(PathBuf::from).or_else(|| {
                if cfg!(windows) {
                    env::var_os("USERPROFILE")
                        .map(PathBuf::from)
                        .or_else(home_dir)
                } else {
                    home_dir()
                }
                .map(|home| home.join(".grok"))
            });
            home.map(|home| home.join("sessions"))
        })
    }

    fn find_summary(&self, id: &str, cwd: &Path) -> Option<PathBuf> {
        let root = self.root()?;
        let mut summaries = Vec::new();
        collect_named(&root, "summary.json", &mut summaries);
        summaries.into_iter().find(|path| {
            let json = fs::read_to_string(path)
                .ok()
                .and_then(|raw| Json::parse(&raw).ok());
            let Some(json) = json else { return false };
            summary_id(&json) == Some(id) && summary_cwd_matches(&json, cwd)
        })
    }

    #[cfg(test)]
    fn at(root: PathBuf) -> Self {
        Self {
            root_override: Some(root),
        }
    }
}

fn collect_named(dir: &Path, name: &str, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_named(&path, name, out);
        } else if path.file_name().and_then(|value| value.to_str()) == Some(name) {
            out.push(path);
        }
    }
}

fn read_session(path: &Path, cwd: &Path) -> Option<SessionRef> {
    let json = Json::parse(&fs::read_to_string(path).ok()?).ok()?;
    if !summary_cwd_matches(&json, cwd) {
        return None;
    }
    let id = summary_id(&json)?.to_string();
    let session_cwd = json.get("info")?.get("cwd")?.as_str()?.to_string();
    let chat = path.parent()?.join("chat_history.jsonl");
    let summary = json
        .get("session_summary")
        .and_then(Json::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let first_user = first_user_prompt(&chat);
    let count = json
        .get("num_chat_messages")
        .or_else(|| json.get("num_messages"))
        .and_then(Json::as_number)
        .map(|value| value as usize);
    Some(SessionRef {
        harness: "grok".to_string(),
        id,
        cwd: session_cwd,
        updated_ms: file_mtime_ms(&chat).or_else(|| file_mtime_ms(path)),
        title: summary
            .or(first_user)
            .unwrap_or_else(|| "(grok session)".to_string()),
        message_count: count,
        delegated: false,
    })
}

fn summary_id(json: &Json) -> Option<&str> {
    json.get("info")?.get("id")?.as_str()
}

fn summary_cwd_matches(json: &Json, cwd: &Path) -> bool {
    let Some(stored) = json
        .get("info")
        .and_then(|info| info.get("cwd"))
        .and_then(Json::as_str)
    else {
        return false;
    };
    paths_match(Path::new(stored), cwd)
}

fn paths_match(left: &Path, right: &Path) -> bool {
    let left = canonical(left);
    let right = canonical(right);
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn first_user_prompt(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let json = match Json::parse(line.trim()) {
            Ok(json) => json,
            Err(_) => continue,
        };
        if json.get("synthetic_reason").is_some() {
            continue;
        }
        if json.get("type").and_then(Json::as_str) == Some("user") {
            if let Some(text) = content_text(&json) {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}

fn content_text(json: &Json) -> Option<String> {
    let content = json.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let mut parts = Vec::new();
    for block in content.as_array()? {
        if block.get("type").and_then(Json::as_str) == Some("text") {
            if let Some(text) = block.get("text").and_then(Json::as_str) {
                parts.push(text.to_string());
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn fixture() -> (PathBuf, PathBuf, String) {
        let root = env::temp_dir().join(format!(
            "parley-grok-session-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let cwd = root.join("worker");
        let id = "01a06582-d66e-7811-b0c9-0b0266e17903".to_string();
        let session = root.join("sessions").join("encoded").join(&id);
        fs::create_dir_all(&cwd).unwrap();
        fs::create_dir_all(&session).unwrap();
        fs::write(
            session.join("summary.json"),
            format!(
                "{{\"info\":{{\"id\":\"{id}\",\"cwd\":\"{}\"}},\"session_summary\":\"fixture\",\"num_chat_messages\":2}}",
                cwd.to_string_lossy().replace('\\', "\\\\")
            ),
        )
        .unwrap();
        fs::write(
            session.join("chat_history.jsonl"),
            "{\"type\":\"system\",\"content\":\"hidden\"}\n{\"type\":\"user\",\"content\":\"synthetic\",\"synthetic_reason\":\"skills\"}\n{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}\n{bad json}\n{\"type\":\"assistant\",\"content\":\"world\",\"tool_calls\":[]}\n{\"type\":\"tool_result\",\"content\":\"ignored\"}\n",
        )
        .unwrap();
        (root, cwd, id)
    }

    #[test]
    fn lists_and_reads_matching_sessions() {
        let (root, cwd, id) = fixture();
        let store = GrokSessions::at(root.join("sessions"));
        let sessions = store.list(&cwd).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, id);
        assert_eq!(sessions[0].title, "fixture");
        let turns = store.transcript(&id, &cwd).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "hello");
        assert_eq!(turns[1].text, "world");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn image_target_validates_id_canonical_cwd_and_distinct_failure_classes() {
        let (root, cwd, id) = fixture();
        let store = GrokSessions::at(root.join("sessions"));
        assert!(store.validate_image_session(&id, &cwd.join(".")).is_ok());
        let other = root.join("other");
        fs::create_dir(&other).unwrap();
        assert_eq!(
            store.validate_image_session(&id, &other).unwrap_err(),
            "image_session_cwd_mismatch"
        );
        assert_eq!(
            store
                .validate_image_session(&id, &root.join("missing"))
                .unwrap_err(),
            "image_session_cwd_unavailable"
        );
        assert_eq!(
            store
                .validate_image_session("../outside", &cwd)
                .unwrap_err(),
            "image_session_id_invalid"
        );
        assert_eq!(
            store
                .validate_image_session("00000000-0000-0000-0000-000000000000", &cwd)
                .unwrap_err(),
            "image_session_not_found"
        );
        let summary = root.join("sessions/encoded").join(&id).join("summary.json");
        fs::write(&summary, "{").unwrap();
        assert_eq!(
            store.validate_image_session(&id, &cwd).unwrap_err(),
            "image_session_summary_invalid"
        );
        fs::remove_file(&summary).unwrap();
        assert_eq!(
            store.validate_image_session(&id, &cwd).unwrap_err(),
            "image_session_summary_unavailable"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_native_image_session_identity_is_ambiguous() {
        let (root, cwd, id) = fixture();
        let duplicate = root.join("sessions/second").join(&id);
        fs::create_dir_all(&duplicate).unwrap();
        fs::copy(
            root.join("sessions/encoded").join(&id).join("summary.json"),
            duplicate.join("summary.json"),
        )
        .unwrap();
        assert_eq!(
            GrokSessions::at(root.join("sessions"))
                .validate_image_session(&id, &cwd)
                .unwrap_err(),
            "image_session_ambiguous"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn first_user_prompt_skips_malformed_records() {
        let path = env::temp_dir().join(format!(
            "parley-grok-history-{}-{}.jsonl",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            "{bad json}\n{\"type\":\"reasoning\",\"content\":\"hidden\"}\n{\"type\":\"user\",\"content\":\"synthetic\",\"synthetic_reason\":\"skills\"}\n{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"visible\"}]}\n",
        )
        .unwrap();
        assert_eq!(first_user_prompt(&path).as_deref(), Some("visible"));
        fs::remove_file(path).unwrap();
    }
}
