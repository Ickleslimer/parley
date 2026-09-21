mod common;

use parley_health::integration::{
    allow_query_root, configure_r3, hook_document, install_hooks, remove_hooks, HOOK_FILE_NAME,
};
use parley_health::scope::ScopeFile;

fn fake_install(home: &common::TempHome) -> std::path::PathBuf {
    let install = home.paths.root.join("install");
    std::fs::create_dir_all(&install).unwrap();
    for binary in [
        "parley-health-query.exe",
        "parley-health-supervisor.exe",
        "parley-health-hook.exe",
    ] {
        std::fs::write(install.join(binary), binary.as_bytes()).unwrap();
    }
    install
}

#[test]
fn installs_exactly_two_dedicated_hooks_and_preserves_unrelated_files() {
    let home = common::TempHome::new("hook-install");
    let install = fake_install(&home);
    let grok_home = home.paths.root.join("grok");
    let hooks = grok_home.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let unrelated = hooks.join("unrelated.json");
    std::fs::write(&unrelated, r#"{"hooks":{"SessionStart":[]}}"#).unwrap();

    install_hooks(&grok_home, &install.join("parley-health-hook.exe")).unwrap();
    let installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(hooks.join(HOOK_FILE_NAME)).unwrap()).unwrap();
    assert_eq!(installed["hooks"].as_object().unwrap().len(), 2);
    assert_eq!(
        installed["hooks"]["PreToolUse"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        installed["hooks"]["StopFailure"].as_array().unwrap().len(),
        1
    );
    assert!(unrelated.exists());

    assert!(remove_hooks(&grok_home).unwrap());
    assert!(unrelated.exists());
    assert!(!hooks.join(HOOK_FILE_NAME).exists());
}

#[test]
fn refuses_to_replace_or_remove_a_modified_reserved_hook_file() {
    let home = common::TempHome::new("hook-refuse");
    let install = fake_install(&home);
    let grok_home = home.paths.root.join("grok");
    let hooks = grok_home.join("hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    let reserved = hooks.join(HOOK_FILE_NAME);
    std::fs::write(&reserved, r#"{"hooks":{"SessionStart":[]}}"#).unwrap();
    assert!(install_hooks(&grok_home, &install.join("parley-health-hook.exe")).is_err());
    assert!(remove_hooks(&grok_home).is_err());
    assert!(reserved.exists());
}

#[test]
fn configure_caches_r3_identity_and_installed_executable_identities() {
    let home = common::TempHome::new("configure-r3");
    let install = fake_install(&home);
    let main = home.paths.root.join("r3");
    let common_dir = main.join(".git");
    std::fs::create_dir_all(&common_dir).unwrap();
    let grok_home = home.paths.root.join("grok");

    configure_r3(&home.paths, &common_dir, &main, &install, &grok_home).unwrap();
    let scope = ScopeFile::load(&home.paths.scope()).unwrap();
    assert!(scope.common_dir_identity_is_current());
    assert!(scope.contains_path(&main.to_string_lossy()));
    assert!(scope.executables.query.is_some());
    assert!(scope.executables.supervisor.is_some());
    assert!(scope.executables.hook.is_some());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &std::fs::read(grok_home.join("hooks").join(HOOK_FILE_NAME)).unwrap()
        )
        .unwrap(),
        hook_document(&install.join("parley-health-hook.exe"))
    );
}

#[test]
fn query_only_root_is_cached_without_expanding_r3_failure_scope() {
    let home = common::TempHome::new("query-root");
    let install = fake_install(&home);
    let main = home.paths.root.join("r3");
    let common_dir = main.join(".git");
    let query_root = home.paths.root.join("parley-dev");
    std::fs::create_dir_all(&common_dir).unwrap();
    std::fs::create_dir_all(&query_root).unwrap();
    let grok_home = home.paths.root.join("grok");

    configure_r3(&home.paths, &common_dir, &main, &install, &grok_home).unwrap();
    allow_query_root(&home.paths, &query_root).unwrap();
    let scope = ScopeFile::load(&home.paths.scope()).unwrap();
    let query_root = query_root.to_string_lossy();
    assert!(scope.in_query_scope(Some(&query_root), Some(&query_root)));
    assert!(!scope.in_scope(Some(&query_root), Some(&query_root)));
}
