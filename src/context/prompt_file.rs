use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::error::{ContextError, ErrorKind};
use super::journal::{resolve_state_root, StateDirEnv};
use super::winfile::refuse_reparse_chain;

const PREFIX: &str = "parley-prompt-";
static COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub(crate) struct PromptFile {
    path: PathBuf,
}

impl PromptFile {
    pub(crate) fn create(contents: &str) -> Result<Self, ContextError> {
        let root = resolve_state_root(&StateDirEnv::from_process())?;
        Self::create_at(&root, contents)
    }

    fn create_at(root: &Path, contents: &str) -> Result<Self, ContextError> {
        if !root.is_absolute() {
            return Err(ContextError::new(
                ErrorKind::Io,
                format!("context state root must be absolute: {}", root.display()),
            ));
        }
        let directory = root.join("prompt-temp");
        refuse_reparse_chain(root)?;
        ensure_directory(root)?;
        ensure_directory(&directory)?;
        refuse_reparse_chain(&directory)?;

        for _ in 0..32 {
            let name = format!(
                "{PREFIX}{}-{}-{}.txt",
                std::process::id(),
                timestamp_ms(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            );
            let path = directory.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    refuse_reparse_chain(&path)?;
                    file.write_all(contents.as_bytes())
                        .and_then(|_| file.flush())
                        .and_then(|_| file.sync_all())
                        .map_err(|error| io_error("write prompt file", &path, error))?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_error("create prompt file", &path, error)),
            }
        }
        Err(ContextError::new(
            ErrorKind::Io,
            "could not allocate a unique prompt file",
        ))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn cleanup(self) -> Result<(), ContextError> {
        refuse_reparse_chain(&self.path)?;
        fs::remove_file(&self.path)
            .map_err(|error| io_error("remove prompt file", &self.path, error))
    }
}

pub(crate) fn cleanup_stale_prompt_files(max_age: Duration) -> Result<(), ContextError> {
    let root = resolve_state_root(&StateDirEnv::from_process())?;
    cleanup_stale_prompt_files_at(&root, max_age)
}

fn cleanup_stale_prompt_files_at(root: &Path, max_age: Duration) -> Result<(), ContextError> {
    let directory = root.join("prompt-temp");
    if !directory.exists() {
        return Ok(());
    }
    refuse_reparse_chain(root)?;
    refuse_reparse_chain(&directory)?;
    let now = SystemTime::now();
    for entry in fs::read_dir(&directory)
        .map_err(|error| io_error("read prompt temp directory", &directory, error))?
    {
        let entry = entry.map_err(|error| io_error("read prompt temp entry", &directory, error))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(PREFIX) || !name.ends_with(".txt") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| io_error("inspect stale prompt file", &path, error))?;
        if is_reparse(&metadata) || !metadata.is_file() {
            continue;
        }
        let modified = metadata
            .modified()
            .map_err(|error| io_error("inspect prompt file timestamp", &path, error))?;
        if now.duration_since(modified).unwrap_or_default() > max_age {
            fs::remove_file(&path)
                .map_err(|error| io_error("remove stale prompt file", &path, error))?;
        }
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), ContextError> {
    fs::create_dir_all(path).map_err(|error| io_error("create prompt directory", path, error))?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| io_error("inspect prompt directory", path, error))?;
    if !metadata.is_dir() || is_reparse(&metadata) {
        return Err(ContextError::new(
            ErrorKind::Io,
            format!(
                "prompt directory is not a plain directory: {}",
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn io_error(action: &str, path: &Path, error: std::io::Error) -> ContextError {
    ContextError::new(
        ErrorKind::Io,
        format!("{action} {}: {error}", path.display()),
    )
}

fn timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn root(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "parley-prompt-test-{}-{}-{name}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn creates_unique_synced_files_and_cleans_each_exact_file() {
        let root = root("create");
        let first = PromptFile::create_at(&root, "one\n😀").unwrap();
        let second = PromptFile::create_at(&root, "two").unwrap();
        assert_ne!(first.path(), second.path());
        assert_eq!(fs::read_to_string(first.path()).unwrap(), "one\n😀");
        assert_eq!(fs::read_to_string(second.path()).unwrap(), "two");
        let first_path = first.path().to_path_buf();
        let second_path = second.path().to_path_buf();
        first.cleanup().unwrap();
        assert!(!first_path.exists());
        assert!(second_path.exists());
        second.cleanup().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_cleanup_is_bounded_to_owned_names() {
        let root = root("stale");
        let owned = PromptFile::create_at(&root, "stale").unwrap();
        let owned_path = owned.path().to_path_buf();
        let directory = root.join("prompt-temp");
        let unrelated = directory.join("keep-me.txt");
        fs::write(&unrelated, "evidence").unwrap();
        std::thread::sleep(Duration::from_millis(2));
        cleanup_stale_prompt_files_at(&root, Duration::ZERO).unwrap();
        assert!(!owned_path.exists());
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "evidence");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_relative_state_roots() {
        let error = PromptFile::create_at(Path::new("relative-state"), "prompt").unwrap_err();
        assert!(error.to_string().contains("must be absolute"));
    }
}
