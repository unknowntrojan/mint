use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use mint_server::mods::{ModIo, Mods};
use mint_server::{AppState, keys, releases::Releases, router, store::Store};
use tracing::info;

const USAGE: &str = "usage:
  mint_server [serve]          run the server
  mint_server gen-key <name>   create a publish key for <name>

environment:
  MINT_SERVER_LISTEN   address to listen on (default 0.0.0.0:8080)
  MINT_SERVER_DATA     data directory (default /data); publish keys live in <data>/keys
  MINT_MODIO_TOKEN     mod.io OAuth token; lets the server download mod files of published
                       presets from mod.io. Without it, it only serves uploaded ones.
  MINT_MODIO_API       mod.io API (default https://api.mod.io/v1)";

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = PathBuf::from(std::env::var("MINT_SERVER_DATA").unwrap_or_else(|_| "/data".into()));
    let keys_file = data.join("keys");

    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["serve"] => serve(data, keys_file).await,
        ["gen-key", publisher] => {
            let (key, line) = keys::generate(publisher).map_err(anyhow::Error::msg)?;
            println!("publish key for {publisher} (shown only once, put it into mint's settings):");
            println!("  {key}");
            println!("add this line to {}:", keys_file.display());
            println!("  {line}");
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

async fn serve(data: PathBuf, keys_file: PathBuf) -> Result<()> {
    tracing_subscriber::fmt()
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let listen = std::env::var("MINT_SERVER_LISTEN").unwrap_or_else(|_| "0.0.0.0:8080".into());
    let store = Store::open(&data).with_context(|| format!("opening {}", data.display()))?;
    if !keys_file.exists() {
        info!("no {} yet, publishing is disabled", keys_file.display());
    }
    let releases = Releases::open(&data, remote_preset::release::PUBLIC_KEY)
        .with_context(|| format!("opening releases in {}", data.display()))?;
    let modio = match std::env::var("MINT_MODIO_TOKEN") {
        Ok(token) if !token.trim().is_empty() => {
            let api =
                std::env::var("MINT_MODIO_API").unwrap_or_else(|_| "https://api.mod.io/v1".into());
            Some(ModIo::new(&api, token.trim()).context("setting up the mod.io client")?)
        }
        _ => {
            info!("no MINT_MODIO_TOKEN, serving only uploaded mod files");
            None
        }
    };
    let mods =
        Mods::open(&data, modio).with_context(|| format!("opening mods in {}", data.display()))?;
    let app = router(AppState {
        store: Arc::new(store),
        releases: Arc::new(releases),
        mods: Arc::new(mods),
        keys_file,
    });

    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    info!("listening on {listen}, data in {}", data.display());
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    Ok(())
}

/// Ctrl+C, or SIGTERM from `docker stop`.
async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.ok();
}
