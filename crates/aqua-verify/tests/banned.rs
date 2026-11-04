use std::collections::BTreeSet;
use std::path::Path;

use aqua_verify::metadata::workspace_root;
use aqua_verify::policy::{
    ADAPTER_CRATES, BANNED_EVERYWHERE, HOST_CRATES, PURE_CRATES, VERIFY_CRATES,
};

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn manifests() -> Vec<std::path::PathBuf> {
    let root = workspace_root();
    let members = std::fs::read_dir(root.join("crates"))
        .expect("crates directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path().join("Cargo.toml"))
        .filter(|path| path.exists());
    std::iter::once(root.join("Cargo.toml"))
        .chain(members)
        .collect()
}

fn declares_dependency(manifest: &str, name: &str) -> bool {
    manifest.lines().map(str::trim).any(|line| {
        line.strip_prefix(name)
            .map(|rest| rest.trim_start().starts_with('=') || rest.starts_with('.'))
            .unwrap_or(false)
            || line.starts_with(&format!("[dependencies.{name}]"))
            || line.starts_with(&format!("[dev-dependencies.{name}]"))
            || line.starts_with(&format!("[build-dependencies.{name}]"))
            || line.contains(&format!("package = \"{name}\""))
    })
}

#[test]
fn no_manifest_declares_a_banned_crate() {
    let found: Vec<String> = manifests()
        .iter()
        .flat_map(|path| {
            let text = read(path);
            BANNED_EVERYWHERE
                .iter()
                .filter(|name| declares_dependency(&text, name))
                .map(|name| format!("{} declares {name}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        found.is_empty(),
        "banned dependencies:\n{}",
        found.join("\n")
    );
}

#[test]
fn lockfile_contains_no_banned_package() {
    let lock = read(&workspace_root().join("Cargo.lock"));
    let packages: BTreeSet<&str> = lock
        .lines()
        .filter_map(|line| line.strip_prefix("name = \""))
        .filter_map(|rest| rest.strip_suffix('"'))
        .collect();
    let found: Vec<&&str> = BANNED_EVERYWHERE
        .iter()
        .filter(|name| packages.contains(*name))
        .collect();
    assert!(found.is_empty(), "banned packages in Cargo.lock: {found:?}");
}

#[test]
fn workspace_crate_list_matches_the_architecture_exactly() {
    let expected: BTreeSet<String> = PURE_CRATES
        .iter()
        .chain(ADAPTER_CRATES)
        .chain(HOST_CRATES)
        .chain(VERIFY_CRATES)
        .map(|name| name.to_string())
        .collect();
    let actual: BTreeSet<String> = std::fs::read_dir(workspace_root().join("crates"))
        .expect("crates directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("Cargo.toml").exists())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    let extra: Vec<&String> = actual.difference(&expected).collect();
    let missing: Vec<&String> = expected.difference(&actual).collect();
    assert!(
        extra.is_empty() && missing.is_empty(),
        "crate list differs from section 4: extra {extra:?}, missing {missing:?}"
    );
}

#[test]
fn no_source_names_a_banned_crate() {
    let tokens = [
        "wgpu::",
        "glyphon::",
        "cosmic_text::",
        "winit::",
        "fontdb::",
        "accesskit_winit::",
    ];
    let root = workspace_root().join("crates");
    let mut stack = vec![root];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match (path.is_dir(), path.extension().and_then(|e| e.to_str())) {
                (true, _) => stack.push(path),
                (false, Some("rs")) if !path.ends_with("tests/banned.rs") => {
                    let text = read(&path);
                    found.extend(
                        tokens
                            .iter()
                            .filter(|token| text.contains(**token))
                            .map(|token| format!("{} uses {token}", path.display())),
                    );
                }
                _ => {}
            }
        }
    }
    assert!(found.is_empty(), "banned sources:\n{}", found.join("\n"));
}
