mod images;
mod serve;
mod spec;
mod stream;
mod watch;

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

enum Command {
    Identity { out: PathBuf },
    Serve(serve::Options),
}

fn flag(arguments: &[String], name: &str) -> Result<String> {
    arguments
        .iter()
        .position(|argument| argument == name)
        .and_then(|index| arguments.get(index + 1))
        .cloned()
        .with_context(|| format!("missing {name}"))
}

fn parse(arguments: &[String]) -> Result<Command> {
    match arguments.first().map(String::as_str) {
        Some("identity") => Ok(Command::Identity {
            out: flag(arguments, "--out")?.into(),
        }),
        Some("serve") => Ok(Command::Serve(serve::Options {
            data: flag(arguments, "--data")?.into(),
            listen: flag(arguments, "--listen")?.parse()?,
            spec: flag(arguments, "--spec")?.into(),
            device: flag(arguments, "--device")?.into(),
            ready: flag(arguments, "--ready")?.into(),
            events: flag(arguments, "--events")?.into(),
            trigger: flag(arguments, "--trigger")?.into(),
            seed_only: arguments.iter().any(|argument| argument == "--seed-only"),
        })),
        _ => bail!("usage: bench-fixture identity --out FILE | serve --data DIR --listen ADDR --spec FILE --device IDENTITY --ready FILE --events FILE --trigger FILE [--seed-only]"),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match parse(&arguments)? {
        Command::Identity { out } => {
            let identity = artisan_api::tls::Identity::generate("aqua-bench")?;
            std::fs::write(&out, serde_json::to_vec_pretty(&identity)?)
                .with_context(|| format!("write {}", out.display()))
        }
        Command::Serve(options) => serve::run(options).await,
    }
}
