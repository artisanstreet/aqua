use std::path::{Path, PathBuf};

use crate::Generic;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FaceRef {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
}

pub(crate) struct Discovery {
    pub(crate) directories: Vec<PathBuf>,
    pub(crate) files: Vec<PathBuf>,
    pub(crate) sorted: Vec<(Generic, Vec<FaceRef>)>,
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

fn configured_directories() -> Vec<PathBuf> {
    std::env::var_os("AQUA_FONT_DIRS")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default()
}

fn flat_files(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_font_file(path))
        .collect();
    paths.sort();
    paths
}

#[cfg(target_os = "linux")]
fn nested_files(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut files = Vec::new();
    let mut pending: Vec<PathBuf> = roots.iter().rev().cloned().collect();
    while let Some(directory) = pending.pop() {
        let Ok(canonical) = directory.canonicalize() else {
            continue;
        };
        if !visited.insert(canonical) {
            continue;
        }
        files.extend(flat_files(&directory));
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        children.sort();
        pending.extend(children.into_iter().rev());
    }
    files
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn discover() -> Discovery {
    let windows = std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(|root| PathBuf::from(root).join("Fonts"));
    let user = std::env::var_os("LOCALAPPDATA").map(|base| {
        PathBuf::from(base)
            .join("Microsoft")
            .join("Windows")
            .join("Fonts")
    });
    let directories: Vec<PathBuf> = windows
        .into_iter()
        .chain(user)
        .chain(configured_directories())
        .collect();
    let files = directories
        .iter()
        .flat_map(|directory| flat_files(directory))
        .collect();
    Discovery {
        directories,
        files,
        sorted: Vec::new(),
    }
}

#[cfg(target_os = "linux")]
fn xdg_directories() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.as_ref().map(|home| home.join(".local").join("share")));
    let data_dirs = std::env::var_os("XDG_DATA_DIRS")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .map(|base| base.join("fonts"))
        .into_iter()
        .chain(home.map(|home| home.join(".fonts")))
        .chain(std::env::split_paths(&data_dirs).map(|base| base.join("fonts")))
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn discover() -> Discovery {
    use std::sync::Once;

    static REPORTED: Once = Once::new();
    let configured = configured_directories();
    let extra = nested_files(&configured);
    match crate::fontconfig::snapshot() {
        Ok(snapshot) => {
            let listed: std::collections::HashSet<PathBuf> =
                snapshot.files.iter().cloned().collect();
            let mut files = snapshot.files;
            files.extend(extra.into_iter().filter(|path| !listed.contains(path)));
            Discovery {
                directories: snapshot.directories.into_iter().chain(configured).collect(),
                files,
                sorted: snapshot.sorted,
            }
        }
        Err(error) => {
            REPORTED.call_once(|| {
                eprintln!("aqua: fontconfig is unavailable ({error}); scanning the XDG font directories instead");
            });
            let directories: Vec<PathBuf> =
                xdg_directories().into_iter().chain(configured).collect();
            Discovery {
                files: nested_files(&directories),
                directories,
                sorted: Vec::new(),
            }
        }
    }
}
