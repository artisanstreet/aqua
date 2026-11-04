use std::path::PathBuf;
use std::sync::OnceLock;

use aqua_verify::lint::{blank_comments_and_strings, scan_crate};
use aqua_verify::metadata::{Workspace, breaches, load, workspace_root};
use aqua_verify::policy::{
    ALLOWED_ADAPTER_EDGES, FORBIDDEN_ADAPTER_CRATES, FORBIDDEN_EXTERNAL_CRATES, PURE_CRATES,
    UNSAFE_AUDITED_CRATES,
};

fn workspace() -> &'static Workspace {
    static WORKSPACE: OnceLock<Workspace> = OnceLock::new();
    WORKSPACE.get_or_init(|| load(&workspace_root()).expect("cargo metadata"))
}

fn present_pure_crates() -> Vec<(&'static str, PathBuf)> {
    PURE_CRATES
        .iter()
        .filter_map(|name| {
            workspace()
                .members
                .get(*name)
                .map(|package| (*name, package.manifest_dir.clone()))
        })
        .collect()
}

#[test]
fn pure_crates_exist_in_the_workspace() {
    let present: Vec<&str> = present_pure_crates()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    for required in [
        "aqua-core",
        "aqua-protocol",
        "aqua-graph",
        "aqua-runtime",
        "aqua-scene",
    ] {
        assert!(
            present.contains(&required),
            "{required} missing from workspace metadata"
        );
    }
}

#[test]
fn pure_crates_do_not_reach_adapters_or_platform_crates() {
    let banned = |name: &str| {
        FORBIDDEN_ADAPTER_CRATES.contains(&name) || FORBIDDEN_EXTERNAL_CRATES.contains(&name)
    };
    let allowed = |from: &str, to: &str| ALLOWED_ADAPTER_EDGES.contains(&(from, to));
    let found: Vec<String> = present_pure_crates()
        .iter()
        .flat_map(|(name, _)| breaches(workspace(), name, &banned, &allowed))
        .map(|breach| breach.to_string())
        .collect();
    assert!(found.is_empty(), "purity violations:\n{}", found.join("\n"));
}

#[test]
fn pure_crates_forbid_unsafe_code() {
    let missing: Vec<&str> = present_pure_crates()
        .into_iter()
        .filter(|(name, _)| !UNSAFE_AUDITED_CRATES.contains(name))
        .filter(|(_, dir)| {
            let root = ["lib.rs", "main.rs"]
                .iter()
                .find_map(|file| std::fs::read_to_string(dir.join("src").join(file)).ok())
                .unwrap_or_default();
            let code: String = blank_comments_and_strings(&root)
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            !code.contains("#![forbid(unsafe_code)]")
        })
        .map(|(name, _)| name)
        .collect();
    assert!(
        missing.is_empty(),
        "missing #![forbid(unsafe_code)]: {missing:?}"
    );
}

#[test]
fn pure_crates_have_no_clock_thread_fs_or_stdio_calls() {
    let hits: Vec<String> = present_pure_crates()
        .iter()
        .flat_map(|(_, dir)| scan_crate(dir))
        .map(|hit| format!("{}:{} uses {}", hit.path.display(), hit.line, hit.token))
        .collect();
    assert!(
        hits.is_empty(),
        "impure calls in pure crates:\n{}",
        hits.join("\n")
    );
}
