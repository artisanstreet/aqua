use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use aqua_verify::lint::blank_comments_and_strings;
use aqua_verify::metadata::{load, workspace_root, Workspace};
use aqua_verify::policy::{
    KERNEL_CRATE, KERNEL_TWIN_FN, REFERENCE_ONLY_KERNEL_USERS, UNSAFE_AUDITED_CRATES,
};

fn workspace() -> &'static Workspace {
    static WORKSPACE: OnceLock<Workspace> = OnceLock::new();
    WORKSPACE.get_or_init(|| load(&workspace_root()).expect("cargo metadata"))
}

fn crate_dir(name: &str) -> PathBuf {
    workspace()
        .members
        .get(name)
        .unwrap_or_else(|| panic!("{name} is not a workspace member"))
        .manifest_dir
        .clone()
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .flat_map(|entry| {
            let path = entry.path();
            match path.is_dir() {
                true => rust_files(&path),
                false => (path.extension().is_some_and(|ext| ext == "rs"))
                    .then_some(path)
                    .into_iter()
                    .collect(),
            }
        })
        .collect()
}

fn code(path: &Path) -> String {
    blank_comments_and_strings(&std::fs::read_to_string(path).unwrap_or_default())
}

fn squashed(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn the_kernel_crate_is_the_only_audited_unsafe_crate_and_has_no_dependencies() {
    assert_eq!(UNSAFE_AUDITED_CRATES, &[KERNEL_CRATE]);
    assert!(workspace().members[KERNEL_CRATE].dependencies.is_empty());
}

#[test]
fn the_kernel_crate_denies_unsafe_operations_inside_unsafe_functions() {
    let root = squashed(&code(&crate_dir(KERNEL_CRATE).join("src/lib.rs")));
    assert!(root.contains("#![deny(unsafe_op_in_unsafe_fn)]"));
    assert!(!root.contains("#![allow(unsafe"));
}

#[test]
fn every_kernel_module_that_uses_unsafe_has_a_safe_twin_and_a_differential_test() {
    let dir = crate_dir(KERNEL_CRATE);
    let tests: String = rust_files(&dir.join("tests"))
        .iter()
        .map(|path| code(path))
        .collect();
    let offenders: Vec<String> = rust_files(&dir.join("src"))
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name != "lib.rs"))
        .filter(|path| code(path).contains("unsafe"))
        .filter_map(|path| {
            let module = path.file_stem()?.to_str()?.to_string();
            let has_twin = code(&path).contains(KERNEL_TWIN_FN);
            let differential = tests.contains(&format!("{module}::reference"));
            (!(has_twin && differential)).then_some(format!(
                "{module}: twin {has_twin}, differential test {differential}"
            ))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "kernels without a reference twin and differential test:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_oracle_only_calls_the_safe_twins() {
    let calls: Vec<String> = REFERENCE_ONLY_KERNEL_USERS
        .iter()
        .flat_map(|name| rust_files(&crate_dir(name).join("src")))
        .flat_map(|path| {
            let text = code(&path);
            text.match_indices("aqua_kernels::")
                .map(|(start, _)| {
                    text[start..]
                        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                        .next()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect::<Vec<_>>()
        })
        .filter(|call| !call.ends_with("::reference") && !call.ends_with("::reference_pixel"))
        .collect();
    assert!(calls.is_empty(), "oracle reaches fast kernels: {calls:?}");
}
