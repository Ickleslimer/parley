//! Stable logical peer sessions for long-running Codex-to-Grok collaboration.

use crate::context::{ContextKey, CursorState, JournalStore, StateDirEnv};
use crate::profile_namespace;
use crate::sha256::sha256;
use std::path::Path;

const CANONICAL_SESSION_DOMAIN: &str = "parley-canonical-peer-binding-v2";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ConversationMode {
    #[default]
    Isolated,
    Canonical,
}

impl ConversationMode {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "isolated" => Ok(Self::Isolated),
            "canonical" => Ok(Self::Canonical),
            _ => Err(format!(
                "conversation mode must be isolated or canonical, got {value}"
            )),
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Isolated => "isolated",
            Self::Canonical => "canonical",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CanonicalAction {
    New,
    Resume,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CanonicalTarget {
    pub(crate) session_id: String,
    pub(crate) action: CanonicalAction,
}

pub(crate) fn resolve_canonical_target(
    source_session: &str,
    epoch: u64,
    cwd: &Path,
) -> Result<CanonicalTarget, String> {
    let namespace = profile_namespace::from_env()?;
    resolve_canonical_target_in(
        &namespace,
        source_session,
        epoch,
        cwd,
        &StateDirEnv::from_process(),
    )
}

fn resolve_canonical_target_in(
    namespace: &str,
    source_session: &str,
    epoch: u64,
    cwd: &Path,
    state_env: &StateDirEnv,
) -> Result<CanonicalTarget, String> {
    let session_id = canonical_session_id(namespace, source_session, epoch, cwd);
    let key =
        ContextKey::new(&session_id, "codex", source_session).map_err(|error| error.to_string())?;
    let store = JournalStore::open(state_env).map_err(|error| error.to_string())?;
    let state = store
        .cursor_state(&key)
        .map_err(|error| error.to_string())?;
    let action = action_for_state(state)?;
    Ok(CanonicalTarget { session_id, action })
}

fn action_for_state(state: CursorState) -> Result<CanonicalAction, String> {
    match state {
        CursorState::Absent => Ok(CanonicalAction::New),
        CursorState::Committed | CursorState::Uncertain => Ok(CanonicalAction::Resume),
        CursorState::InFlight => {
            Err("canonical peer session already has live in-flight context state".to_string())
        }
    }
}

fn canonical_session_id(namespace: &str, source_session: &str, epoch: u64, cwd: &Path) -> String {
    let worktree = canonical_worktree_key(cwd);
    let material =
        format!("{CANONICAL_SESSION_DOMAIN}\0{namespace}\0{source_session}\0{epoch}\0{worktree}");
    let digest = sha256(material.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn canonical_worktree_key(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn canonical_id_is_stable_uuid_v8() {
        let first = canonical_session_id(
            "profile-a",
            "codex-session",
            0,
            Path::new(r"C:\approved\worker"),
        );
        let second = canonical_session_id(
            "profile-a",
            "codex-session",
            0,
            Path::new(r"c:/APPROVED/worker/"),
        );
        assert_eq!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "8");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
        assert_eq!(first.as_bytes()[8], b'-');
        assert_eq!(first.as_bytes()[13], b'-');
        assert_eq!(first.as_bytes()[18], b'-');
        assert_eq!(first.as_bytes()[23], b'-');
    }

    #[test]
    fn canonical_id_separates_profile_source_epoch_and_worktree() {
        let worktree = Path::new(r"C:\approved\worker-a");
        let baseline = canonical_session_id("profile-a", "codex-session", 0, worktree);
        assert_ne!(
            baseline,
            canonical_session_id("profile-b", "codex-session", 0, worktree)
        );
        assert_ne!(
            baseline,
            canonical_session_id("profile-a", "other-session", 0, worktree)
        );
        assert_ne!(
            baseline,
            canonical_session_id("profile-a", "codex-session", 1, worktree)
        );
        assert_ne!(
            baseline,
            canonical_session_id(
                "profile-a",
                "codex-session",
                0,
                Path::new(r"C:\approved\worker-b"),
            )
        );
    }

    #[test]
    fn cursor_state_selects_new_resume_or_fail_closed() {
        assert_eq!(
            action_for_state(CursorState::Absent).unwrap(),
            CanonicalAction::New
        );
        assert_eq!(
            action_for_state(CursorState::Committed).unwrap(),
            CanonicalAction::Resume
        );
        assert_eq!(
            action_for_state(CursorState::Uncertain).unwrap(),
            CanonicalAction::Resume
        );
        assert!(action_for_state(CursorState::InFlight).is_err());
    }

    #[test]
    fn absent_canonical_lookup_is_read_only() {
        let root = std::env::temp_dir().join(format!(
            "parley-peer-session-{}-{}",
            std::process::id(),
            TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let target = resolve_canonical_target_in(
            "profile-test",
            "source-session",
            0,
            Path::new(r"C:\approved\worker"),
            &StateDirEnv {
                parley_context_state_dir: Some(root.clone()),
                localappdata: None,
            },
        )
        .unwrap();
        assert_eq!(target.action, CanonicalAction::New);
        assert!(!root.exists());
    }

    #[test]
    fn conversation_mode_is_normalized_and_validated() {
        assert_eq!(
            ConversationMode::parse(" CANONICAL ").unwrap(),
            ConversationMode::Canonical
        );
        assert_eq!(
            ConversationMode::parse("isolated").unwrap(),
            ConversationMode::Isolated
        );
        assert!(ConversationMode::parse("shared").is_err());
    }
}
