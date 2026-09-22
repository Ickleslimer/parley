use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::context::{refuse_reparse_chain, ContextMode, ContextRecovery};
use crate::jobs::{HandoffMode, JobMode};

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
    grok_locked_model: Option<String>,
    grok_locked_version: Option<String>,
    grok_locked_reasoning_effort: Option<String>,
    grok_locked_permission_mode: Option<String>,
    grok_require_session_id: bool,
    grok_max_turns: Option<u64>,
    grok_denies: Vec<String>,
    grok_allows: Vec<String>,
    grok_health_query_command: Option<String>,
    grok_require_handoff_footer: bool,
    grok_subagent_mode: GrokSubagentMode,
    guarded_subagents: Option<GuardedSubagentPolicy>,
    context_locked_source: Option<String>,
    context_locked_mode: Option<ContextMode>,
    context_require_explicit_session: bool,
    handoff: Option<HandoffPolicy>,
}

#[derive(Clone, Debug)]
pub(crate) struct HandoffPolicy {
    pub(crate) state_dir: PathBuf,
    pub(crate) peer_peek_exe: PathBuf,
    pub(crate) wait_exe: PathBuf,
    pub(crate) alert_exe: PathBuf,
    pub(crate) hook_exe: PathBuf,
    pub(crate) peer_peek_command: String,
    pub(crate) wait_command: String,
    pub(crate) alert_command: String,
    pub(crate) wait_slice_secs: u64,
    pub(crate) execution_ceiling_secs: u64,
    pub(crate) acknowledgement_ceiling_secs: u64,
    pub(crate) excerpt_chars: usize,
    pub(crate) max_turns: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct GuardedSubagentPolicy {
    pub(crate) lane_state_dir: PathBuf,
    pub(crate) hook_exe: PathBuf,
    pub(crate) max_writers: usize,
    pub(crate) model: String,
    pub(crate) reasoning_effort: String,
    typed_roles_ready: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GrokSubagentMode {
    #[default]
    Unspecified,
    Off,
    Guarded,
}

pub(crate) struct PolicyRequest<'a> {
    pub harness: &'a str,
    pub yolo: bool,
    pub model: &'a mut Option<String>,
    pub reasoning_effort: &'a mut Option<String>,
    pub permission_mode: &'a mut Option<String>,
    pub max_turns: &'a mut Option<String>,
    pub session_id: Option<&'a str>,
    pub resume_id: Option<&'a str>,
    pub passthrough: &'a [String],
}

pub(crate) struct ContextPolicyRequest<'a> {
    pub target_harness: &'a str,
    pub source_harness: &'a mut String,
    pub source_session: &'a str,
    pub mode: &'a mut ContextMode,
    pub mode_explicit: bool,
    pub recovery: Option<ContextRecovery>,
    pub max_context_chars: Option<usize>,
    pub session_id: Option<&'a str>,
    pub resume_id: Option<&'a str>,
}

impl Default for RuntimePolicy {
    fn default() -> Self {
        Self {
            allowed_cwd_root: None,
            mcp_default_yolo: true,
            disable_yolo: false,
            grok_locked_model: None,
            grok_locked_version: None,
            grok_locked_reasoning_effort: None,
            grok_locked_permission_mode: None,
            grok_require_session_id: false,
            grok_max_turns: None,
            grok_denies: Vec::new(),
            grok_allows: Vec::new(),
            grok_health_query_command: None,
            grok_require_handoff_footer: false,
            grok_subagent_mode: GrokSubagentMode::Unspecified,
            guarded_subagents: None,
            context_locked_source: None,
            context_locked_mode: None,
            context_require_explicit_session: false,
            handoff: None,
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

        let grok_locked_model = env_nonempty("PARLEY_GROK_LOCKED_MODEL");
        let grok_locked_version = env_nonempty("PARLEY_GROK_LOCKED_VERSION");
        let grok_locked_reasoning_effort = env_nonempty("PARLEY_GROK_LOCKED_REASONING_EFFORT")
            .map(|value| normalize_reasoning_effort(&value))
            .transpose()?;
        let grok_locked_permission_mode = env_nonempty("PARLEY_GROK_LOCKED_PERMISSION_MODE");
        let mut grok_denies = if grok_locked_permission_mode.is_some() {
            hardened_grok_denies()
        } else {
            Vec::new()
        };
        let grok_health_query_exe = match env_nonempty("PARLEY_GROK_HEALTH_QUERY_EXE") {
            Some(value) => {
                let path = PathBuf::from(value);
                if !path.is_absolute() || !path.is_file() {
                    return Err(format!(
                        "PARLEY_GROK_HEALTH_QUERY_EXE is not an existing absolute file: {}",
                        path.display()
                    ));
                }
                Some(fs::canonicalize(&path).map_err(|error| {
                    format!(
                        "canonicalize PARLEY_GROK_HEALTH_QUERY_EXE {}: {error}",
                        path.display()
                    )
                })?)
            }
            None => None,
        };
        let grok_require_handoff_footer =
            env_bool("PARLEY_GROK_REQUIRE_HANDOFF_FOOTER")?.unwrap_or(false);
        let grok_subagent_mode = match env_nonempty("PARLEY_GROK_SUBAGENT_MODE")
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None => GrokSubagentMode::Unspecified,
            Some("off") => GrokSubagentMode::Off,
            Some("guarded") => GrokSubagentMode::Guarded,
            Some(value) => {
                return Err(format!(
                    "PARLEY_GROK_SUBAGENT_MODE must be off or guarded, got {value}"
                ))
            }
        };
        let typed_roles_ready =
            env_bool("PARLEY_GROK_SUBAGENT_TYPED_ROLES_READY")?.unwrap_or(false);
        if typed_roles_ready && grok_subagent_mode != GrokSubagentMode::Guarded {
            return Err(
                "PARLEY_GROK_SUBAGENT_TYPED_ROLES_READY requires PARLEY_GROK_SUBAGENT_MODE=guarded"
                    .to_string(),
            );
        }
        let guarded_subagents = if grok_subagent_mode == GrokSubagentMode::Guarded {
            let lane_state_dir = env::var_os("PARLEY_LANE_STATE_DIR")
                .map(PathBuf::from)
                .ok_or_else(|| {
                    "PARLEY_GROK_SUBAGENT_MODE=guarded requires PARLEY_LANE_STATE_DIR".to_string()
                })?;
            if !lane_state_dir.is_absolute() {
                return Err("PARLEY_LANE_STATE_DIR must be absolute".to_string());
            }
            refuse_reparse_chain(&lane_state_dir).map_err(|error| error.to_string())?;
            let hook_exe = env::var_os("PARLEY_GROK_LANE_HOOK_EXE")
                .map(PathBuf::from)
                .ok_or_else(|| {
                    "PARLEY_GROK_SUBAGENT_MODE=guarded requires PARLEY_GROK_LANE_HOOK_EXE"
                        .to_string()
                })?;
            if !hook_exe.is_absolute() || !hook_exe.is_file() {
                return Err(format!(
                    "PARLEY_GROK_LANE_HOOK_EXE is not an existing absolute file: {}",
                    hook_exe.display()
                ));
            }
            refuse_reparse_chain(&hook_exe).map_err(|error| error.to_string())?;
            let hook_exe = fs::canonicalize(&hook_exe).map_err(|error| {
                format!(
                    "canonicalize PARLEY_GROK_LANE_HOOK_EXE {}: {error}",
                    hook_exe.display()
                )
            })?;
            let max_writers = env_nonempty("PARLEY_GROK_SUBAGENT_MAX_WRITERS")
                .ok_or_else(|| {
                    "PARLEY_GROK_SUBAGENT_MODE=guarded requires PARLEY_GROK_SUBAGENT_MAX_WRITERS"
                        .to_string()
                })?
                .parse::<usize>()
                .map_err(|_| "PARLEY_GROK_SUBAGENT_MAX_WRITERS must be an integer".to_string())?;
            if max_writers != 2 {
                return Err(
                    "guarded Grok subagents require PARLEY_GROK_SUBAGENT_MAX_WRITERS=2".to_string(),
                );
            }
            let model = grok_locked_model.clone().ok_or_else(|| {
                "guarded Grok subagents require PARLEY_GROK_LOCKED_MODEL".to_string()
            })?;
            let reasoning_effort = grok_locked_reasoning_effort.clone().ok_or_else(|| {
                "guarded Grok subagents require PARLEY_GROK_LOCKED_REASONING_EFFORT".to_string()
            })?;
            if model != "grok-4.7" || reasoning_effort != "xhigh" {
                return Err(
                    "guarded Grok subagents require locked grok-4.7 with xhigh reasoning"
                        .to_string(),
                );
            }
            if grok_locked_version.is_none() || grok_locked_permission_mode.is_none() {
                return Err(
                    "guarded Grok subagents require locked CLI version and permission mode"
                        .to_string(),
                );
            }
            Some(GuardedSubagentPolicy {
                lane_state_dir,
                hook_exe,
                max_writers,
                model,
                reasoning_effort,
                typed_roles_ready,
            })
        } else {
            None
        };
        if let Some(guarded) = &guarded_subagents {
            add_lane_hook_tool_rules(&guarded.hook_exe, &mut grok_denies);
        }
        let context_locked_source =
            env_nonempty("PARLEY_CONTEXT_LOCKED_SOURCE").map(|value| value.to_ascii_lowercase());
        let context_locked_mode = env_nonempty("PARLEY_CONTEXT_LOCKED_MODE")
            .map(|value| ContextMode::parse(&value))
            .transpose()?;
        let context_require_explicit_session =
            env_bool("PARLEY_CONTEXT_REQUIRE_EXPLICIT_SESSION")?.unwrap_or(false);
        if grok_require_handoff_footer && grok_health_query_exe.is_none() {
            return Err(
                "PARLEY_GROK_REQUIRE_HANDOFF_FOOTER requires PARLEY_GROK_HEALTH_QUERY_EXE"
                    .to_string(),
            );
        }
        let mut grok_allows = Vec::new();
        let mut grok_health_query_command = None;
        if let Some(query) = &grok_health_query_exe {
            if grok_locked_permission_mode.is_none() {
                return Err(
                    "PARLEY_GROK_HEALTH_QUERY_EXE requires PARLEY_GROK_LOCKED_PERMISSION_MODE"
                        .to_string(),
                );
            }
            let (allows, command) = add_health_tool_rules(query, &mut grok_denies)?;
            grok_allows = allows;
            grok_health_query_command = Some(command);
        }
        let handoff = match env_nonempty("PARLEY_HANDOFF_LOCKED_MODE")
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("off") => None,
            Some("required") => {
                if grok_locked_permission_mode.is_none()
                    || grok_locked_model.as_deref() != Some("grok-4.7")
                    || grok_locked_reasoning_effort.as_deref() != Some("xhigh")
                    || grok_locked_version.is_none()
                {
                    return Err("required handoffs require locked Grok 4.7/XHigh, CLI version, and permission mode".to_string());
                }
                let state_dir = required_absolute_dir_value("PARLEY_HANDOFF_STATE_DIR")?;
                let peer_peek_exe = required_executable("PARLEY_HANDOFF_PEER_PEEK_EXE")?;
                let wait_exe = required_executable("PARLEY_HANDOFF_WAIT_EXE")?;
                let alert_exe = required_executable("PARLEY_HANDOFF_ALERT_EXE")?;
                let hook_exe = required_executable("PARLEY_HANDOFF_HOOK_EXE")?;
                let wait_slice_secs = required_exact_u64("PARLEY_HANDOFF_WAIT_SLICE_SEC", 300)?;
                let execution_ceiling_secs =
                    required_exact_u64("PARLEY_HANDOFF_EXECUTION_CEILING_SEC", 21_600)?;
                let acknowledgement_ceiling_secs =
                    required_exact_u64("PARLEY_HANDOFF_ACK_CEILING_SEC", 21_600)?;
                let excerpt_chars =
                    required_exact_u64("PARLEY_HANDOFF_EXCERPT_CHARS", 4_096)? as usize;
                let max_turns = required_exact_u64("PARLEY_HANDOFF_MAX_TURNS", 128)?;
                let peer_peek_command = powershell_direct_command(&peer_peek_exe)?;
                let wait_command = powershell_direct_command(&wait_exe)?;
                let alert_command = powershell_direct_command(&alert_exe)?;
                grok_allows.extend([
                    format!("Bash({peer_peek_command})"),
                    format!("Bash({wait_command})"),
                    format!("Bash({alert_command})"),
                ]);
                add_handoff_hook_tool_rules(&hook_exe, &mut grok_denies);
                Some(HandoffPolicy {
                    state_dir,
                    peer_peek_exe,
                    wait_exe,
                    alert_exe,
                    hook_exe,
                    peer_peek_command,
                    wait_command,
                    alert_command,
                    wait_slice_secs,
                    execution_ceiling_secs,
                    acknowledgement_ceiling_secs,
                    excerpt_chars,
                    max_turns,
                })
            }
            Some(value) => {
                return Err(format!(
                    "PARLEY_HANDOFF_LOCKED_MODE must be off or required, got {value}"
                ))
            }
        };

        Ok(Self {
            allowed_cwd_root,
            mcp_default_yolo: env_bool("PARLEY_MCP_DEFAULT_YOLO")?.unwrap_or(true),
            disable_yolo: env_bool("PARLEY_DISABLE_YOLO")?.unwrap_or(false),
            grok_locked_model,
            grok_locked_version,
            grok_locked_reasoning_effort,
            grok_locked_permission_mode,
            grok_require_session_id: env_bool("PARLEY_GROK_REQUIRE_SESSION_ID")?.unwrap_or(false),
            grok_max_turns,
            grok_denies,
            grok_allows,
            grok_health_query_command,
            grok_require_handoff_footer,
            grok_subagent_mode,
            guarded_subagents,
            context_locked_source,
            context_locked_mode,
            context_require_explicit_session,
            handoff,
        })
    }

    pub(crate) fn resolve_mcp_yolo(&self, requested: Option<bool>) -> Result<bool, String> {
        let yolo = requested.unwrap_or(self.mcp_default_yolo);
        if self.disable_yolo && yolo {
            return Err("permission bypass is disabled by PARLEY_DISABLE_YOLO".to_string());
        }
        Ok(yolo)
    }

    pub(crate) fn apply_request(&self, request: PolicyRequest<'_>) -> Result<(), String> {
        let PolicyRequest {
            harness,
            yolo,
            model,
            reasoning_effort,
            permission_mode,
            max_turns,
            session_id,
            resume_id,
            passthrough,
        } = request;
        if self.disable_yolo && (yolo || permission_mode.as_deref() == Some("bypassPermissions")) {
            return Err("permission bypass is disabled by PARLEY_DISABLE_YOLO".to_string());
        }
        if harness != "grok" {
            if reasoning_effort.is_some() {
                return Err("--reasoning-effort is only supported for Grok".to_string());
            }
            return Ok(());
        }

        if let Some(expected) = &self.grok_locked_version {
            validate_grok_version(expected)?;
        }

        if let Some(flag) = grok_passthrough_control(passthrough) {
            return Err(format!(
                "Grok model and reasoning controls must use structured Parley options, not passthrough: {flag}"
            ));
        }

        if let Some(requested) = reasoning_effort.as_deref() {
            *reasoning_effort = Some(normalize_reasoning_effort(requested)?);
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

        if let Some(locked) = &self.grok_locked_model {
            if let Some(requested) = model.as_deref() {
                if requested != locked {
                    return Err(format!(
                        "Grok model is locked to {locked}; requested {requested}"
                    ));
                }
            }
            *model = Some(locked.clone());
        }
        if let Some(locked) = &self.grok_locked_reasoning_effort {
            if let Some(requested) = reasoning_effort.as_deref() {
                if requested != locked {
                    return Err(format!(
                        "Grok reasoning effort is locked to {locked}; requested {requested}"
                    ));
                }
            }
            *reasoning_effort = Some(locked.clone());
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

    pub(crate) fn apply_context(&self, request: ContextPolicyRequest<'_>) -> Result<(), String> {
        let ContextPolicyRequest {
            target_harness,
            source_harness,
            source_session,
            mode,
            mode_explicit,
            recovery,
            max_context_chars,
            session_id,
            resume_id,
        } = request;
        if let Some(locked) = &self.context_locked_source {
            if !source_harness.is_empty() && source_harness != locked {
                return Err(format!(
                    "context source is locked to {locked}; requested {source_harness}"
                ));
            }
            *source_harness = locked.clone();
        }
        if let Some(locked) = self.context_locked_mode {
            if mode_explicit && *mode != locked {
                return Err(format!(
                    "context mode is locked to {}; requested {}",
                    locked.as_str(),
                    mode.as_str()
                ));
            }
            if !mode_explicit {
                *mode = locked;
            }
        }

        if *mode == ContextMode::Snapshot {
            if recovery.is_some() {
                return Err("context recovery is only valid for uncertain auto resumes".to_string());
            }
            return Ok(());
        }
        if target_harness != "grok" {
            return Err("stateful context modes are only supported for Grok".to_string());
        }
        if source_harness != "codex" {
            return Err("stateful context modes require source harness codex".to_string());
        }
        if source_session.is_empty()
            || source_session.eq_ignore_ascii_case("latest")
            || source_session.eq_ignore_ascii_case("last")
        {
            return Err(
                "stateful context requires an explicit Codex source session id".to_string(),
            );
        }
        if max_context_chars.is_some() {
            return Err("--max-context applies only to snapshot context mode".to_string());
        }
        if session_id.is_none() && resume_id.is_none() {
            return Err(
                "stateful context requires an explicit target session_id or resume_id".to_string(),
            );
        }
        if self.context_require_explicit_session && (session_id.is_none() && resume_id.is_none()) {
            return Err(
                "context requires an explicit target session under PARLEY_CONTEXT_REQUIRE_EXPLICIT_SESSION"
                    .to_string(),
            );
        }
        match *mode {
            ContextMode::Auto => {
                if recovery.is_some() && resume_id.is_none() {
                    return Err(
                        "context recovery is only valid for an uncertain auto resume".to_string(),
                    );
                }
            }
            ContextMode::Seed => {
                if session_id.is_none() || resume_id.is_some() {
                    return Err("seed context mode requires session_id, not resume_id".to_string());
                }
                if recovery.is_some() {
                    return Err("context recovery is only valid for auto resume".to_string());
                }
            }
            ContextMode::Delta => {
                if resume_id.is_none() || session_id.is_some() {
                    return Err("delta context mode requires resume_id, not session_id".to_string());
                }
                if recovery.is_some() {
                    return Err("context recovery is only valid for auto resume".to_string());
                }
            }
            ContextMode::Snapshot => unreachable!(),
        }
        Ok(())
    }

    pub(crate) fn grok_denies(&self) -> &[String] {
        &self.grok_denies
    }

    pub(crate) fn grok_allows(&self) -> &[String] {
        &self.grok_allows
    }

    pub(crate) fn grok_subagents_disabled(&self) -> bool {
        self.grok_subagent_mode == GrokSubagentMode::Off
    }

    #[allow(dead_code)]
    pub(crate) fn grok_subagents_guarded(&self) -> bool {
        self.grok_subagent_mode == GrokSubagentMode::Guarded
    }

    pub(crate) fn guarded_subagents(&self) -> Option<&GuardedSubagentPolicy> {
        self.guarded_subagents.as_ref()
    }

    pub(crate) fn require_grok_child_lanes_ready(&self) -> Result<(), String> {
        let guarded = self.guarded_subagents.as_ref().ok_or_else(|| {
            "Grok child lanes require PARLEY_GROK_SUBAGENT_MODE=guarded".to_string()
        })?;
        if !guarded.typed_roles_ready {
            return Err("native Grok child lanes are disabled: typed subagent roles have not passed the locked-profile capability canary; use zero grok_child lanes until the profile is revalidated".to_string());
        }
        Ok(())
    }

    pub(crate) fn requires_handoff_footer(&self) -> bool {
        self.grok_require_handoff_footer
    }

    pub(crate) fn apply_handoff_contract(&self, harness: &str, prompt: &mut String) {
        if harness != "grok" || !self.grok_require_handoff_footer {
            return;
        }
        let Some(query) = &self.grok_health_query_command else {
            return;
        };
        prompt.push_str("\n\nTWO CHAIRS LOCKED RESPONSE CONTRACT\n");
        prompt.push_str("The current user-authorized request explicitly permits executing only the exact peer-health query below.\n");
        prompt.push_str("Keep the normal final response self-contained. Immediately before finishing, invoke exactly this read-only command with no arguments, wrappers, redirects, or chaining:\n");
        prompt.push_str(query);
        prompt.push('\n');
        prompt.push_str("End the response with this exact field block, copying only current evidence from the query JSON:\n");
        prompt.push_str("TWO_CHAIRS_HANDOFF\npeer: codex\nevidence_class: <usage_sample|quota_exhausted|unavailable>\nincident_id: <id|none>\nas_of_ms: <integer|unknown>\n");
        prompt.push_str("Include event_id and exchange_id only when present. End with: continuity: not_authorized\n");
        prompt.push_str("Stale or unavailable evidence never means the peer is down. Do not retry the query or authorize continuation.\n");
    }

    pub(crate) fn resolve_handoff_mode(
        &self,
        job_mode: JobMode,
        requested: Option<HandoffMode>,
    ) -> Result<HandoffMode, String> {
        let locked = self.handoff.is_some();
        let expected = if locked && matches!(job_mode, JobMode::Write | JobMode::Review) {
            HandoffMode::Required
        } else {
            HandoffMode::None
        };
        if let Some(requested) = requested {
            if locked && requested != expected {
                return Err(format!(
                    "handoff_mode is locked to {} for job_mode={}",
                    expected.as_str(),
                    job_mode.as_str()
                ));
            }
            if requested == HandoffMode::Required && !locked {
                return Err(
                    "handoff_mode=required requires a locked handoff runtime profile".to_string(),
                );
            }
            return Ok(requested);
        }
        Ok(expected)
    }

    pub(crate) fn handoff(&self) -> Option<&HandoffPolicy> {
        self.handoff.as_ref()
    }

    pub(crate) fn require_codex_handoff_caller(&self) -> Result<(), String> {
        if self.handoff.is_none() {
            return Err("acknowledged handoffs are not enabled for this profile".to_string());
        }
        match env::var("PARLEY_CALLER") {
            Ok(caller) if caller.eq_ignore_ascii_case("codex") => Ok(()),
            _ => Err("handoff receipt requires the locked Codex caller".to_string()),
        }
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

fn normalize_reasoning_effort(value: &str) -> Result<String, String> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "low" | "medium" | "high" | "xhigh" => Ok(normalized),
        _ => Err(format!(
            "Grok reasoning effort must be low, medium, high, or xhigh, got {value}"
        )),
    }
}

fn grok_passthrough_control(args: &[String]) -> Option<&str> {
    args.iter().find_map(|arg| {
        matches!(
            arg.as_str(),
            "-m" | "--model"
                | "--effort"
                | "--reasoning-effort"
                | "--agents"
                | "--agent"
                | "--tools"
                | "--disallowed-tools"
                | "--sandbox"
                | "--no-subagents"
        )
        .then_some(arg.as_str())
        .or_else(|| {
            [
                "--model=",
                "--effort=",
                "--reasoning-effort=",
                "--agents=",
                "--agent=",
                "--tools=",
                "--disallowed-tools=",
                "--sandbox=",
            ]
            .iter()
            .any(|prefix| arg.starts_with(prefix))
            .then_some(arg.as_str())
        })
    })
}

fn validate_grok_version(expected: &str) -> Result<(), String> {
    let output = Command::new("grok")
        .arg("--version")
        .output()
        .map_err(|error| format!("failed to run grok --version: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "grok --version failed with status {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    let actual = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version_matches(expected, &actual) {
        Ok(())
    } else {
        Err(format!(
            "Grok CLI version is locked to {expected}; found {actual}"
        ))
    }
}

fn version_matches(expected: &str, actual: &str) -> bool {
    let expected = expected.trim();
    let actual = actual.trim();
    actual == expected
        || actual
            .strip_prefix(expected)
            .is_some_and(|suffix| suffix.starts_with(char::is_whitespace))
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

fn add_health_tool_rules(
    query: &Path,
    denies: &mut Vec<String>,
) -> Result<(Vec<String>, String), String> {
    let command = powershell_direct_command(query)?;
    let allows = vec![format!("Bash({command})")];
    if let Some(parent) = query.parent() {
        for writer in ["parley-health-supervisor.exe", "parley-health-hook.exe"] {
            denies.push(format!("Bash(*{}*)", parent.join(writer).to_string_lossy()));
        }
    }
    denies.push("Bash(*parley-health-supervisor.exe*)".to_string());
    denies.push("Bash(*parley-health-hook.exe*)".to_string());
    Ok((allows, command))
}

fn add_lane_hook_tool_rules(hook: &Path, denies: &mut Vec<String>) {
    denies.push(format!("Bash(*{}*)", hook.to_string_lossy()));
    denies.push("Bash(*parley-lane-hook.exe*)".to_string());
}

fn add_handoff_hook_tool_rules(hook: &Path, denies: &mut Vec<String>) {
    denies.push(format!("Bash(*{}*)", hook.to_string_lossy()));
    denies.push("Bash(*parley-handoff-hook.exe*)".to_string());
}

fn required_absolute_dir_value(name: &str) -> Result<PathBuf, String> {
    let value = env::var_os(name).ok_or_else(|| format!("{name} is required"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(format!("{name} must be absolute"));
    }
    if let Some(existing) = path.ancestors().find(|candidate| candidate.exists()) {
        refuse_reparse_chain(existing).map_err(|error| error.to_string())?;
    }
    Ok(path)
}

fn required_executable(name: &str) -> Result<PathBuf, String> {
    let value = env::var_os(name).ok_or_else(|| format!("{name} is required"))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() || !path.is_file() {
        return Err(format!(
            "{name} is not an existing absolute file: {}",
            path.display()
        ));
    }
    refuse_reparse_chain(&path).map_err(|error| error.to_string())?;
    fs::canonicalize(&path)
        .map_err(|error| format!("canonicalize {name} {}: {error}", path.display()))
}

fn required_exact_u64(name: &str, expected: u64) -> Result<u64, String> {
    let value = env_nonempty(name).ok_or_else(|| format!("{name} is required"))?;
    let parsed = value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be an integer, got {value}"))?;
    if parsed != expected {
        return Err(format!("{name} must be {expected}, got {parsed}"));
    }
    Ok(parsed)
}

fn powershell_direct_command(path: &Path) -> Result<String, String> {
    let direct = command_path_text(path);
    if is_matcher_safe_command(&direct) {
        return Ok(direct);
    }

    #[cfg(windows)]
    let text = {
        let parent = path
            .parent()
            .ok_or_else(|| "PARLEY_GROK_HEALTH_QUERY_EXE has no parent directory".to_string())?;
        let file_name = path
            .file_name()
            .ok_or_else(|| "PARLEY_GROK_HEALTH_QUERY_EXE has no executable filename".to_string())?;
        let candidate = windows_short_path(parent)?.join(file_name);
        let resolved = fs::canonicalize(&candidate).map_err(|error| {
            format!(
                "resolve matcher-safe PARLEY_GROK_HEALTH_QUERY_EXE {}: {error}",
                candidate.display()
            )
        })?;
        if !paths_equal(&resolved, path) {
            return Err(
                "matcher-safe PARLEY_GROK_HEALTH_QUERY_EXE does not resolve to the configured file"
                    .to_string(),
            );
        }
        command_path_text(&candidate)
    };
    #[cfg(not(windows))]
    let text = direct;

    if text.chars().any(|ch| matches!(ch, '\r' | '\n' | '\0')) {
        return Err(
            "PARLEY_GROK_HEALTH_QUERY_EXE contains an unsafe control character".to_string(),
        );
    }
    if !is_matcher_safe_command(&text) {
        return Err(
            "PARLEY_GROK_HEALTH_QUERY_EXE has no matcher-safe path without spaces or shell syntax"
                .to_string(),
        );
    }
    Ok(text)
}

fn command_path_text(path: &Path) -> String {
    let original = path.to_string_lossy();
    if let Some(rest) = original.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = original.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        original.into_owned()
    }
}

fn is_matcher_safe_command(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '\\' | '/' | ':' | '.' | '_' | '-' | '~')
        })
}

#[cfg(windows)]
fn windows_short_path(path: &Path) -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::ptr;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetShortPathNameW(long_path: *const u16, short_path: *mut u16, buffer_len: u32) -> u32;
    }

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let required = unsafe { GetShortPathNameW(wide.as_ptr(), ptr::null_mut(), 0) };
    if required == 0 {
        return Err(format!(
            "resolve short PARLEY_GROK_HEALTH_QUERY_EXE parent {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    let mut output = vec![0_u16; required as usize];
    let written = unsafe { GetShortPathNameW(wide.as_ptr(), output.as_mut_ptr(), required) };
    if written == 0 || written >= required {
        return Err(format!(
            "resolve short PARLEY_GROK_HEALTH_QUERY_EXE parent {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    output.truncate(written as usize);
    Ok(PathBuf::from(OsString::from_wide(&output)))
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
        let mut model = None;
        let mut effort = None;
        let mut mode = Some("auto".to_string());
        let mut turns = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: true,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903"),
                resume_id: None,
                passthrough: &[],
            })
            .is_err());

        let mut mode = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903"),
                resume_id: Some("01a06582-d66e-7811-b0c9-0b0266e17904"),
                passthrough: &[],
            })
            .unwrap_err()
            .contains("mutually exclusive"));

        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: Some("not-a-uuid"),
                resume_id: None,
                passthrough: &[],
            })
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
            grok_locked_model: Some("grok-4.6".to_string()),
            grok_locked_reasoning_effort: Some("xhigh".to_string()),
            grok_locked_permission_mode: Some("auto".to_string()),
            grok_require_session_id: true,
            grok_max_turns: Some(30),
            grok_denies: hardened_grok_denies(),
            ..RuntimePolicy::default()
        };
        let id = Some("01a06582-d66e-7811-b0c9-0b0266e17903".to_string());
        let mut model = None;
        let mut effort = None;
        let mut mode = None;
        let mut turns = None;
        policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: id.as_deref(),
                resume_id: None,
                passthrough: &[],
            })
            .unwrap();
        assert_eq!(model.as_deref(), Some("grok-4.6"));
        assert_eq!(effort.as_deref(), Some("xhigh"));
        assert_eq!(mode.as_deref(), Some("auto"));
        assert_eq!(turns.as_deref(), Some("30"));

        let mut mode = Some("auto".to_string());
        let mut turns = Some("12".to_string());
        policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: id.as_deref(),
                resume_id: None,
                passthrough: &[],
            })
            .unwrap();
        assert_eq!(turns.as_deref(), Some("12"));

        let mut turns = Some("31".to_string());
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: id.as_deref(),
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("capped at 30"));

        let mut wrong_mode = Some("default".to_string());
        let mut turns = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut wrong_mode,
                max_turns: &mut turns,
                session_id: id.as_deref(),
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("locked to auto"));

        let mut mode = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: None,
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("require session_id or resume_id"));
    }

    #[test]
    fn locked_context_policy_injects_omissions_and_rejects_conflicts() {
        let policy = RuntimePolicy {
            context_locked_source: Some("codex".to_string()),
            context_locked_mode: Some(ContextMode::Auto),
            context_require_explicit_session: true,
            ..RuntimePolicy::default()
        };
        let mut source = String::new();
        let mut mode = ContextMode::Snapshot;
        policy
            .apply_context(ContextPolicyRequest {
                target_harness: "grok",
                source_harness: &mut source,
                source_session: "codex-session",
                mode: &mut mode,
                mode_explicit: false,
                recovery: None,
                max_context_chars: None,
                session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903"),
                resume_id: None,
            })
            .unwrap();
        assert_eq!(source, "codex");
        assert_eq!(mode, ContextMode::Auto);

        let mut source = "claude".to_string();
        let mut mode = ContextMode::Auto;
        assert!(policy
            .apply_context(ContextPolicyRequest {
                target_harness: "grok",
                source_harness: &mut source,
                source_session: "codex-session",
                mode: &mut mode,
                mode_explicit: true,
                recovery: None,
                max_context_chars: None,
                session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903"),
                resume_id: None,
            })
            .unwrap_err()
            .contains("source is locked"));

        let mut source = "codex".to_string();
        let mut mode = ContextMode::Snapshot;
        assert!(policy
            .apply_context(ContextPolicyRequest {
                target_harness: "grok",
                source_harness: &mut source,
                source_session: "codex-session",
                mode: &mut mode,
                mode_explicit: true,
                recovery: None,
                max_context_chars: None,
                session_id: Some("01a06582-d66e-7811-b0c9-0b0266e17903"),
                resume_id: None,
            })
            .unwrap_err()
            .contains("mode is locked"));
    }

    #[test]
    fn stateful_context_policy_enforces_the_session_matrix() {
        let policy = RuntimePolicy::default();
        let cases = [
            (
                ContextMode::Seed,
                None,
                Some("resume"),
                None,
                "requires session_id",
            ),
            (
                ContextMode::Delta,
                Some("new"),
                None,
                None,
                "requires resume_id",
            ),
            (
                ContextMode::Auto,
                Some("new"),
                None,
                Some(ContextRecovery::Replay),
                "uncertain auto resume",
            ),
        ];
        for (requested_mode, session_id, resume_id, recovery, expected) in cases {
            let mut source = "codex".to_string();
            let mut mode = requested_mode;
            let error = policy
                .apply_context(ContextPolicyRequest {
                    target_harness: "grok",
                    source_harness: &mut source,
                    source_session: "source-session",
                    mode: &mut mode,
                    mode_explicit: true,
                    recovery,
                    max_context_chars: None,
                    session_id,
                    resume_id,
                })
                .unwrap_err();
            assert!(error.contains(expected), "{error}");
        }

        let mut source = "codex".to_string();
        let mut mode = ContextMode::Auto;
        let error = policy
            .apply_context(ContextPolicyRequest {
                target_harness: "claude",
                source_harness: &mut source,
                source_session: "source-session",
                mode: &mut mode,
                mode_explicit: true,
                recovery: None,
                max_context_chars: None,
                session_id: Some("new"),
                resume_id: None,
            })
            .unwrap_err();
        assert!(error.contains("only supported for Grok"));

        let mut source = "codex".to_string();
        let mut mode = ContextMode::Auto;
        let error = policy
            .apply_context(ContextPolicyRequest {
                target_harness: "grok",
                source_harness: &mut source,
                source_session: "source-session",
                mode: &mut mode,
                mode_explicit: true,
                recovery: None,
                max_context_chars: Some(1_000),
                session_id: Some("new"),
                resume_id: None,
            })
            .unwrap_err();
        assert!(error.contains("only to snapshot"));
    }

    #[test]
    fn health_query_rules_allow_only_query_and_deny_writer_images() {
        let query = PathBuf::from(r"C:\PARLEY~1\health\parley-health-query.exe");
        let mut denies = hardened_grok_denies();
        let (allows, command) = add_health_tool_rules(&query, &mut denies).unwrap();

        assert_eq!(command, query.to_string_lossy());
        assert_eq!(allows, vec![format!("Bash({command})")]);
        assert!(!allows[0].contains('*'));
        assert!(!allows[0].contains("& \""));
        assert!(!allows[0].contains(' '));
        for writer in ["parley-health-supervisor.exe", "parley-health-hook.exe"] {
            assert!(denies.iter().any(|rule| rule.contains(writer)));
            assert!(denies.iter().any(|rule| {
                rule.contains(
                    &query
                        .parent()
                        .unwrap()
                        .join(writer)
                        .to_string_lossy()
                        .to_string(),
                )
            }));
        }
        assert!(!allows.iter().any(|rule| {
            rule.contains("parley-health-supervisor") || rule.contains("parley-health-hook")
        }));
    }

    #[test]
    fn lane_hook_image_is_immutably_denied_by_path_and_name() {
        let hook = PathBuf::from(r"C:\Program Files\Parley\lanes\parley-lane-hook.exe");
        let mut denies = hardened_grok_denies();
        add_lane_hook_tool_rules(&hook, &mut denies);
        assert!(denies
            .iter()
            .any(|rule| rule.contains(&hook.to_string_lossy().to_string())));
        assert!(denies
            .iter()
            .any(|rule| rule.contains("parley-lane-hook.exe")));
    }

    #[test]
    fn native_child_lanes_require_an_explicit_capability_canary() {
        let mut policy = RuntimePolicy::default();
        assert!(policy
            .require_grok_child_lanes_ready()
            .unwrap_err()
            .contains("require PARLEY_GROK_SUBAGENT_MODE=guarded"));

        policy.guarded_subagents = Some(GuardedSubagentPolicy {
            lane_state_dir: PathBuf::from(r"C:\state\lanes"),
            hook_exe: PathBuf::from(r"C:\Program Files\Parley\parley-lane-hook.exe"),
            max_writers: 2,
            model: "grok-4.7".to_string(),
            reasoning_effort: "xhigh".to_string(),
            typed_roles_ready: false,
        });
        assert!(policy
            .require_grok_child_lanes_ready()
            .unwrap_err()
            .contains("typed subagent roles have not passed"));

        policy.guarded_subagents.as_mut().unwrap().typed_roles_ready = true;
        assert!(policy.require_grok_child_lanes_ready().is_ok());
    }

    #[test]
    fn locked_handoff_contract_names_exact_query_and_never_applies_to_other_harnesses() {
        let query = PathBuf::from(r"C:\PARLEY~1\parley-health-query.exe");
        let command = powershell_direct_command(&query).unwrap();
        let policy = RuntimePolicy {
            grok_health_query_command: Some(command.clone()),
            grok_require_handoff_footer: true,
            ..RuntimePolicy::default()
        };
        let mut prompt = "task".to_string();
        policy.apply_handoff_contract("grok", &mut prompt);
        assert!(prompt.contains(&command));
        assert!(!prompt.contains("& \""));
        assert!(prompt.contains("current user-authorized request explicitly permits"));
        assert!(prompt.contains("continuity: not_authorized"));
        assert!(prompt.contains("Stale or unavailable evidence never means the peer is down"));

        let mut other = "task".to_string();
        policy.apply_handoff_contract("codex", &mut other);
        assert_eq!(other, "task");
    }

    #[test]
    fn acknowledged_handoff_mode_is_locked_by_job_kind() {
        let helper = PathBuf::from(r"C:\Program Files\Parley\helper.exe");
        let policy = RuntimePolicy {
            handoff: Some(HandoffPolicy {
                state_dir: PathBuf::from(r"C:\Users\test\AppData\Local\Parley\handoffs"),
                peer_peek_exe: helper.clone(),
                wait_exe: helper.clone(),
                alert_exe: helper.clone(),
                hook_exe: helper,
                peer_peek_command: "peer-peek".to_string(),
                wait_command: "handoff-wait".to_string(),
                alert_command: "peer-alert".to_string(),
                wait_slice_secs: 300,
                execution_ceiling_secs: 21_600,
                acknowledgement_ceiling_secs: 21_600,
                excerpt_chars: 4_096,
                max_turns: 128,
            }),
            ..RuntimePolicy::default()
        };
        for mode in [JobMode::Write, JobMode::Review] {
            assert_eq!(
                policy.resolve_handoff_mode(mode, None).unwrap(),
                HandoffMode::Required
            );
            assert_eq!(
                policy
                    .resolve_handoff_mode(mode, Some(HandoffMode::Required))
                    .unwrap(),
                HandoffMode::Required
            );
            assert!(policy
                .resolve_handoff_mode(mode, Some(HandoffMode::None))
                .is_err());
        }
        assert_eq!(
            policy.resolve_handoff_mode(JobMode::Probe, None).unwrap(),
            HandoffMode::None
        );
        assert!(policy
            .resolve_handoff_mode(JobMode::Probe, Some(HandoffMode::Required))
            .is_err());
        assert!(RuntimePolicy::default()
            .resolve_handoff_mode(JobMode::Write, Some(HandoffMode::Required))
            .is_err());
    }

    #[test]
    fn locked_grok_policy_rejects_model_effort_and_passthrough_overrides() {
        let policy = RuntimePolicy {
            grok_locked_model: Some("grok-4.6".to_string()),
            grok_locked_reasoning_effort: Some("xhigh".to_string()),
            ..RuntimePolicy::default()
        };
        let mut model = Some("grok-4.5".to_string());
        let mut effort = Some("xhigh".to_string());
        let mut mode = None;
        let mut turns = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: None,
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("model is locked"));

        let mut model = Some("grok-4.6".to_string());
        let mut effort = Some("HIGH".to_string());
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: None,
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("reasoning effort is locked"));

        for passthrough in [
            vec!["--model".to_string(), "grok-4.5".to_string()],
            vec!["-m".to_string(), "grok-4.5".to_string()],
            vec!["--effort=low".to_string()],
            vec!["--reasoning-effort".to_string(), "high".to_string()],
            vec!["--agents={}".to_string()],
            vec!["--agent".to_string(), "writer".to_string()],
            vec!["--tools".to_string(), "Edit".to_string()],
            vec!["--disallowed-tools=Bash".to_string()],
            vec!["--sandbox".to_string(), "none".to_string()],
            vec!["--no-subagents".to_string()],
        ] {
            let mut model = Some("grok-4.6".to_string());
            let mut effort = Some("xhigh".to_string());
            assert!(policy
                .apply_request(PolicyRequest {
                    harness: "grok",
                    yolo: false,
                    model: &mut model,
                    reasoning_effort: &mut effort,
                    permission_mode: &mut mode,
                    max_turns: &mut turns,
                    session_id: None,
                    resume_id: None,
                    passthrough: &passthrough,
                })
                .unwrap_err()
                .contains("not passthrough"));
        }
    }

    #[test]
    fn locked_version_accepts_channel_suffix_only() {
        let expected = "grok 1.0.40 (eb1a2256660d)";
        assert!(version_matches(expected, expected));
        assert!(version_matches(
            expected,
            "grok 1.0.40 (eb1a2256660d) [stable]"
        ));
        assert!(!version_matches(expected, "grok 1.0.41 (different)"));
        assert!(!version_matches(
            expected,
            "grok 1.0.40 (eb1a2256660d)-evil"
        ));
    }

    #[test]
    fn validates_reasoning_effort_and_rejects_other_harnesses() {
        assert_eq!(normalize_reasoning_effort(" XHIGH ").unwrap(), "xhigh");
        assert!(normalize_reasoning_effort("maximum").is_err());

        let policy = RuntimePolicy::default();
        let mut model = None;
        let mut effort = Some("high".to_string());
        let mut mode = None;
        let mut turns = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "claude",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: None,
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("only supported for Grok"));
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

        let mut model = None;
        let mut effort = None;
        let mut mode = Some("bypassPermissions".to_string());
        let mut turns = None;
        assert!(policy
            .apply_request(PolicyRequest {
                harness: "grok",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: None,
                resume_id: None,
                passthrough: &[],
            })
            .unwrap_err()
            .contains("permission bypass is disabled"));
    }

    #[test]
    fn legacy_harnesses_keep_non_uuid_session_semantics() {
        let policy = RuntimePolicy::default();
        let mut model = None;
        let mut effort = None;
        let mut mode = None;
        let mut turns = None;
        policy
            .apply_request(PolicyRequest {
                harness: "claude",
                yolo: false,
                model: &mut model,
                reasoning_effort: &mut effort,
                permission_mode: &mut mode,
                max_turns: &mut turns,
                session_id: Some("legacy-session-name"),
                resume_id: Some("legacy-resume-name"),
                passthrough: &[],
            })
            .unwrap();
    }
}
