mod client;
mod config;
mod hub;
mod upnp;
mod watcher;

use crate::hub::Hub;
use anyhow::Result;
use clap::Parser;
use std::env;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "nat2", about, version, author)]
#[command(args_conflicts_with_subcommands = true)]
struct Opt {
    #[arg(short = 'c', long, default_value = "config.json")]
    config: String,
    #[arg(long)]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::parse();
    if env::var("RUST_LOG").is_err() {
        if opt.debug {
            env::set_var("RUST_LOG", "nat2=debug,reqwest=debug");
        } else {
            env::set_var("RUST_LOG", "nat2=info,reqwest=warn");
        }
    }
    let cfg = config::load(opt.config)?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(tracing_subscriber::fmt::time::time())
        .init();
    let mut hub = Hub::new(cfg).await?;
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("closing connections");
            hub.close().await;
        },
        _ = hub.run() => {},
    }
    Ok(())
}
