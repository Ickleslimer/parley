use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::fsutil;
use crate::paths::HealthPaths;
use crate::schema::HealthError;
use crate::scope::{ScopeFile, ScopeUpdate};

pub const HOOK_FILE_NAME: &str = "two-chairs-peer-health.json";

pub fn configure_r3(
    health_paths: &HealthPaths,
    git_common_dir: &Path,
    main_root: &Path,
    install_dir: &Path,
    grok_home: &Path,
) -> Result<(), HealthError> {
    require_directory(git_common_dir, "Git common directory")?;
    require_directory(main_root, "R3 main root")?;
    let query = install_dir.join("parley-health-query.exe");
    let supervisor = install_dir.join("parley-health-supervisor.exe");
    let hook = install_dir.join("parley-health-hook.exe");
    require_file(&query, "health query")?;
    require_file(&supervisor, "health supervisor")?;
    require_file(&hook, "health hook")?;

    health_paths.ensure()?;
    let mut scope = ScopeFile::load_or_empty(&health_paths.scope());
    scope.apply_update(ScopeUpdate {
        git_common_dir: Some(git_common_dir.to_string_lossy().to_string()),
        main_root: Some(main_root.to_string_lossy().to_string()),
        worktree_roots: Vec::new(),
        query_roots: Vec::new(),
        query_path: Some(query.to_string_lossy().to_string()),
        supervisor_path: Some(supervisor.to_string_lossy().to_string()),
        hook_path: Some(hook.to_string_lossy().to_string()),
    });
    scope.refresh_cached_roots();
    scope.save(&health_paths.scope())?;
    install_hooks(grok_home, &hook)
}

pub fn allow_query_root(health_paths: &HealthPaths, root: &Path) -> Result<(), HealthError> {
    require_directory(root, "query-only root")?;
    let mut scope = ScopeFile::load(&health_paths.scope())?;
    scope.apply_update(ScopeUpdate {
        query_roots: vec![root.to_string_lossy().to_string()],
        ..ScopeUpdate::default()
    });
    scope.save(&health_paths.scope())
}

pub fn refresh_installation(
    health_paths: &HealthPaths,
    install_dir: &Path,
    grok_home: &Path,
) -> Result<(), HealthError> {
    let scope = ScopeFile::load(&health_paths.scope())?;
    let common = scope
        .git_common_dir
        .as_deref()
        .ok_or_else(|| HealthError::msg("health scope has no Git common directory"))?;
    if scope.main_root.trim().is_empty() {
        return Err(HealthError::msg("health scope has no R3 main root"));
    }
    configure_r3(
        health_paths,
        Path::new(common),
        Path::new(&scope.main_root),
        install_dir,
        grok_home,
    )
}

pub fn install_hooks(grok_home: &Path, hook_exe: &Path) -> Result<(), HealthError> {
    require_file(hook_exe, "health hook")?;
    let hooks_dir = grok_home.join("hooks");
    fs::create_dir_all(&hooks_dir).map_err(|error| {
        HealthError::msg(format!(
            "create Grok hooks {}: {error}",
            hooks_dir.display()
        ))
    })?;
    let path = hooks_dir.join(HOOK_FILE_NAME);
    if path.exists() {
        let existing = read_json(&path)?;
        if !is_two_chairs_document(&existing) {
            return Err(HealthError::msg(format!(
                "refusing to replace unrelated Grok hook file {}",
                path.display()
            )));
        }
    }
    let bytes = serde_json::to_vec_pretty(&hook_document(hook_exe))?;
    fsutil::atomic_write(&path, &bytes)
}

pub fn remove_hooks(grok_home: &Path) -> Result<bool, HealthError> {
    let path = grok_home.join("hooks").join(HOOK_FILE_NAME);
    if !path.exists() {
        return Ok(false);
    }
    let mut existing = read_json(&path)?;
    if !remove_managed_entries(&mut existing) {
        return Ok(false);
    }
    if only_empty_hooks_remain(&existing) {
        fs::remove_file(&path)
            .map_err(|error| HealthError::msg(format!("remove {}: {error}", path.display())))?;
    } else {
        let bytes = serde_json::to_vec_pretty(&existing)?;
        fsutil::atomic_write(&path, &bytes)?;
    }
    Ok(true)
}

pub fn grok_home() -> PathBuf {
    if let Some(path) = env::var_os("GROK_HOME") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("USERPROFILE") {
        return PathBuf::from(path).join(".grok");
    }
    PathBuf::from(r"C:\Users\Default\.grok")
}

pub fn hook_document(hook_exe: &Path) -> Value {
    let command = format!("\"{}\"", hook_exe.to_string_lossy());
    json!({
        "hooks": {
            "PreToolUse": [{
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": command, "timeout": 5}]
            }],
            "StopFailure": [{
                "hooks": [{"type": "command", "command": command, "timeout": 5}]
            }]
        }
    })
}

fn is_two_chairs_document(value: &Value) -> bool {
    let Some(hooks) = value.get("hooks").and_then(Value::as_object) else {
        return false;
    };
    if hooks.len() != 2 || !hooks.contains_key("PreToolUse") || !hooks.contains_key("StopFailure") {
        return false;
    }
    event_has_one_managed_hook(hooks.get("PreToolUse"), true)
        && event_has_one_managed_hook(hooks.get("StopFailure"), false)
}

fn event_has_one_managed_hook(value: Option<&Value>, pretool: bool) -> bool {
    let Some(groups) = value.and_then(Value::as_array) else {
        return false;
    };
    if groups.len() != 1 {
        return false;
    }
    let Some(group) = groups[0].as_object() else {
        return false;
    };
    if pretool && group.get("matcher").and_then(Value::as_str) != Some("Bash") {
        return false;
    }
    if !pretool && group.contains_key("matcher") {
        return false;
    }
    let Some(handlers) = group.get("hooks").and_then(Value::as_array) else {
        return false;
    };
    if handlers.len() != 1 {
        return false;
    }
    let Some(handler) = handlers[0].as_object() else {
        return false;
    };
    handler.get("type").and_then(Value::as_str) == Some("command")
        && handler.get("timeout").and_then(Value::as_u64) == Some(5)
        && handler
            .get("command")
            .and_then(Value::as_str)
            .map(command_targets_hook)
            .unwrap_or(false)
}

fn command_targets_hook(command: &str) -> bool {
    let command = command.trim().trim_matches('"').replace('/', "\\");
    command
        .rsplit('\\')
        .next()
        .map(|name| name.eq_ignore_ascii_case("parley-health-hook.exe"))
        .unwrap_or(false)
}

fn remove_managed_entries(value: &mut Value) -> bool {
    let Some(hooks) = value.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut removed = false;
    let mut empty_events = Vec::new();
    for event in ["PreToolUse", "StopFailure"] {
        let Some(groups) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
            continue;
        };
        groups.retain_mut(|group| {
            let Some(object) = group.as_object_mut() else {
                return true;
            };
            let Some(handlers) = object.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = handlers.len();
            handlers.retain(|handler| !is_managed_handler(handler));
            removed |= handlers.len() != before;
            !(handlers.is_empty() && object.keys().all(|key| key == "hooks" || key == "matcher"))
        });
        if groups.is_empty() {
            empty_events.push(event);
        }
    }
    for event in empty_events {
        hooks.remove(event);
    }
    removed
}

fn is_managed_handler(value: &Value) -> bool {
    let Some(handler) = value.as_object() else {
        return false;
    };
    handler.get("type").and_then(Value::as_str) == Some("command")
        && handler
            .get("command")
            .and_then(Value::as_str)
            .map(command_targets_hook)
            .unwrap_or(false)
}

fn only_empty_hooks_remain(value: &Value) -> bool {
    let Some(root) = value.as_object() else {
        return false;
    };
    root.len() == 1
        && root
            .get("hooks")
            .and_then(Value::as_object)
            .map(|hooks| hooks.is_empty())
            .unwrap_or(false)
}

fn read_json(path: &Path) -> Result<Value, HealthError> {
    let bytes = fsutil::read_bounded(path, 256 * 1024)?;
    if bytes.clipped {
        return Err(HealthError::msg("Grok hook file exceeds bound"));
    }
    Ok(serde_json::from_slice(fsutil::strip_bom(&bytes.bytes))?)
}

fn require_directory(path: &Path, label: &str) -> Result<(), HealthError> {
    if path.is_absolute() && path.is_dir() {
        Ok(())
    } else {
        Err(HealthError::msg(format!(
            "{label} must be an existing absolute directory: {}",
            path.display()
        )))
    }
}

fn require_file(path: &Path, label: &str) -> Result<(), HealthError> {
    if path.is_absolute() && path.is_file() {
        Ok(())
    } else {
        Err(HealthError::msg(format!(
            "{label} must be an existing absolute file: {}",
            path.display()
        )))
    }
}
