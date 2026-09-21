use parley_health::command::{analyze_command, HealthBinary};
use parley_health::scope::{
    canonical_contains, normalize_windows_path, paths_equivalent, query_matches_installed,
    worktrees_from_common_dir, ExecutableIdentity,
};

mod common;

#[test]
fn component_boundary_matching_rejects_sibling_prefixes() {
    assert!(canonical_contains(r"C:\proj", r"C:\proj"));
    assert!(canonical_contains(r"C:\proj", r"C:\proj\worktree"));
    assert!(canonical_contains(r"C:\Proj", r"c:\proj\src"));
    assert!(!canonical_contains(r"C:\proj", r"C:\proj-evil"));
    assert!(!canonical_contains(r"C:\proj", r"C:\project"));
    assert!(!canonical_contains(r"C:\proj", r"C:\pro"));
}

#[test]
fn windows_path_normalization_is_case_and_slash_insensitive() {
    assert_eq!(
        normalize_windows_path(r"C:/Repo/"),
        normalize_windows_path(r"c:\repo")
    );
    assert!(paths_equivalent(r"\\?\C:\Work\Root", r"c:\work\root"));
}

#[test]
fn query_identity_helper_matches_path_when_identity_absent() {
    let installed = ExecutableIdentity {
        path: normalize_windows_path(r"C:\Parley\parley-health-query.exe"),
        volume_serial: None,
        file_index: None,
    };
    assert!(query_matches_installed(
        r"C:\Parley\parley-health-query.exe",
        r"C:\repo",
        &[],
        &installed,
        None
    ));
    assert!(!query_matches_installed(
        r"C:\other\parley-health-query.exe",
        r"C:\repo",
        &[],
        &installed,
        None
    ));
}

#[test]
fn worktree_discovery_reads_gitdir_files_without_spawning_git() {
    let home = common::TempHome::new("git-common");
    let common_dir = home.paths.root.join(".git");
    std::fs::create_dir_all(common_dir.join("worktrees").join("wt1")).unwrap();
    let worktree = home.paths.root.join("linked");
    std::fs::create_dir_all(worktree.join(".git")).unwrap();
    std::fs::write(
        common_dir.join("worktrees").join("wt1").join("gitdir"),
        worktree.join(".git").to_string_lossy().as_bytes(),
    )
    .unwrap();
    let roots = worktrees_from_common_dir(&common_dir);
    assert!(roots
        .iter()
        .any(|root| paths_equivalent(root, &home.paths.root.to_string_lossy())));
    assert!(roots
        .iter()
        .any(|root| paths_equivalent(root, &worktree.to_string_lossy())));
}

#[test]
fn command_analysis_distinguishes_exact_names_from_prefix_lookalikes() {
    let query = analyze_command(r"C:\Parley\parley-health-query.exe");
    assert_eq!(query.health_binary, Some(HealthBinary::Query));
    assert!(query.is_exact_query());

    let sibling = analyze_command(r"C:\Parley\parley-health-query-wrapper.exe");
    assert_eq!(sibling.health_binary, Some(HealthBinary::Query));
    assert!(!sibling.is_exact_query());
}
