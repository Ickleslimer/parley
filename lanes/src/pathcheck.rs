use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Denial, LaneError};
use crate::fsutil::{self, metadata_is_reparse};
use crate::schema::{Access, FileIdentity, GrantKind, PathGrant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalPath {
    pub drive: Option<String>,
    pub parts: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ObservedDir {
    pub canonical: PathBuf,
    pub identity: FileIdentity,
    pub normal: NormalPath,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedPath {
    pub full: PathBuf,
    pub relative: Vec<String>,
    pub ancestor_rel_len: usize,
}

pub fn observe_dir(path: &Path) -> Result<ObservedDir, LaneError> {
    if !path.is_absolute() {
        return Err(LaneError::new(Denial::Cwd, "directory must be absolute"));
    }
    fsutil::refuse_reparse_chain(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            LaneError::new(Denial::StaleGrant, "directory is missing")
        } else {
            LaneError::new(Denial::Cwd, format!("inspect directory: {error}"))
        }
    })?;
    if metadata_is_reparse(&metadata) {
        return Err(LaneError::new(Denial::Path, "directory is a reparse point"));
    }
    if !metadata.is_dir() {
        return Err(LaneError::new(Denial::Cwd, "path is not a directory"));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| LaneError::new(Denial::Cwd, format!("canonicalize directory: {error}")))?;
    fsutil::refuse_reparse_chain(&canonical)?;
    let identity = fsutil::file_identity(&canonical)?;
    let normal = normal_path(&canonical)?;
    Ok(ObservedDir {
        canonical,
        identity,
        normal,
    })
}

pub fn same_dir(left: &NormalPath, right: &NormalPath) -> bool {
    left.drive == right.drive && left.parts == right.parts
}

pub fn resolve_operation(cwd: &ObservedDir, requested: &Path) -> Result<ResolvedPath, LaneError> {
    if requested.as_os_str().is_empty() {
        return Err(LaneError::new(Denial::Path, "path is empty"));
    }
    let raw = requested.to_string_lossy();
    if has_glob(&raw) {
        return Err(LaneError::new(Denial::Path, "path contains a glob"));
    }
    let full = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        cwd.canonical.join(requested)
    };
    let lexical = normal_path(&full)?;
    fsutil::refuse_reparse_chain(&full)?;
    let ancestor = deepest_existing(&full)?;
    let ancestor_canonical = fs::canonicalize(&ancestor)
        .map_err(|error| LaneError::new(Denial::Path, format!("canonicalize ancestor: {error}")))?;
    fsutil::refuse_reparse_chain(&ancestor_canonical)?;
    let ancestor_normal = normal_path(&ancestor_canonical)?;
    if lexical.drive != ancestor_normal.drive
        || ancestor_normal.parts.len() > lexical.parts.len()
        || lexical.parts[..ancestor_normal.parts.len()] != ancestor_normal.parts
    {
        return Err(LaneError::new(Denial::Path, "path identity is ambiguous"));
    }
    if !within(&lexical, &cwd.normal) {
        return Err(LaneError::new(Denial::Path, "path escapes the worktree"));
    }
    if !within(&ancestor_normal, &cwd.normal) {
        return Err(LaneError::new(
            Denial::Path,
            "existing ancestor escapes the worktree",
        ));
    }
    let suffix_len = lexical.parts.len() - ancestor_normal.parts.len();
    if suffix_len > 0 {
        let metadata = fs::symlink_metadata(&ancestor)
            .map_err(|error| LaneError::new(Denial::Path, format!("inspect ancestor: {error}")))?;
        if metadata_is_reparse(&metadata) || !metadata.is_dir() {
            return Err(LaneError::new(
                Denial::Path,
                "new path descends through a file or reparse point",
            ));
        }
    }
    let relative = lexical.parts[cwd.normal.parts.len()..].to_vec();
    if relative.is_empty() {
        return Err(LaneError::new(Denial::Path, "path is the worktree root"));
    }
    Ok(ResolvedPath {
        full,
        relative,
        ancestor_rel_len: ancestor_normal.parts.len() - cwd.normal.parts.len(),
    })
}

pub fn reject_hardlinked_write(resolved: &ResolvedPath) -> Result<(), LaneError> {
    match fs::symlink_metadata(&resolved.full) {
        Ok(metadata) if metadata.is_file() && fsutil::file_link_count(&resolved.full)? > 1 => Err(
            LaneError::new(Denial::Path, "write target has multiple hard links"),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(LaneError::new(
            Denial::Path,
            format!("inspect write target: {error}"),
        )),
    }
}

pub fn normalize_grant(cwd: &ObservedDir, grant: &PathGrant) -> Result<PathGrant, LaneError> {
    let parts = normalize_relative(&grant.path)?;
    let mut synthetic = PathBuf::new();
    for part in &parts {
        synthetic.push(part);
    }
    let resolved = resolve_operation(cwd, &synthetic)?;
    if resolved.relative != parts {
        return Err(LaneError::new(
            Denial::Path,
            "grant path does not stay inside the worktree",
        ));
    }
    match grant.kind {
        GrantKind::Tree => {
            if resolved.ancestor_rel_len < parts.len() {
                return Err(LaneError::new(
                    Denial::Path,
                    "tree grant does not exist inside the worktree",
                ));
            }
            let target = join_relative(&cwd.canonical, &parts);
            let metadata = fs::symlink_metadata(&target).map_err(|error| {
                LaneError::new(Denial::Path, format!("inspect tree grant: {error}"))
            })?;
            if metadata_is_reparse(&metadata) || !metadata.is_dir() {
                return Err(LaneError::new(
                    Denial::Path,
                    "tree grant is not a real directory",
                ));
            }
        }
        GrantKind::File => {
            if resolved.ancestor_rel_len + 1 < parts.len() {
                return Err(LaneError::new(
                    Denial::Path,
                    "file grant ancestor is ambiguous or missing",
                ));
            }
            let target = join_relative(&cwd.canonical, &parts);
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata_is_reparse(&metadata) || !metadata.is_file() => {
                    return Err(LaneError::new(
                        Denial::Path,
                        "file grant is not a real file",
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(LaneError::new(
                        Denial::Path,
                        format!("inspect file grant: {error}"),
                    ));
                }
            }
        }
    }
    Ok(PathGrant {
        access: grant.access,
        kind: grant.kind,
        path: parts.join("/"),
    })
}

pub fn grants_overlap(left: &PathGrant, right: &PathGrant) -> bool {
    let Ok(left_parts) = normalize_relative(&left.path) else {
        return true;
    };
    let Ok(right_parts) = normalize_relative(&right.path) else {
        return true;
    };
    (left.kind == GrantKind::Tree && right_parts.starts_with(&left_parts))
        || (right.kind == GrantKind::Tree && left_parts.starts_with(&right_parts))
        || left_parts == right_parts
}

pub fn grant_covers(grant: &PathGrant, needed: Access, resolved: &ResolvedPath) -> bool {
    if needed == Access::Write && grant.access != Access::Write {
        return false;
    }
    let Ok(parts) = normalize_relative(&grant.path) else {
        return false;
    };
    match grant.kind {
        GrantKind::Tree => {
            resolved.relative.starts_with(&parts) && resolved.ancestor_rel_len >= parts.len()
        }
        GrantKind::File => {
            resolved.relative == parts && resolved.ancestor_rel_len + 1 >= parts.len()
        }
    }
}

pub fn normal_path(path: &Path) -> Result<NormalPath, LaneError> {
    let mut drive = None;
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                drive = Some(normalize_drive(prefix.as_os_str())?);
            }
            Component::RootDir => {}
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                reject_component(&text)?;
                parts.push(casefold(&text));
            }
            Component::CurDir | Component::ParentDir => {
                return Err(LaneError::new(
                    Denial::Path,
                    "path contains a traversal component",
                ));
            }
        }
    }
    if path.is_absolute() && drive.is_none() {
        return Err(LaneError::new(
            Denial::Path,
            "absolute path has no local drive",
        ));
    }
    Ok(NormalPath { drive, parts })
}

fn within(path: &NormalPath, root: &NormalPath) -> bool {
    path.drive == root.drive
        && path.parts.len() >= root.parts.len()
        && path.parts[..root.parts.len()] == root.parts
}

fn normalize_relative(path: &str) -> Result<Vec<String>, LaneError> {
    if path.is_empty() || has_glob(path) {
        return Err(LaneError::new(
            Denial::Path,
            "grant path is empty or a glob",
        ));
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(LaneError::new(Denial::Path, "grant path must be relative"));
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_string_lossy();
                reject_component(&text)?;
                let folded = casefold(&text);
                if folded == ".git" {
                    return Err(LaneError::new(
                        Denial::Path,
                        "git administrative paths are not grantable",
                    ));
                }
                parts.push(folded);
            }
            _ => {
                return Err(LaneError::new(
                    Denial::Path,
                    "grant path contains a traversal or prefix component",
                ));
            }
        }
    }
    if parts.is_empty() {
        return Err(LaneError::new(Denial::Path, "grant path is empty"));
    }
    Ok(parts)
}

fn join_relative(root: &Path, parts: &[String]) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
    }
    path
}

fn deepest_existing(path: &Path) -> Result<PathBuf, LaneError> {
    let mut current = path.to_path_buf();
    loop {
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata_is_reparse(&metadata) => {
                return Err(LaneError::new(
                    Denial::Path,
                    "path traverses a reparse point",
                ));
            }
            Ok(_) => return Ok(current),
            Err(error) if error.kind() == io::ErrorKind::NotFound => match current.parent() {
                Some(parent) if parent != current.as_path() => current = parent.to_path_buf(),
                _ => {
                    return Err(LaneError::new(
                        Denial::Path,
                        "path has no existing ancestor",
                    ))
                }
            },
            Err(error) => {
                return Err(LaneError::new(
                    Denial::Path,
                    format!("inspect path: {error}"),
                ))
            }
        }
    }
}

fn has_glob(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'))
}

fn reject_component(text: &str) -> Result<(), LaneError> {
    if text.is_empty()
        || text.ends_with('.')
        || text.ends_with(' ')
        || text
            .chars()
            .any(|ch| matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'))
    {
        return Err(LaneError::new(
            Denial::Path,
            "path component is not a literal name",
        ));
    }
    let stem = text.split('.').next().unwrap_or(text);
    let folded = stem.to_ascii_uppercase();
    let device = matches!(folded.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (folded.len() == 4
            && (folded.starts_with("COM") || folded.starts_with("LPT"))
            && folded.as_bytes()[3].is_ascii_digit()
            && folded.as_bytes()[3] != b'0');
    if device {
        return Err(LaneError::new(
            Denial::Path,
            "path component is a device name",
        ));
    }
    Ok(())
}

fn normalize_drive(prefix: &std::ffi::OsStr) -> Result<String, LaneError> {
    let text = prefix
        .to_string_lossy()
        .to_ascii_lowercase()
        .replace('/', "\\");
    let stripped = [r"\\?\", r"\\.\"]
        .into_iter()
        .find_map(|marker| text.strip_prefix(marker))
        .unwrap_or(text.as_str());
    if stripped.starts_with(r"unc\") {
        return Err(LaneError::new(Denial::Path, "unc paths are rejected"));
    }
    let bytes = stripped.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Ok(format!("{}:", bytes[0] as char));
    }
    Err(LaneError::new(
        Denial::Path,
        "path prefix is not a local drive",
    ))
}

fn casefold(text: &str) -> String {
    #[cfg(windows)]
    {
        text.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        text.to_string()
    }
}
