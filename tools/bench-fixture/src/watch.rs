use anyhow::Result;
use artisan_api::ThreadId;
use forge_store::Store;
use serde_json::json;
use std::{io::Write as _, path::Path, time::Duration};

pub async fn opened(store: Store, threads: Vec<ThreadId>, events: &Path) -> Result<()> {
    let mut pending = threads;
    while !pending.is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut still = Vec::new();
        for id in pending {
            let unread = store
                .thread(&id)
                .await?
                .is_none_or(|thread| thread.marked_unread);
            if unread {
                still.push(id);
                continue;
            }
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis());
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(events)?;
            writeln!(
                file,
                "{}",
                json!({"event": "opened", "thread": id, "unix_ms": unix_ms})
            )?;
        }
        pending = still;
    }
    Ok(())
}
