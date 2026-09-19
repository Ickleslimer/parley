use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const HARDENED_GROK_DENIES: &[&str] = &[
    "WebFetch",
    "WebSearch",
    "MCPTool",
    "Bash(git push*)",
    "Bash(git fetch*)",
    "Bash(git pull*)",
    "Bash(git clone*)",
    "Bash(git commit*)",
    "Bash(git checkout*)",
    "Bash(git switch*)",
    "Bash(git branch*)",
    "Bash(git tag*)",
    "Bash(git reset*)",
    "Bash(git restore*)",
    "Bash(git clean*)",
    "Bash(git rebase*)",
    "Bash(git merge*)",
    "Bash(git cherry-pick*)",
    "Bash(git revert*)",
    "Bash(git update-ref*)",
    "Bash(git symbolic-ref*)",
    "Bash(git replace*)",
    "Bash(git stash*)",
    "Bash(git worktree*)",
    "Bash(git submodule*)",
    "Bash(git rm*)",
    "Bash(git init*)",
    "Bash(rm *)",
    "Bash(rmdir *)",
    "Bash(unlink *)",
    "Bash(del *)",
    "Bash(erase *)",
    "Bash(rd *)",
    "Bash(Remove-Item*)",
    "Bash(remove-item*)",
    "Bash(Clear-Content*)",
    "Bash(clear-content*)",
    "Bash(cmd *)",
    "Bash(cmd.exe *)",
    "Bash(powershell *)",
    "Bash(powershell.exe *)",
    "Bash(pwsh *)",
    "Bash(bash *)",
    "Bash(sh *)",
    "Bash(zsh *)",
    "Bash(fish *)",
    "Bash(wsl *)",
    "Bash(python -c *)",
    "Bash(py -c *)",
    "Bash(node -e *)",
    "Bash(node --eval *)",
    "Bash(curl*)",
    "Bash(wget*)",
    "Bash(Invoke-WebRequest*)",
    "Bash(invoke-webrequest*)",
    "Bash(iwr *)",
    "Bash(Invoke-RestMethod*)",
    "Bash(invoke-restmethod*)",
    "Bash(irm *)",
    "Bash(Start-BitsTransfer*)",
    "Bash(start-bitstransfer*)",
    "Bash(bitsadmin*)",
    "Bash(certutil*)",
    "Bash(aria2c*)",
    "Bash(ftp *)",
    "Bash(scp *)",
    "Bash(npx*)",
    "Bash(pnpx*)",
    "Bash(bunx*)",
    "Bash(npm install*)",
    "Bash(npm i *)",
    "Bash(npm add*)",
    "Bash(npm exec*)",
    "Bash(pnpm install*)",
    "Bash(pnpm add*)",
    "Bash(yarn install*)",
    "Bash(yarn add*)",
    "Bash(bun install*)",
    "Bash(bun add*)",
    "Bash(pip install*)",
    "Bash(pip3 install*)",
    "Bash(python -m pip install*)",
    "Bash(py -m pip install*)",
    "Bash(uv add*)",
    "Bash(uv sync*)",
    "Bash(uv pip install*)",
    "Bash(cargo install*)",
    "Bash(dotnet add * package*)",
    "Bash(dotnet tool install*)",
    "Bash(dotnet tool update*)",
    "Bash(go install*)",
    "Bash(gem install*)",
    "Bash(composer install*)",
    "Bash(composer require*)",
    "Bash(composer update*)",
    "Bash(winget*)",
    "Bash(choco*)",
    "Bash(scoop*)",
    "Bash(apt *)",
    "Bash(apt-get*)",
    "Bash(apk *)",
    "Bash(dnf *)",
    "Bash(yum *)",
    "Bash(pacman *)",
    "Bash(zypper *)",
    "Bash(brew *)",
    "Bash(port install*)",
    "Bash(snap install*)",
];

#[derive(Clone, Debug)]
pub(crate) struct RuntimePolicy {
    allowed_cwd_root: Option<PathBuf>,
    mcp_default_yolo: bool,
    disable_yolo: bool,
    grok_locked_permission_mode: Option<String>,
    grok_require_session_id: bool,
    grok_max_turns: Option<u64>,
    grok_denies: Vec<String>,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            allowed_cwd_root: None,
            mcp_default_yolo: true,
            disable_yolo: false,
            grok_locked_permission_mode: None,
            grok_require_session_id: false,
            grok_max_turns: None,
            grok_denies: Vec::new(),
        }
    }
}

impl RuntimePolicy {
    pub(crate) fn from_env() -> Result<Self, String> {
        let allowed_cwd_root = match env::var_os("PARLEY_ALLOWED_CWD_ROOT") {
            Some(value) => {
                let path = PathBuf::from(value);
                if !path.is_dir() {
                    return Err(format!(
                        "PARLEY_ALLOWED_CWD_ROOT is not an existing directory: {}",
                        path.display()
                    ));
                }
                Some(fs::canonicalize(&path).map_err(|error| {
                    format!(
                        "canonicalize PARLEY_ALLOWED_CWD_ROOT {}: {error}",
                        path.display()
                    )
                })?)
            }
            None => None,
        };

        let grok_max_turns = match env::var("PARLEY_GROK_MAX_TURNS") {
            Ok(value) => {
                let turns = value.trim().parse::<u64>().map_err(|_| {
                    format!("PARLEY_GROK_MAX_TURNS must be a positive integer, got {value}")
                })?;
                if turns == 0 {
                    return Err("PARLEY_GROK_MAX_TURNS must be greater than zero".to_string());
                }
                Some(turns)
            }
            Err(env::VarError::NotPresent) => None,
            Err(error) => return Err(format!("read PARLEY_GROK_MAX_TURNS: {error}")),
        };

        let grok_locked_permission_mode = env_nonempty("PARLEY_GROK_LOCKED_PERMISSION_MODE");
        let grok_denies = if grok_locked_permission_mode.is_some() {
            hardened_grok_denies()
        } else {
            Vec::new()
        };

        Ok(Self {
            allowed_cwd_root,
            mcp_default_yolo: env_bool("PARLEY_MCP_DEFAULT_YOLO")?.unwrap_or(true),
            disable_yolo: env_bool("PARLEY_DISABLE_YOLO")?.unwrap_or(false),
            grok_locked_permission_mode,
            grok_require_session_id: env_bool("PARLEY_GROK_REQUIRE_SESSION_ID")?.unwrap_or(false),
            grok_max_turns,
            grok_denies,
        })
    }

    pub(crate) fn resolve_mcp_yolo(&self, requested: Option<bool>) -> Result<bool, String> {
        let yolo = requested.unwrap_or(self.mcp_default_yolo);
        if self.disable_yolo && yolo {
            return Err("permission bypass is disabled by PARLEY_DISABLE_YOLO".to_string());
        }
        Ok(yolo)
    }

    pub(crate) fn apply_request(
        &self,
        harness: &str,
        yolo: bool,
        permission_mode: &mut Option<String>,
        max_turns: &mut Option<String>,
        session_id: &Option<String>,
        resume_id: &Option<String>,
    ) -> Result<(), String> {
        if self.disable_yolo && (yolo || permission_mode.as_deref() == Some("bypassPermissions")) {
            return Err("permission bypass is disabled by PARLEY_DISABLE_YOLO".to_string());
        }
        if harness != "grok" {
            return Ok(());
        }

        if session_id.is_some() && resume_id.is_some() {
            return Err("--session-id and --resume-id are mutually exclusive".to_string());
        }
        if yolo && (permission_mode.is_some() || self.grok_locked_permission_mode.is_some()) {
            return Err("--yolo cannot be combined with --permission-mode".to_string());
        }
        if let Some(id) = session_id {
            if !is_uuid(id) {
                return Err(format!("--session-id must be a UUID, got {id}"));
            }
        }

        if let Some(locked) = &self.grok_locked_permission_mode {
            if let Some(requested) = permission_mode.as_deref() {
                if requested != locked {
                    return Err(format!(
                        "Grok permission mode is locked to {locked}; requested {requested}"
                    ));
                }
            }
            *permission_mode = Some(locked.clone());
        }
        let requested_turns = max_turns
            .as_deref()
            .map(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|turns| *turns > 0)
                    .ok_or_else(|| format!("--max-turns must be a positive integer, got {value}"))
            })
            .transpose()?;
        if let Some(limit) = self.grok_max_turns {
            if let Some(requested) = requested_turns {
                if requested > limit {
                    return Err(format!(
                        "Grok max turns is capped at {limit}; requested {requested}"
                    ));
                }
            } else {
                *max_turns = Some(limit.to_string());
            }
        }
        if self.grok_require_session_id && session_id.is_none() && resume_id.is_none() {
            return Err(
                "Grok calls require session_id or resume_id under PARLEY_GROK_REQUIRE_SESSION_ID"
                    .to_string(),
            );
        }
        Ok(())
    }

    pub(crate) fn validate_spawn_cwd(&self, cwd: &Path) -> Result<PathBuf, String> {
        let resolved = fs::canonicalize(cwd).map_err(|error| {
            format!(
                "working directory {} is unavailable: {error}",
                cwd.display()
            )
        })?;
        if !resolved.is_dir() {
            return Err(format!(
                "working directory is not a directory: {}",
                resolved.display()
            ));
        }

        if let Some(root) = &self.allowed_cwd_root {
            if paths_equal(&resolved, root) || !path_starts_with(&resolved, root) {
                return Err(format!(
                    "working directory {} is outside the approved worker root {}",
                    resolved.display(),
                    root.display()
                ));
            }
        }
        Ok(resolved)
    }

    pub(crate) fn grok_denies(&self) -> &[String] {
        &self.grok_denies
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        allowed_cwd_root: &Path,
        mcp_default_yolo: bool,
        disable_yolo: bool,
    ) -> Self {
        Self {
            allowed_cwd_root: Some(fs::canonicalize(allowed_cwd_root).unwrap()),
            mcp_default_yolo,
            disable_yolo,
            ..Self::default()
        }
    }
}

fn env_bool(name: &str) -> Result<Option<bool>, String> {
    let value = match env::var(name) {
        Ok(value) => value,
        Err(env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(format!("read {name}: {error}")),
    };
    let parsed = match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        _ => return Err(format!("{name} must be true or false, got {value}")),
    };
    Ok(Some(parsed))
}

fn env_nonempty(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn hardened_grok_denies() -> Vec<String> {
    HARDENED_GROK_DENIES
        .iter()
        .map(|rule| match rule.strip_prefix("Bash(") {
            Some(pattern) => format!("Bash(*{pattern}"),
            None => (*rule).to_string(),
        })
        .collect()
}

fn is_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn path_starts_with(path: &Path, root: &Path) -> bool {
    if !cfg!(windows) {
        return path.starts_with(root);
    }
    let path_components = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    let root_components = root
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    path_components.starts_with(&root_components)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(1);

    fn temp_root() -> PathBuf {
        env::temp_dir().join(format!(
            "parley-policy-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn validates_uuid_shape() {
        assert!(is_uuid("01a06582-d66e-7811-b0c9-0b0266e17903"));
        assert!(!is_uuid("not-a-uuid"));
        assert!(!is_uuid("01a06582d66e-7811-b0c9-0b0266e17903"));
    }

    #[test]
    fn rejects_conflicting_session_and_permission_options() {
        let policy = RuntimePolicy::default();
        let mut mode = Some("auto".to_string());
        let mut turns = None;
        assert!(policy
            .apply_request(
                "grok",
                true,
                &mut mode,
                &mut turns,
                &Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string()),
                &None,
            )
            .is_err());

        let mut mode = None;
        assert!(policy
            .apply_request(
                "grok",
                false,
                &mut mode,
                &mut turns,
                &Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string()),
                &Some("01a06582-d66e-7811-b0c9-0b0266e17904".to_string()),
            )
            .unwrap_err()
            .contains("mutually exclusive"));

        assert!(policy
            .apply_request(
                "grok",
                false,
                &mut mode,
                &mut turns,
                &Some("not-a-uuid".to_string()),
                &None,
            )
            .unwrap_err()
            .contains("must be a UUID"));
    }

    #[test]
    fn enforces_canonical_descendants_only() {
        let root = temp_root();
        let child = root.join("worker");
        let sibling = root.with_file_name(format!(
            "{}-sibling",
            root.file_name().unwrap().to_string_lossy()
        ));
        fs::create_dir_all(&child).unwrap();
        fs::create_dir_all(&sibling).unwrap();
        let policy = RuntimePolicy {
            allowed_cwd_root: Some(fs::canonicalize(&root).unwrap()),
            ..RuntimePolicy::default()
        };

        assert_eq!(
            policy.validate_spawn_cwd(&child).unwrap(),
            fs::canonicalize(&child).unwrap()
        );
        assert!(policy.validate_spawn_cwd(&root).is_err());
        assert!(policy.validate_spawn_cwd(&sibling).is_err());
        assert!(policy.validate_spawn_cwd(&root.join("missing")).is_err());

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(sibling).unwrap();
    }

    #[test]
    fn locked_grok_policy_injects_defaults_and_caps_turns() {
        let policy = RuntimePolicy {
            grok_locked_permission_mode: Some("auto".to_string()),
            grok_require_session_id: true,
            grok_max_turns: Some(30),
            grok_denies: hardened_grok_denies(),
            ..RuntimePolicy::default()
        };
        let id = Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string());
        let mut mode = None;
        let mut turns = None;
        policy
            .apply_request("grok", false, &mut mode, &mut turns, &id, &None)
            .unwrap();
        assert_eq!(mode.as_deref(), Some("auto"));
        assert_eq!(turns.as_deref(), Some("30"));

        let mut mode = Some("auto".to_string());
        let mut turns = Some("12".to_string());
        policy
            .apply_request("grok", false, &mut mode, &mut turns, &id, &None)
            .unwrap();
        assert_eq!(turns.as_deref(), Some("12"));

        let mut turns = Some("31".to_string());
        assert!(policy
            .apply_request("grok", false, &mut mode, &mut turns, &id, &None)
            .unwrap_err()
            .contains("capped at 30"));

        let mut wrong_mode = Some("default".to_string());
        let mut turns = None;
        assert!(policy
            .apply_request("grok", false, &mut wrong_mode, &mut turns, &id, &None,)
            .unwrap_err()
            .contains("locked to auto"));

        let mut mode = None;
        assert!(policy
            .apply_request("grok", false, &mut mode, &mut turns, &None, &None)
            .unwrap_err()
            .contains("require session_id or resume_id"));
    }

    #[test]
    fn hardened_denies_cover_required_categories() {
        let denies = hardened_grok_denies();
        for rule in [
            "WebFetch",
            "WebSearch",
            "MCPTool",
            "Bash(*git push*)",
            "Bash(*Remove-Item*)",
            "Bash(*pwsh *)",
            "Bash(*curl*)",
            "Bash(*npm install*)",
            "Bash(*winget*)",
        ] {
            assert!(denies.iter().any(|candidate| candidate == rule), "{rule}");
        }
    }

    #[test]
    fn disable_yolo_overrides_legacy_mcp_default() {
        let policy = RuntimePolicy {
            disable_yolo: true,
            ..RuntimePolicy::default()
        };
        assert!(policy.resolve_mcp_yolo(None).is_err());
        assert!(!policy.resolve_mcp_yolo(Some(false)).unwrap());

        let mut mode = Some("bypassPermissions".to_string());
        let mut turns = None;
        assert!(policy
            .apply_request("grok", false, &mut mode, &mut turns, &None, &None)
            .unwrap_err()
            .contains("permission bypass is disabled"));
    }

    #[test]
    fn legacy_harnesses_keep_non_uuid_session_semantics() {
        let policy = RuntimePolicy::default();
        let mut mode = None;
        let mut turns = None;
        policy
            .apply_request(
                "claude",
                false,
                &mut mode,
                &mut turns,
                &Some("legacy-session-name".to_string()),
                &Some("legacy-resume-name".to_string()),
            )
            .unwrap();
    }
}
