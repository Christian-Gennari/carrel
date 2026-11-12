//! CLI entry point for `stylosd`.
//!
//! WP3 ships the sync and pairing commands used by tests and by the future
//! D-Bus service. The long-running daemon and its D-Bus API arrive in WP4.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Args, Parser, Subcommand};
use stylosd::{generate_device_pubkey, sync_once, PairingSession, SyncConfig, SyncOptions};

#[derive(Debug, Parser)]
#[command(
    name = "stylosd",
    version,
    about = "Stylos system daemon (sync client)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Synchronize the writing directory with the paired server, once.
    Sync(SyncArgs),
    /// Pair this device with a server and store the token.
    Pair(PairArgs),
}

#[derive(Debug, Args)]
struct SyncArgs {
    /// Run one sync and exit. The background daemon is a later work package.
    #[arg(long)]
    once: bool,

    /// Writing directory (plain Markdown files).
    #[arg(long, env = "STYLOS_WRITING_DIR", default_value = "/data/writing")]
    dir: PathBuf,

    /// State directory: sync.toml and sync-state.sqlite.
    #[arg(long, env = "STYLOS_STATE_DIR", default_value = "/data/.stylos")]
    state: PathBuf,
}

#[derive(Debug, Args)]
struct PairArgs {
    /// Base URL of the server, for example https://192.168.1.20:8443
    #[arg(long)]
    url: String,

    /// State directory: sync.toml and sync-state.sqlite.
    #[arg(long, env = "STYLOS_STATE_DIR", default_value = "/data/.stylos")]
    state: PathBuf,

    /// Device name shown on the approval page.
    #[arg(long, env = "STYLOS_DEVICE_NAME")]
    name: Option<String>,

    /// Poll interval while waiting for approval.
    #[arg(long, default_value_t = 2000)]
    poll_ms: u64,

    /// Give up after this many seconds (the protocol allows 10 minutes).
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sync(args) => run_sync(args).await,
        Command::Pair(args) => run_pair(args).await,
    }
}

async fn run_sync(args: SyncArgs) -> Result<()> {
    if !args.once {
        bail!("only `stylosd sync --once` is implemented; the D-Bus daemon is WP4");
    }
    let report = sync_once(&args.dir, &args.state, &SyncOptions::default()).await?;
    if report.is_noop() {
        println!("Sync complete: nothing to transfer.");
    } else {
        println!(
            "Sync complete: {} pulled, {} pushed, {} deleted locally, \
             {} deleted on the server, {} conflict copy/copies.",
            report.pulled,
            report.pushed,
            report.deleted_local,
            report.deleted_remote,
            report.conflicts
        );
    }
    if report.skipped_local > 0 {
        println!(
            "{} local file(s) were skipped (not UTF-8 Markdown or not a valid path).",
            report.skipped_local
        );
    }
    Ok(())
}

async fn run_pair(args: PairArgs) -> Result<()> {
    let device_name = args.name.unwrap_or_else(default_device_name);
    let device_pubkey = generate_device_pubkey()?;
    let session = PairingSession::start(&args.url, &device_name, &device_pubkey).await?;

    println!("Pairing code: {}", session.code());
    println!("Approve this code at: {}", session.approval_url());
    println!("Waiting for approval…");

    let credential = session
        .wait(
            Duration::from_millis(args.poll_ms.max(25)),
            Duration::from_secs(args.timeout_secs.max(1)),
        )
        .await?;
    let config = credential.to_sync_config();
    config.save(&args.state)?;

    println!(
        "Paired with {}.",
        config
            .server_name
            .as_deref()
            .unwrap_or(config.server_url.as_str())
    );
    println!("Server URL: {}", config.server_url);
    match &config.cert_sha256 {
        Some(fingerprint) => println!("Certificate pinned: {fingerprint}"),
        None => println!("Certificate not pinned (public CA)."),
    }
    println!("Configuration: {}", SyncConfig::path(&args.state).display());
    Ok(())
}

fn default_device_name() -> String {
    if let Ok(name) = std::env::var("HOSTNAME") {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Ok(name) = std::fs::read_to_string("/etc/hostname") {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    "stylos".to_string()
}
