use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::schema::HealthError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthPaths {
    pub root: PathBuf,
}

impl HealthPaths {
    pub fn from_env() -> Self {
        if let Some(override_root) = env::var_os("PARLEY_HEALTH_HOME").map(PathBuf::from) {
            if override_root.is_absolute() {
                return Self { root: override_root };
            }
        }
        Self {
            root: local_app_data().join("Parley").join("health"),
        }
    }

    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn journal(&self) -> PathBuf {
        self.root.join("journal.jsonl")
    }

    pub fn snapshot(&self) -> PathBuf {
        self.root.join("snapshot.json")
    }

    pub fn state(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn scope(&self) -> PathBuf {
        self.root.join("scope.json")
    }

    pub fn inbox(&self) -> PathBuf {
        self.root.join("inbox")
    }

    pub fn quarantine(&self) -> PathBuf {
        self.root.join("quarantine")
    }

    pub fn ensure(&self) -> Result<(), HealthError> {
        create_dir(&self.root)?;
        create_dir(&self.inbox())?;
        create_dir(&self.quarantine())?;
        Ok(())
    }
}

fn create_dir(path: &Path) -> Result<(), HealthError> {
    fs::create_dir_all(path).map_err(|error| {
        HealthError::msg(format!("create {}: {error}", path.display()))
    })
}

fn local_app_data() -> PathBuf {
    if let Some(path) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path);
    }
    if let Some(home) = env::var_os("USERPROFILE") {
        return PathBuf::from(home).join("AppData").join("Local");
    }
    PathBuf::from(r"C:\ProgramData")
}
