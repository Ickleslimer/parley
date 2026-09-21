//! Codex-only sessions-root and explicit rollout resolution.

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};

use super::error::ContextError;
use super::parse::{get_str, parse_json};
use super::reader::{read_first_complete_record, PhysicalRecord};
use super::winfile::{canonical_path, open_shared_read, source_identity, SourceIdentity};

const SOURCE_HARNESS: &str = "codex";

#[derive(Clone, Debug, Default)]
pub(crate) struct CodexHomeEnv {
    pub parley_codex_home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub userprofile: Option<PathBuf>,
}

impl CodexHomeEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            parley_codex_home: env_path("PARLEY_CODEX_HOME"),
            codex_home: env_path("CODEX_HOME"),
            home: env_path("HOME"),
            userprofile: env_path("USERPROFILE"),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedSource {
    pub session_id: String,
    pub path: PathBuf,
    pub identity: SourceIdentity,
}

pub(crate) fn resolve_codex_sessions_root(env: &CodexHomeEnv) -> Result<PathBuf, ContextError> {
    let codex_root = if let Some(path) = nonempty(env.parley_codex_home.as_ref()) {
        path.clone()
    } else if let Some(path) = nonempty(env.codex_home.as_ref()) {
        path.clone()
    } else if let Some(home) = nonempty(env.home.as_ref()) {
        home.join(".codex")
    } else if let Some(userprofile) = nonempty(env.userprofile.as_ref()) {
        userprofile.join(".codex")
    } else {
        return Err(ContextError::not_found(
            "cannot resolve Codex home (PARLEY_CODEX_HOME, CODEX_HOME, HOME, USERPROFILE)",
        ));
    };
    let sessions = codex_root.join("sessions");
    if !sessions.is_dir() {
        return Err(ContextError::not_found(format!(
            "Codex sessions root does not exist: {}",
            sessions.display()
        )));
    }
    Ok(canonical_path(&sessions))
}

pub(crate) fn resolve_codex_rollout(
    sessions_root: &Path,
    session_id: &str,
) -> Result<ResolvedSource, ContextError> {
    if session_id.is_empty() {
        return Err(ContextError::malformed(
            "Codex source session id must be explicit",
        ));
    }
    let root = canonical_path(sessions_root);
    let rollouts = collect_unique_rollouts(&root)?;
    let mut matches = Vec::new();
    for path in rollouts {
        let likely_match = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains(session_id));
        match leading_session_id(&path) {
            Ok(Some(id)) if id == session_id => matches.push(path),
            Ok(_) => {}
            Err(error) if likely_match => return Err(error),
            Err(_) => {}
        }
    }
    match matches.len() {
        0 => Err(ContextError::not_found(format!(
            "codex session {session_id} not found"
        ))),
        1 => {
            let path = matches.pop().unwrap();
            let file = open_shared_read(&path)?;
            let identity = source_identity(&file, &path)?;
            Ok(ResolvedSource {
                session_id: session_id.to_string(),
                path,
                identity,
            })
        }
        _ => Err(ContextError::new(
            super::error::ErrorKind::Duplicate,
            format!("duplicate Codex rollouts for session {session_id}"),
        )),
    }
}

pub(crate) fn source_harness() -> &'static str {
    SOURCE_HARNESS
}

fn collect_unique_rollouts(root: &Path) -> Result<Vec<PathBuf>, ContextError> {
    let mut out = Vec::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    let mut seen_files: HashSet<(u32, u64)> = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(root.to_path_buf());
    while let Some(dir) = queue.pop_front() {
        let canon = canonical_path(&dir);
        if !seen_dirs.insert(canon.to_string_lossy().into_owned()) {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match std::fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if meta.file_type().is_symlink() {
                if path.is_dir() {
                    queue.push_back(path);
                } else if is_rollout_name(&path) {
                    push_unique_rollout(path, &mut out, &mut seen_files);
                }
                continue;
            }
            if meta.is_dir() {
                queue.push_back(path);
            } else if is_rollout_name(&path) {
                push_unique_rollout(path, &mut out, &mut seen_files);
            }
        }
    }
    Ok(out)
}

fn push_unique_rollout(
    path: PathBuf,
    out: &mut Vec<PathBuf>,
    seen_files: &mut HashSet<(u32, u64)>,
) {
    if let Ok(file) = open_shared_read(&path) {
        if let Ok(identity) = source_identity(&file, &path) {
            if !seen_files.insert((identity.volume_serial, identity.file_index)) {
                return;
            }
        }
    }
    out.push(path);
}

fn is_rollout_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(".jsonl"))
}

fn leading_session_id(path: &Path) -> Result<Option<String>, ContextError> {
    let Some(record) = read_first_complete_record(path)? else {
        return Ok(None);
    };
    Ok(session_id_from_meta(&record))
}

fn session_id_from_meta(record: &PhysicalRecord) -> Option<String> {
    let json = parse_json(&record.text).ok()?;
    if get_str(&json, "type") != Some("session_meta") {
        return None;
    }
    json.get("payload")
        .and_then(|payload| get_str(payload, "id"))
        .map(str::to_string)
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

fn nonempty(path: Option<&PathBuf>) -> Option<&PathBuf> {
    path.filter(|value| !value.as_os_str().is_empty())
}
