use crate::{
    images,
    spec::{self, Spec},
    stream, watch,
};
use anyhow::{Context, Result};
use artisan_api::tls::Identity;
use forge_store::{NewItem, Store};
use serde_json::json;
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf};

pub struct Options {
    pub data: PathBuf,
    pub listen: SocketAddr,
    pub spec: PathBuf,
    pub device: PathBuf,
    pub ready: PathBuf,
    pub events: PathBuf,
    pub trigger: PathBuf,
    pub seed_only: bool,
}

async fn seed(store: &Store, spec: &Spec) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for (name, image) in &spec.images {
        let blob = store.put_image(images::render(*image)?).await?;
        hashes.insert(name.clone(), blob.hash.as_str().to_owned());
    }
    for project in &spec.projects {
        store.create_project(project.clone()).await?;
    }
    for thread in &spec.threads {
        store.create_thread(thread.clone()).await?;
    }
    for value in &spec.items {
        let item = spec::item(value.clone(), &hashes)?;
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
    for id in &spec.unread {
        store.mark_unread(id).await?;
    }
    for id in &spec.settled {
        store.settle_thread(id, u64::MAX).await?;
    }
    Ok(hashes)
}

pub async fn run(options: Options) -> Result<()> {
    let spec = Spec::load(&options.spec)?;
    let device: Identity = serde_json::from_slice(&std::fs::read(&options.device)?)
        .context("read the device identity")?;
    let server = forge_http::start(forge_http::ServeConfig {
        data_dir: options.data.clone(),
        listen: options.listen,
    })
    .await?;
    let store = server.store().clone();
    let address = server.local_addr()?;
    store
        .add_device("aqua-bench".to_owned(), device.certificate.clone())
        .await?;
    let fresh = store.state().await?.threads.is_empty();
    let hashes = if fresh {
        seed(&store, &spec).await?
    } else {
        BTreeMap::new()
    };
    let invite = forge_http::invite(&store).await?;
    let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate)?;
    let ready = json!({
        "address": address.to_string(),
        "fingerprint": fingerprint,
        "threads": spec.threads.iter().map(|thread| thread.id.clone()).collect::<Vec<_>>(),
        "items": spec.items.len(),
        "stream_thread": spec.stream.thread_id,
        "stream_steps": spec.stream.steps.len(),
    });
    std::fs::write(&options.ready, serde_json::to_vec_pretty(&ready)?)?;
    if options.seed_only {
        drop(store);
        return server.run_until(async {}).await;
    }
    let _ = std::fs::remove_file(&options.events);
    let _ = std::fs::remove_file(&options.trigger);
    tokio::spawn({
        let store = store.clone();
        let events = options.events.clone();
        let unread = spec.unread.clone();
        async move {
            if let Err(error) = watch::opened(store, unread, &events).await {
                eprintln!("watch: {error:#}");
            }
        }
    });
    tokio::spawn({
        let store = store.clone();
        let (events, trigger) = (options.events.clone(), options.trigger.clone());
        async move {
            if let Err(error) = stream::play(store, spec, hashes, &trigger, &events).await {
                eprintln!("stream: {error:#}");
            }
        }
    });
    server
        .run_until(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
