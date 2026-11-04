use std::path::PathBuf;
use std::sync::OnceLock;

use aqua_verify::lint::{blank_comments_and_strings, scan_crate};
use aqua_verify::metadata::{breaches, load, workspace_root, Workspace};
use aqua_verify::policy::{
    architecture_crate_roles, root_public_items, ADAPTER_CRATES, ADAPTER_PORT_IMPLS,
    ADAPTER_ROOT_ITEMS, ALLOWED_ADAPTER_EDGES, FORBIDDEN_ADAPTER_CRATES, FORBIDDEN_EXTERNAL_CRATES,
    HOST_CRATES, PURE_CRATES, UNSAFE_AUDITED_CRATES, VERIFY_CRATES,
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
fn every_pure_crate_exists_in_the_workspace() {
    let present: Vec<&str> = present_pure_crates()
        .iter()
        .map(|(name, _)| *name)
        .collect();
    let missing: Vec<&str> = PURE_CRATES
        .iter()
        .copied()
        .filter(|name| !present.contains(name))
        .collect();
    assert!(
        missing.is_empty(),
        "pure crates missing from workspace metadata: {missing:?}"
    );
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

fn crate_sources(name: &str) -> Vec<String> {
    fn walk(dir: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match path.is_dir() {
                true => walk(&path, out),
                false if path.extension().is_some_and(|ext| ext == "rs") => {
                    out.extend(std::fs::read_to_string(&path))
                }
                false => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&workspace_root().join("crates").join(name).join("src"), &mut out);
    out
}

fn crate_root(name: &str) -> String {
    std::fs::read_to_string(workspace_root().join("crates").join(name).join("src/lib.rs"))
        .unwrap_or_else(|error| panic!("{name}: {error}"))
}

#[test]
fn pure_adapter_host_and_verify_lists_match_the_architecture_document() {
    let document = std::fs::read_to_string(workspace_root().join("ARCHITECTURE.md"))
        .expect("ARCHITECTURE.md");
    let roles = architecture_crate_roles(&document);
    let named = |role: &str| -> Vec<String> {
        let mut names: Vec<String> = roles
            .iter()
            .filter(|(r, _)| r == role)
            .map(|(_, name)| name.clone())
            .collect();
        names.sort();
        names
    };
    let sorted = |lists: &[&[&str]]| -> Vec<String> {
        let mut names: Vec<String> = lists
            .iter()
            .flat_map(|list| list.iter().map(|name| name.to_string()))
            .collect();
        names.sort();
        names
    };
    assert_eq!(PURE_CRATES.len(), 13);
    assert_eq!(named("Pure"), sorted(&[PURE_CRATES]));
    assert_eq!(named("Host"), sorted(&[HOST_CRATES]));
    assert_eq!(
        named("Adapters"),
        sorted(&[ADAPTER_CRATES, VERIFY_CRATES])
    );
}

#[test]
fn every_listed_crate_is_a_workspace_member_and_every_member_is_listed() {
    let listed: Vec<&str> = [PURE_CRATES, ADAPTER_CRATES, HOST_CRATES, VERIFY_CRATES]
        .iter()
        .flat_map(|list| list.iter().copied())
        .collect();
    let mut members: Vec<&str> = workspace().members.keys().map(String::as_str).collect();
    members.sort();
    let mut expected = listed.clone();
    expected.sort();
    assert_eq!(members, expected);
}

#[test]
fn adapters_expose_only_declared_root_items() {
    let failures: Vec<String> = ADAPTER_CRATES
        .iter()
        .flat_map(|name| {
            let declared: &[&str] = ADAPTER_ROOT_ITEMS
                .iter()
                .find(|(crate_name, _)| crate_name == name)
                .map(|(_, items)| *items)
                .unwrap_or(&[]);
            let actual = root_public_items(&crate_root(name));
            let undeclared = actual
                .iter()
                .filter(|item| !declared.contains(&item.as_str()))
                .map(|item| format!("{name} exports undeclared {item}"))
                .collect::<Vec<_>>();
            let vanished = declared
                .iter()
                .filter(|item| !actual.iter().any(|a| a == *item))
                .map(|item| format!("{name} declares {item} but no longer exports it"))
                .collect::<Vec<_>>();
            undeclared.into_iter().chain(vanished).collect::<Vec<_>>()
        })
        .collect();
    assert!(
        failures.is_empty(),
        "adapter facade drift:\n{}",
        failures.join("\n")
    );
}

#[test]
fn adapters_declare_and_implement_their_ports() {
    let missing: Vec<String> = ADAPTER_PORT_IMPLS
        .iter()
        .filter(|(name, port)| {
            !crate_sources(name).iter().any(|source| {
                blank_comments_and_strings(source)
                    .lines()
                    .any(|line| {
                        let line = line.trim_start();
                        line.starts_with("impl")
                            && line.contains(&format!("{port} for "))
                            && !line.contains("fmt::")
                    })
            })
        })
        .map(|(name, port)| format!("{name} has no impl of {port}"))
        .collect();
    assert!(missing.is_empty(), "{}", missing.join("\n"));
}

#[test]
fn adapters_never_re_export_platform_crates_except_declared_ones() {
    let leaks: Vec<String> = ADAPTER_CRATES
        .iter()
        .flat_map(|name| {
            root_public_items(&crate_root(name))
                .into_iter()
                .filter(|item| FORBIDDEN_EXTERNAL_CRATES.contains(&item.as_str()))
                .map(move |item| format!("{name} re-exports {item}"))
        })
        .collect();
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}
