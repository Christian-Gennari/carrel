//! CLI entry point for `stylosd`.
//!
//! `stylosd run` is the system service. `stylosd sync` and `stylosd pair` are
//! the single-shot commands used by tests and by an administrator.
//! `stylosd gdrive-pair` and `stylosd gdrive-sync` are the manual test tools
//! for a real Google account (WP6b).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use stylosd::dbus::RunConfig;
use stylosd::export::PandocDocxConverter;
use stylosd::gdrive::{DriveSyncRunner, GdriveConfig, OAuthApp, OAuthClient, OAuthEndpoints};
use stylosd::{
    generate_device_pubkey, sync_once, GdriveRunner, PairingSession, SyncConfig, SyncOptions,
};

#[derive(Debug, Parser)]
#[command(
    name = "stylosd",
    version,
    about = "Stylos system daemon (sync client, history, radio, D-Bus)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the D-Bus daemon (the systemd service).
    Run(RunArgs),
    /// Synchronize the writing directory with the paired server, once.
    Sync(SyncArgs),
    /// Pair this device with a server and store the token.
    Pair(PairArgs),
    /// Pair this device with Google Drive (OAuth device flow) and store the token.
    GdrivePair(GdrivePairArgs),
    /// Mirror the writing directory to Google Drive, once.
    GdriveSync(GdriveSyncArgs),
}

#[derive(Debug, Args)]
struct RunArgs {
    /// Writing directory (plain Markdown files).
    #[arg(long, env = "STYLOS_WRITING_DIR", default_value = "/data/writing")]
    dir: PathBuf,

    /// State directory: sync.toml, sync-state.sqlite, history.
    #[arg(long, env = "STYLOS_STATE_DIR", default_value = "/data/.stylos")]
    state: PathBuf,
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

#[derive(Debug, Args)]
struct GdrivePairArgs {
    /// State directory: sync.toml and sync-state.sqlite.
    #[arg(long, env = "STYLOS_STATE_DIR", default_value = "/data/.stylos")]
    state: PathBuf,

    /// Google OAuth client file (default: /data/.stylos/oauth.toml, then
    /// /usr/share/stylos/oauth.toml).
    #[arg(long)]
    oauth: Option<PathBuf>,

    /// Give up after this many seconds (Google codes last ten minutes).
    #[arg(long, default_value_t = 600)]
    timeout_secs: u64,
}

#[derive(Debug, Args)]
struct GdriveSyncArgs {
    /// Writing directory (plain Markdown files).
    #[arg(long, env = "STYLOS_WRITING_DIR", default_value = "/data/writing")]
    dir: PathBuf,

    /// State directory: sync.toml and sync-state.sqlite.
    #[arg(long, env = "STYLOS_STATE_DIR", default_value = "/data/.stylos")]
    state: PathBuf,

    /// Google OAuth client file (default: /data/.stylos/oauth.toml, then
    /// /usr/share/stylos/oauth.toml).
    #[arg(long)]
    oauth: Option<PathBuf>,

    /// Also upload a .docx copy of every file, for Google Docs.
    #[arg(long)]
    docx: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run(args) => {
            tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "stylosd=info".into()),
                )
                .with_target(false)
                .init();
            stylosd::dbus::run(RunConfig {
                writing_dir: args.dir,
                state_dir: args.state,
            })
            .await
        }
        Command::Sync(args) => run_sync(args).await,
        Command::Pair(args) => run_pair(args).await,
        Command::GdrivePair(args) => run_gdrive_pair(args).await,
        Command::GdriveSync(args) => run_gdrive_sync(args).await,
    }
}

async fn run_sync(args: SyncArgs) -> Result<()> {
    if !args.once {
        bail!("only `stylosd sync --once` is implemented; the D-Bus daemon is WP4");
    }
    let report = sync_once(&args.dir, &args.state, &SyncOptions::default()).await?;
    if report.is_noop() {
        println!("Sync complete: nothing to transfer.");
    } else if report.library_only() {
        println!(
            "Sync complete: {} library item(s) updated, {} removed.",
            report.library_items, report.library_deleted
        );
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
        if let Some(library) = report.library_summary() {
            println!("{library}");
        }
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

/// `stylosd gdrive-pair`: run the Google device flow and store the refresh
/// token in `<state>/sync.toml` (mode 0600).
async fn run_gdrive_pair(args: GdrivePairArgs) -> Result<()> {
    let app = load_oauth(args.oauth.as_deref())?;
    let client = OAuthClient::new(app, OAuthEndpoints::from_env_or_default())?;
    let device = client.start_device_flow().await?;

    println!("Open this URL on a phone or computer signed in to Google:");
    println!("  {}", device.verification_url);
    println!("and enter this code: {}", device.user_code);
    println!("Waiting for approval…");

    let tokens = client
        .wait_for_approval(&device, Duration::from_secs(args.timeout_secs.max(1)))
        .await?;
    let refresh_token = tokens
        .refresh_token
        .filter(|token| !token.trim().is_empty())
        .context("Google did not return a refresh token; try pairing again")?;
    let docx_copy = GdriveConfig::load(&args.state)?
        .map(|config| config.docx_copy)
        .unwrap_or(false);
    GdriveConfig {
        refresh_token,
        docx_copy,
    }
    .save(&args.state)?;

    println!("Paired with Google Drive.");
    println!("Configuration: {}", SyncConfig::path(&args.state).display());
    let oauth = args
        .oauth
        .as_ref()
        .map(|path| format!(" --oauth {}", path.display()))
        .unwrap_or_default();
    println!(
        "Next: stylosd gdrive-sync --dir /data/writing --state {}{oauth}",
        args.state.display()
    );
    Ok(())
}

/// `stylosd gdrive-sync`: one real mirror run, for the manual account test.
async fn run_gdrive_sync(args: GdriveSyncArgs) -> Result<()> {
    if !args.dir.is_dir() {
        bail!("writing directory {} does not exist", args.dir.display());
    }
    let mut config = GdriveConfig::load(&args.state)?
        .context("Google Drive is not paired; run `stylosd gdrive-pair` first")?;
    if config.docx_copy != args.docx {
        config.docx_copy = args.docx;
        config.save(&args.state)?;
    }

    let runner = match args.oauth.as_deref() {
        Some(path) => DriveSyncRunner::with_converter(Arc::new(PandocDocxConverter))
            .with_app(OAuthApp::load_from(path)?),
        None => DriveSyncRunner::with_converter(Arc::new(PandocDocxConverter)),
    };
    let report = runner.sync(&args.dir, &args.state).await?;

    println!(
        "Google Drive: {} uploaded, {} unchanged, {} folder(s) created, \
         {} moved to .deleted, {} skipped.",
        report.uploaded, report.unchanged, report.folders, report.deleted, report.skipped
    );
    if args.docx {
        println!("Every changed file also got a .docx copy for Google Docs.");
    }
    Ok(())
}

fn load_oauth(path: Option<&Path>) -> Result<OAuthApp> {
    match path {
        Some(path) => OAuthApp::load_from(path),
        None => OAuthApp::load(),
    }
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
