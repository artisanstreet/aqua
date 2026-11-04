use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub manifest_dir: PathBuf,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Workspace {
    pub members: BTreeMap<String, Package>,
    pub graph: BTreeMap<String, Package>,
}

#[derive(Debug)]
pub enum MetadataError {
    Spawn(std::io::Error),
    Failed(String),
    Parse(serde_json::Error),
    Shape(&'static str),
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataError::Spawn(error) => write!(f, "cargo metadata did not start: {error}"),
            MetadataError::Failed(stderr) => write!(f, "cargo metadata failed: {stderr}"),
            MetadataError::Parse(error) => write!(f, "cargo metadata output is not json: {error}"),
            MetadataError::Shape(what) => write!(f, "cargo metadata output is missing {what}"),
        }
    }
}

impl std::error::Error for MetadataError {}

pub fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap_or_else(|_| manifest.to_path_buf())
}

pub const TARGETS: &[&str] = &["x86_64-unknown-linux-gnu", "x86_64-pc-windows-gnu"];

fn metadata_for(root: &Path, target: &str) -> Result<Workspace, MetadataError> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--offline",
            "--filter-platform",
            target,
            "--manifest-path",
        ])
        .arg(root.join("Cargo.toml"))
        .output()
        .map_err(MetadataError::Spawn)?;
    match output.status.success() {
        true => parse(&output.stdout),
        false => Err(MetadataError::Failed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )),
    }
}

pub fn merge(left: Workspace, right: Workspace) -> Workspace {
    let union = |mut base: BTreeMap<String, Package>, other: BTreeMap<String, Package>| {
        for (name, package) in other {
            base.entry(name)
                .and_modify(|existing| {
                    let merged: BTreeSet<String> = existing
                        .dependencies
                        .iter()
                        .chain(&package.dependencies)
                        .cloned()
                        .collect();
                    existing.dependencies = merged.into_iter().collect();
                })
                .or_insert(package);
        }
        base
    };
    Workspace {
        members: union(left.members, right.members),
        graph: union(left.graph, right.graph),
    }
}

pub fn load(root: &Path) -> Result<Workspace, MetadataError> {
    TARGETS
        .iter()
        .map(|target| metadata_for(root, target))
        .try_fold(Workspace::default(), |acc, next| {
            next.map(|n| merge(acc, n))
        })
}

pub fn parse(bytes: &[u8]) -> Result<Workspace, MetadataError> {
    let value: Value = serde_json::from_slice(bytes).map_err(MetadataError::Parse)?;
    let packages = value["packages"]
        .as_array()
        .ok_or(MetadataError::Shape("packages"))?;
    let nodes = value["resolve"]["nodes"]
        .as_array()
        .ok_or(MetadataError::Shape("resolve.nodes"))?;
    let member_ids: BTreeSet<&str> = value["workspace_members"]
        .as_array()
        .ok_or(MetadataError::Shape("workspace_members"))?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let by_id: BTreeMap<&str, &Value> = packages
        .iter()
        .filter_map(|package| package["id"].as_str().map(|id| (id, package)))
        .collect();
    let runtime_edges = |node: &Value| -> Vec<String> {
        node["deps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|dep| {
                dep["dep_kinds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|kind| kind["kind"].as_str() != Some("dev"))
            })
            .filter_map(|dep| dep["pkg"].as_str())
            .filter_map(|id| by_id.get(id))
            .filter_map(|package| package["name"].as_str().map(str::to_string))
            .collect()
    };
    let mut workspace = Workspace::default();
    for node in nodes {
        let Some(id) = node["id"].as_str() else {
            continue;
        };
        let Some(package) = by_id.get(id) else {
            continue;
        };
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        let manifest_dir = package["manifest_path"]
            .as_str()
            .and_then(|path| Path::new(path).parent())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let entry = Package {
            name: name.to_string(),
            manifest_dir,
            dependencies: runtime_edges(node),
        };
        if member_ids.contains(id) {
            workspace.members.insert(name.to_string(), entry.clone());
        }
        workspace.graph.insert(name.to_string(), entry);
    }
    Ok(workspace)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Breach {
    pub krate: String,
    pub banned: String,
    pub path: Vec<String>,
}

impl std::fmt::Display for Breach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} reaches {} via {}",
            self.krate,
            self.banned,
            self.path.join(" -> ")
        )
    }
}

pub fn breaches(
    workspace: &Workspace,
    krate: &str,
    banned: &dyn Fn(&str) -> bool,
    allowed_edge: &dyn Fn(&str, &str) -> bool,
) -> Vec<Breach> {
    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: VecDeque<String> = VecDeque::from([krate.to_string()]);
    let mut seen: BTreeSet<String> = BTreeSet::from([krate.to_string()]);
    let mut found = Vec::new();
    while let Some(current) = queue.pop_front() {
        let Some(package) = workspace.graph.get(&current) else {
            continue;
        };
        for dependency in &package.dependencies {
            if allowed_edge(&current, dependency) || !seen.insert(dependency.clone()) {
                continue;
            }
            parents.insert(dependency.clone(), current.clone());
            match banned(dependency) {
                true => {
                    let mut path = vec![dependency.clone()];
                    let mut cursor = dependency.clone();
                    while let Some(parent) = parents.get(&cursor) {
                        path.push(parent.clone());
                        cursor = parent.clone();
                    }
                    path.reverse();
                    found.push(Breach {
                        krate: krate.to_string(),
                        banned: dependency.clone(),
                        path,
                    });
                }
                false => queue.push_back(dependency.clone()),
            }
        }
    }
    found
}
