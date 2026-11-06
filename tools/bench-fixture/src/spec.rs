use crate::images::ImageSpec;
use anyhow::{Context, Result};
use artisan_api::{CreateProject, CreateThread, ItemId, ItemKind, RunId, Status, ThreadId};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Deserialize)]
pub struct Spec {
    pub projects: Vec<CreateProject>,
    pub threads: Vec<CreateThread>,
    pub images: BTreeMap<String, ImageSpec>,
    pub items: Vec<Value>,
    pub unread: Vec<ThreadId>,
    #[serde(default)]
    pub settled: Vec<ThreadId>,
    pub stream: StreamSpec,
}

#[derive(Debug, Deserialize)]
pub struct StreamSpec {
    pub thread_id: ThreadId,
    pub run_id: RunId,
    pub model: String,
    pub steps: Vec<StreamStep>,
}

#[derive(Debug, Deserialize)]
pub struct StreamStep {
    pub at_ms: u64,
    pub item: Value,
}

#[derive(Debug, Deserialize)]
pub struct SpecItem {
    pub id: ItemId,
    pub thread_id: ThreadId,
    pub run_id: Option<RunId>,
    pub parent_id: Option<ItemId>,
    pub native_id: Option<String>,
    pub status: Status,
    pub kind: ItemKind,
}

impl Spec {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        serde_json::from_slice(&bytes).context("parse the fixture spec")
    }
}

pub fn substitute_blobs(value: &mut Value, hashes: &BTreeMap<String, String>) -> Result<()> {
    match value {
        Value::String(text) => {
            if let Some(name) = text.strip_prefix("blob:") {
                *text = hashes
                    .get(name)
                    .with_context(|| format!("spec references unknown image {name}"))?
                    .clone();
            }
            Ok(())
        }
        Value::Array(values) => values
            .iter_mut()
            .try_for_each(|value| substitute_blobs(value, hashes)),
        Value::Object(map) => map
            .values_mut()
            .try_for_each(|value| substitute_blobs(value, hashes)),
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
    }
}

pub fn item(mut value: Value, hashes: &BTreeMap<String, String>) -> Result<SpecItem> {
    substitute_blobs(&mut value, hashes)?;
    serde_json::from_value(value).context("parse a fixture item")
}
