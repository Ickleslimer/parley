use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::command::HealthBinary;
use crate::fsutil::{self, FileIdentity};
use crate::schema::{HealthError, SCHEMA_VERSION};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ScopeFile {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_common_dir: Option<String>,
    #[serde(default)]
    pub main_root: String,
    #[serde(default)]
    pub worktree_roots: Vec<String>,
    #[serde(default)]
    pub executables: HealthExecutables,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HealthExecutables {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<ExecutableIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor: Option<ExecutableIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<ExecutableIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutableIdentity {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_serial: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_index: Option<u64>,
}

impl ExecutableIdentity {
    pub fn identity(&self) -> Option<FileIdentity> {
        match (self.volume_serial, self.file_index) {
            (Some(volume_serial), Some(file_index)) => Some(FileIdentity {
                volume_serial,
                file_index,
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScopeUpdate {
    pub git_common_dir: Option<String>,
    pub main_root: Option<String>,
    pub worktree_roots: Vec<String>,
    pub query_path: Option<String>,
    pub supervisor_path: Option<String>,
    pub hook_path: Option<String>,
}

impl ScopeFile {
    pub fn empty() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            git_common_dir: None,
            main_root: String::new(),
            worktree_roots: Vec::new(),
            executables: HealthExecutables::default(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, HealthError> {
        let bytes = fsutil::read_bounded(path, 256 * 1024)?;
        if bytes.clipped {
            return Err(HealthError::msg("scope.json exceeds bound"));
        }
        let parsed: Self = serde_json::from_slice(fsutil::strip_bom(&bytes.bytes))?;
        if parsed.schema_version != SCHEMA_VERSION {
            return Err(HealthError::msg("unsupported scope schema"));
        }
        Ok(parsed)
    }

    pub fn load_or_empty(path: &Path) -> Self {
        Self::load(path).unwrap_or_else(|_| Self::empty())
    }

    pub fn save(&self, path: &Path) -> Result<(), HealthError> {
        let bytes = serde_json::to_vec_pretty(self)?;
        fsutil::atomic_write(path, &bytes)
    }

    pub fn cached_roots(&self) -> Vec<String> {
        let mut roots = Vec::new();
        if !self.main_root.trim().is_empty() {
            roots.push(self.main_root.clone());
        }
        roots.extend(self.worktree_roots.iter().cloned());
        roots
    }

    pub fn contains_path(&self, candidate: &str) -> bool {
        self.cached_roots()
            .iter()
            .any(|root| canonical_contains(root, candidate))
    }

    pub fn in_scope(&self, cwd: Option<&str>, workspace_root: Option<&str>) -> bool {
        match (cwd, workspace_root) {
            (Some(cwd), Some(workspace)) => {
                self.contains_path(cwd) && self.contains_path(workspace)
            }
            (Some(cwd), None) => self.contains_path(cwd),
            (None, Some(workspace)) => self.contains_path(workspace),
            (None, None) => false,
        }
    }

    pub fn executable(&self, binary: HealthBinary) -> Option<&ExecutableIdentity> {
        match binary {
            HealthBinary::Query => self.executables.query.as_ref(),
            HealthBinary::Supervisor => self.executables.supervisor.as_ref(),
            HealthBinary::Hook => self.executables.hook.as_ref(),
        }
    }

    pub fn apply_update(&mut self, update: ScopeUpdate) {
        if let Some(common) = update.git_common_dir {
            self.git_common_dir = Some(normalize_windows_path(&common));
            let discovered = worktrees_from_common_dir(Path::new(&common));
            for root in discovered {
                push_unique(&mut self.worktree_roots, root);
            }
        }
        if let Some(main) = update.main_root {
            self.main_root = normalize_windows_path(&main);
        }
        for root in update.worktree_roots {
            push_unique(&mut self.worktree_roots, normalize_windows_path(&root));
        }
        if let Some(path) = update.query_path {
            self.executables.query = Some(identity_for_path(&path));
        }
        if let Some(path) = update.supervisor_path {
            self.executables.supervisor = Some(identity_for_path(&path));
        }
        if let Some(path) = update.hook_path {
            self.executables.hook = Some(identity_for_path(&path));
        }
        self.schema_version = SCHEMA_VERSION;
        self.refresh_identities();
    }

    pub fn refresh_identities(&mut self) {
        refresh_one(&mut self.executables.query);
        refresh_one(&mut self.executables.supervisor);
        refresh_one(&mut self.executables.hook);
    }
}

pub fn normalize_windows_path(path: &str) -> String {
    let mut text = path.trim().replace('/', "\\");
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        text = if let Some(unc) = rest.strip_prefix("UNC\\") {
            format!(r"\\{unc}")
        } else {
            rest.to_string()
        };
    }
    while text.len() > 3 && text.ends_with('\\') {
        text.pop();
    }
    text.to_ascii_lowercase()
}

pub fn canonical_contains(root: &str, candidate: &str) -> bool {
    let root = normalize_windows_path(root);
    let candidate = normalize_windows_path(candidate);
    if root.is_empty() || candidate.is_empty() {
        return false;
    }
    if candidate == root {
        return true;
    }
    let prefix = if root.ends_with('\\') {
        root
    } else {
        format!("{root}\\")
    };
    candidate.starts_with(&prefix)
}

pub fn paths_equivalent(left: &str, right: &str) -> bool {
    normalize_windows_path(left) == normalize_windows_path(right)
}

pub fn is_absolute_windows(path: &str) -> bool {
    let path = path.trim();
    let bytes = path.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'))
        || path.starts_with(r"\\")
}

pub fn resolve_candidate(token: &str, cwd: &str, path_dirs: &[String]) -> String {
    let token = token.trim().trim_matches('"');
    if is_absolute_windows(token) {
        return normalize_windows_path(token);
    }
    if token.contains('\\') || token.contains('/') {
        let joined = format!(
            "{}\\{}",
            cwd.trim_end_matches(['\\', '/']),
            token.trim_start_matches(['\\', '/'])
        );
        return normalize_windows_path(&joined);
    }
    for dir in path_dirs {
        let candidate = PathBuf::from(dir).join(token);
        if candidate.is_file() {
            return normalize_windows_path(&candidate.to_string_lossy());
        }
    }
    normalize_windows_path(token)
}

pub fn query_matches_installed(
    command_exe: &str,
    cwd: &str,
    path_dirs: &[String],
    installed: &ExecutableIdentity,
    observed_identity: Option<FileIdentity>,
) -> bool {
    if let (Some(observed), Some(wanted)) = (observed_identity, installed.identity()) {
        return observed == wanted;
    }
    let resolved = resolve_candidate(command_exe, cwd, path_dirs);
    paths_equivalent(&resolved, &installed.path)
        || paths_equivalent(command_exe, &installed.path)
}

pub fn worktrees_from_common_dir(common: &Path) -> Vec<String> {
    let mut roots = Vec::new();
    if common.file_name().and_then(|name| name.to_str()) == Some(".git") {
        if let Some(parent) = common.parent() {
            roots.push(normalize_windows_path(&parent.to_string_lossy()));
        }
    }
    let worktrees = common.join("worktrees");
    let entries = match fs::read_dir(&worktrees) {
        Ok(entries) => entries,
        Err(_) => return roots,
    };
    for entry in entries.flatten() {
        let gitdir = entry.path().join("gitdir");
        if let Ok(text) = fs::read_to_string(&gitdir) {
            let path = Path::new(text.trim());
            if let Some(root) = path.parent() {
                push_unique(&mut roots, normalize_windows_path(&root.to_string_lossy()));
            }
        }
    }
    roots
}

fn identity_for_path(path: &str) -> ExecutableIdentity {
    let normalized = normalize_windows_path(path);
    let (volume_serial, file_index) = fsutil::file_identity(Path::new(path))
        .ok()
        .map(|identity| (Some(identity.volume_serial), Some(identity.file_index)))
        .unwrap_or((None, None));
    ExecutableIdentity {
        path: normalized,
        volume_serial,
        file_index,
    }
}

fn refresh_one(slot: &mut Option<ExecutableIdentity>) {
    if let Some(current) = slot.as_mut() {
        if let Ok(identity) = fsutil::file_identity(Path::new(&current.path)) {
            current.volume_serial = Some(identity.volume_serial);
            current.file_index = Some(identity.file_index);
        }
    }
}

fn push_unique(roots: &mut Vec<String>, root: String) {
    if !root.is_empty() && !roots.iter().any(|existing| paths_equivalent(existing, &root)) {
        roots.push(root);
    }
}
