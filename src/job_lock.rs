//! Cross-process execution leases for locked Grok profiles.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::ask::AskRequest;
use crate::context::refuse_reparse_chain;
use crate::peer_session::ConversationMode;
use crate::profile_namespace;
use crate::signals::fnv1a_64;

pub(crate) struct ExecutionLease {
    _files: Vec<File>,
}

impl ExecutionLease {
    pub(crate) fn acquire_for_request(req: &AskRequest, owner: &str) -> Result<Self, String> {
        Self::acquire_request_and_worktrees(req, Vec::new(), owner)
    }

    pub(crate) fn acquire_profile(
        root: &Path,
        profile_namespace: &str,
        owner: &str,
    ) -> Result<Self, String> {
        acquire(
            root,
            vec![LockKey::new("profile", profile_namespace)],
            owner,
        )
    }

    pub(crate) fn acquire_for_job(
        req: &AskRequest,
        paths: impl IntoIterator<Item = PathBuf>,
        owner: &str,
    ) -> Result<Self, String> {
        let lease = Self::acquire_request_and_worktrees(req, paths, owner)?;
        if lease._files.is_empty() {
            return Err("asynchronous jobs require an active execution lease".to_string());
        }
        Ok(lease)
    }

    fn acquire_request_and_worktrees(
        req: &AskRequest,
        paths: impl IntoIterator<Item = PathBuf>,
        owner: &str,
    ) -> Result<Self, String> {
        let Some(root) = require_conversation_root(req.conversation_mode, configured_root()?)?
        else {
            return Ok(Self { _files: Vec::new() });
        };
        if req.harness != "grok" {
            return Ok(Self { _files: Vec::new() });
        }
        let profile_namespace = profile_namespace::from_env()?;
        let session = req
            .session_id
            .as_deref()
            .or(req.resume_id.as_deref())
            .ok_or_else(|| {
                "locked job execution requires an explicit target session".to_string()
            })?;
        let canonical_cwd = fs::canonicalize(&req.cwd).map_err(|error| {
            format!(
                "canonicalize execution worktree {}: {error}",
                req.cwd.display()
            )
        })?;
        let mut keys = vec![
            LockKey::new("profile", &profile_namespace),
            LockKey::new("session", session),
            LockKey::new("worktree", &path_key(&canonical_cwd)),
        ];
        for path in paths {
            let canonical = fs::canonicalize(&path).map_err(|error| {
                format!(
                    "canonicalize execution worktree {}: {error}",
                    path.display()
                )
            })?;
            keys.push(LockKey::new("worktree", &path_key(&canonical)));
        }
        acquire(&root, keys, owner)
    }
}

fn require_conversation_root(
    conversation_mode: Option<ConversationMode>,
    root: Option<PathBuf>,
) -> Result<Option<PathBuf>, String> {
    if conversation_mode == Some(ConversationMode::Canonical) && root.is_none() {
        return Err(
            "canonical conversation mode requires PARLEY_ASYNC_JOBS_ENABLED=true and PARLEY_JOB_STATE_DIR so the native session has a cross-process execution lease"
                .to_string(),
        );
    }
    Ok(root)
}

fn configured_root() -> Result<Option<PathBuf>, String> {
    let enabled = match env::var("PARLEY_ASYNC_JOBS_ENABLED") {
        Ok(value) => parse_bool("PARLEY_ASYNC_JOBS_ENABLED", &value)?,
        Err(env::VarError::NotPresent) => false,
        Err(error) => return Err(format!("read PARLEY_ASYNC_JOBS_ENABLED: {error}")),
    };
    if !enabled {
        return Ok(None);
    }
    let root = env::var_os("PARLEY_JOB_STATE_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "PARLEY_ASYNC_JOBS_ENABLED requires PARLEY_JOB_STATE_DIR".to_string())?;
    if !root.is_absolute() {
        return Err("PARLEY_JOB_STATE_DIR must be absolute".to_string());
    }
    Ok(Some(root))
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LockKey {
    kind: String,
    value: String,
}

impl LockKey {
    fn new(kind: &str, value: &str) -> Self {
        Self {
            kind: kind.to_string(),
            value: value.to_string(),
        }
    }

    fn file_name(&self) -> String {
        format!(
            "{}-{:016x}.lock",
            self.kind,
            fnv1a_64(&format!("{}\0{}", self.kind, self.value))
        )
    }
}

fn acquire(root: &Path, mut keys: Vec<LockKey>, owner: &str) -> Result<ExecutionLease, String> {
    keys.sort();
    keys.dedup();
    let lock_dir = root.join("locks");
    refuse_reparse_chain(root).map_err(|error| error.to_string())?;
    fs::create_dir_all(&lock_dir)
        .map_err(|error| format!("create job lock directory {}: {error}", lock_dir.display()))?;
    refuse_reparse_chain(&lock_dir).map_err(|error| error.to_string())?;

    let mut files = Vec::with_capacity(keys.len());
    for key in keys {
        let path = lock_dir.join(key.file_name());
        let mut file = open_exclusive(&path).map_err(|error| {
            format!(
                "{} lease is already held or unavailable ({}): {error}",
                key.kind,
                path.display()
            )
        })?;
        file.set_len(0)
            .and_then(|_| file.seek(SeekFrom::Start(0)))
            .and_then(|_| {
                writeln!(
                    file,
                    "schema_version=1\nkind={}\nowner={}\npid={}",
                    key.kind,
                    sanitize(owner),
                    std::process::id()
                )
            })
            .and_then(|_| file.flush())
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("persist execution lease {}: {error}", path.display()))?;
        files.push(file);
    }
    Ok(ExecutionLease { _files: files })
}

#[cfg(windows)]
fn open_exclusive(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .share_mode(0)
        .open(path)
}

#[cfg(not(windows))]
fn open_exclusive(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "locked async jobs require Windows exclusive file sharing",
    ))
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(format!("{name} must be true or false, got {value}")),
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase()
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        .take(96)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_conversations_require_a_configured_execution_lease() {
        assert!(require_conversation_root(Some(ConversationMode::Canonical), None).is_err());
        assert!(
            require_conversation_root(Some(ConversationMode::Isolated), None)
                .unwrap()
                .is_none()
        );
        assert!(require_conversation_root(None, None).unwrap().is_none());

        let root = PathBuf::from(r"C:\parley-jobs");
        assert_eq!(
            require_conversation_root(Some(ConversationMode::Canonical), Some(root.clone()))
                .unwrap(),
            Some(root)
        );
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = env::temp_dir().join(format!(
            "parley-job-lock-{name}-{}-{}",
            std::process::id(),
            fnv1a_64(name)
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    #[cfg(windows)]
    fn exact_key_is_exclusive_and_reusable_after_drop() {
        let root = temp_root("exclusive");
        let key = LockKey::new("session", "session-a");
        let first = acquire(&root, vec![key.clone()], "first").unwrap();
        assert!(acquire(&root, vec![key.clone()], "second").is_err());
        drop(first);
        assert!(acquire(&root, vec![key], "third").is_ok());
    }

    #[test]
    #[cfg(windows)]
    fn distinct_keys_do_not_contend() {
        let root = temp_root("distinct");
        let _first = acquire(&root, vec![LockKey::new("session", "a")], "first").unwrap();
        let second = acquire(&root, vec![LockKey::new("session", "b")], "second");
        assert!(second.is_ok());
    }

    #[test]
    #[cfg(windows)]
    fn partial_acquisition_releases_profile_and_session_on_worktree_contention() {
        let root = temp_root("partial-acquisition");
        let profile = LockKey::new("profile", "owner");
        let session = LockKey::new("session", "session");
        let worktree = LockKey::new("worktree", "shared");
        let _blocker = acquire(&root, vec![worktree.clone()], "blocker").unwrap();
        assert!(acquire(
            &root,
            vec![profile.clone(), session.clone(), worktree],
            "contender",
        )
        .is_err());
        assert!(acquire(&root, vec![profile, session], "next-owner").is_ok());
    }
}
