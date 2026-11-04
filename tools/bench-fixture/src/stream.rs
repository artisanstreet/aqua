use crate::spec::{self, Spec};
use anyhow::{Context, Result};
use forge_store::{NewItem, Store};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio::time::Instant;

fn run_record(spec: &Spec, status: &str, ended: bool) -> Result<artisan_api::Run> {
    let now = artisan_api::now();
    serde_json::from_value(json!({
        "config": null,
        "id": spec.stream.run_id,
        "thread_id": spec.stream.thread_id,
        "engine": "claude",
        "model": spec.stream.model,
        "input_cutoff": 0,
        "status": status,
        "usage": {
            "input": 0, "output": 0, "reasoning": 0, "cache_read": 0,
            "cache_write": null, "context_used": null, "context_window": null, "cost_usd": null
        },
        "session_id": null,
        "error": null,
        "seq": 0,
        "started_at": now,
        "ended_at": if ended { json!(now) } else { Value::Null },
        "continuation": null
    }))
    .context("build the streaming run")
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis())
}

fn mark(events: &Path, event: &str) -> Result<()> {
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events)?;
    writeln!(file, "{}", json!({"event": event, "unix_ms": unix_ms()}))?;
    Ok(())
}

pub async fn play(
    store: Store,
    spec: Spec,
    hashes: BTreeMap<String, String>,
    trigger: &Path,
    events: &Path,
) -> Result<()> {
    while !trigger.exists() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    mark(events, "stream_start")?;
    store.upsert_run(run_record(&spec, "running", false)?).await?;
    let start = Instant::now();
    for step in &spec.stream.steps {
        tokio::time::sleep_until(start + Duration::from_millis(step.at_ms)).await;
        let item = spec::item(step.item.clone(), &hashes)?;
        store
            .upsert_item(NewItem {
                id: item.id,
                thread_id: item.thread_id,
                run_id: item.run_id,
                parent_id: item.parent_id,
                native_id: item.native_id,
                status: item.status,
                kind: item.kind,
            })
            .await?;
    }
    store.upsert_run(run_record(&spec, "done", true)?).await?;
    mark(events, "stream_end")
}
