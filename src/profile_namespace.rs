//! Stable, sanitized identity for one locked asynchronous profile.

use std::env;
use std::fs;
use std::path::Path;

use crate::signals::fnv1a_64;

pub(crate) fn from_env() -> Result<String, String> {
    let caller = env::var("PARLEY_CALLER").unwrap_or_else(|_| "parley".to_string());
    validate_caller(&caller)?;
    let configured_root = env::var_os("PARLEY_ALLOWED_CWD_ROOT")
        .ok_or_else(|| "asynchronous jobs require PARLEY_ALLOWED_CWD_ROOT".to_string())?;
    let configured_root = Path::new(&configured_root);
    if !configured_root.is_dir() {
        return Err(format!(
            "PARLEY_ALLOWED_CWD_ROOT is not an existing directory: {}",
            configured_root.display()
        ));
    }
    let canonical_root = fs::canonicalize(configured_root).map_err(|error| {
        format!(
            "canonicalize PARLEY_ALLOWED_CWD_ROOT {}: {error}",
            configured_root.display()
        )
    })?;
    Ok(for_root(&caller, &canonical_root))
}

fn for_root(caller: &str, canonical_root: &Path) -> String {
    let root = canonical_root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();
    format!("profile-{:016x}", fnv1a_64(&format!("{caller}\0{root}")))
}

fn validate_caller(caller: &str) -> Result<(), String> {
    if caller.is_empty()
        || caller.len() > 64
        || caller.starts_with('.')
        || caller.contains("..")
        || !caller.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return Err(
            "PARLEY_CALLER must be one path segment of letters, digits, '.', '_' or '-'"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_caller_with_different_roots_has_distinct_namespace() {
        let first = for_root("codex", Path::new(r"C:\approved\r3"));
        let second = for_root("codex", Path::new(r"C:\approved\parley"));
        assert_ne!(first, second);
    }

    #[test]
    fn root_spelling_is_normalized_without_leaking_the_path() {
        let first = for_root("codex", Path::new(r"C:\Approved\R3\\"));
        let second = for_root("codex", Path::new(r"c:\approved\r3"));
        assert_eq!(first, second);
        assert!(!first.contains("approved"));
        assert_eq!(first.len(), "profile-".len() + 16);
    }

    #[test]
    fn caller_contributes_to_profile_identity() {
        let root = Path::new(r"C:\approved\r3");
        assert_ne!(for_root("codex", root), for_root("other", root));
    }

    #[test]
    fn invalid_callers_are_rejected_before_hashing() {
        for caller in ["", ".hidden", "two..dots", "bad/path", "bad\\path"] {
            assert!(validate_caller(caller).is_err(), "{caller}");
        }
    }
}
