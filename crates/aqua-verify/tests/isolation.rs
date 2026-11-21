use std::path::{Path, PathBuf};

use aqua_verify::metadata::workspace_root;
use aqua_verify::policy::{
    GENERAL_PASTEBOARD_TOKENS, REAL_SESSION_TOKENS, SESSION_ESCAPE_TOKENS, TEST_SESSION_DEAD_ENDS,
};

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            match (path.is_dir(), path.extension().and_then(|e| e.to_str())) {
                (true, _) => stack.push(path),
                (false, Some("rs")) => found.push(path),
                _ => {}
            }
        }
    }
    found.sort();
    found
}

#[test]
fn cargo_points_every_process_it_starts_at_a_dead_end_session_bus_and_display() {
    let config = workspace_root().join(".cargo").join("config.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap_or_else(|error| panic!("{}: {error}", config.display()));
    let missing: Vec<String> = TEST_SESSION_DEAD_ENDS
        .iter()
        .filter(|(name, value)| {
            !text.lines().any(|line| {
                line.trim() == format!("{name} = {{ value = \"{value}\", force = true }}")
            })
        })
        .map(|(name, value)| format!("{name} = {{ value = \"{value}\", force = true }}"))
        .collect();
    assert!(
        missing.is_empty(),
        "{} must force these dead ends:\n{}",
        config.display(),
        missing.join("\n")
    );
    let leaked: Vec<String> = TEST_SESSION_DEAD_ENDS
        .iter()
        .filter(|(name, value)| std::env::var(name).as_deref() != Ok(*value))
        .map(|(name, value)| {
            format!(
                "{name} is {:?} in a test process, not {value:?}",
                std::env::var(name).ok()
            )
        })
        .collect();
    assert!(leaked.is_empty(), "{}", leaked.join("\n"));
}

#[cfg(unix)]
#[test]
fn a_test_process_cannot_connect_to_a_session_bus_or_a_compositor() {
    use std::os::unix::net::UnixStream;

    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let bus = match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(address) => address
            .split(';')
            .filter_map(|entry| entry.strip_prefix("unix:"))
            .flat_map(|entry| entry.split(','))
            .find_map(|pair| pair.strip_prefix("path="))
            .map(PathBuf::from),
        Err(_) => runtime.as_ref().map(|dir| dir.join("bus")),
    };
    let display = std::env::var_os("WAYLAND_DISPLAY")
        .map(PathBuf::from)
        .map(|name| match name.is_absolute() {
            true => name,
            false => runtime.clone().unwrap_or_default().join(name),
        });
    let reachable: Vec<String> = [("session bus", bus), ("Wayland display", display)]
        .into_iter()
        .filter_map(|(what, path)| {
            let path = path?;
            let wslg = path
                .canonicalize()
                .is_ok_and(|resolved| resolved.starts_with("/mnt/wslg"));
            (wslg || UnixStream::connect(&path).is_ok())
                .then(|| format!("the default {what} {} is reachable", path.display()))
        })
        .collect();
    assert!(reachable.is_empty(), "{}", reachable.join("\n"));
}

#[test]
fn test_code_never_names_the_real_session_or_strips_the_dead_ends() {
    let this = Path::new(file!())
        .file_name()
        .expect("this test file has a name");
    let found: Vec<String> = rust_files(&workspace_root().join("crates"))
        .into_iter()
        .filter(|path| {
            path.file_name() != Some(this) && !path.ends_with("aqua-verify/src/policy.rs")
        })
        .flat_map(|path| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            REAL_SESSION_TOKENS
                .iter()
                .chain(SESSION_ESCAPE_TOKENS)
                .filter(|token| text.contains(**token))
                .map(|token| format!("{} uses {token}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        found.is_empty(),
        "sources that could reach the user's session:\n{}",
        found.join("\n")
    );
}

fn is_test_source(path: &Path) -> bool {
    path.components().any(|part| part.as_os_str() == "tests")
        || path
            .ancestors()
            .any(|dir| dir.ends_with("aqua-shell-appkit/src/testing"))
}

#[test]
fn no_test_source_names_the_general_pasteboard() {
    let this = Path::new(file!())
        .file_name()
        .expect("this test file has a name");
    let found: Vec<String> = rust_files(&workspace_root().join("crates"))
        .into_iter()
        .filter(|path| path.file_name() != Some(this) && is_test_source(path))
        .flat_map(|path| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            GENERAL_PASTEBOARD_TOKENS
                .iter()
                .filter(|token| text.contains(**token))
                .map(|token| format!("{} uses {token}", path.display()))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        found.is_empty(),
        "test sources that could write to the user's clipboard:\n{}",
        found.join("\n")
    );
}
