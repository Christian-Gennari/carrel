//! CLI entry point for the self-hosted Stylos sync server.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use stylos_server::{start, Config};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "stylos-server",
    version,
    about = "Self-hosted Stylos sync server (plain Markdown + HTTPS + mDNS)"
)]
struct Cli {
    /// Directory that holds the user's Markdown files and the index.
    #[arg(long, env = "STYLOS_DATA_DIR", default_value = "/data")]
    data_dir: PathBuf,

    /// Address to listen on. TLS is always used.
    #[arg(long, env = "STYLOS_LISTEN", default_value = "0.0.0.0:8443")]
    listen: SocketAddr,

    /// Password for the /approve page. Generated and stored when absent.
    #[arg(long, env = "STYLOS_ADMIN_PASSWORD")]
    admin_password: Option<String>,

    /// Name announced over mDNS and used in conflict-copy names.
    #[arg(long, env = "STYLOS_LABEL")]
    label: Option<String>,

    /// Do not announce the server over mDNS (tests, hosts without multicast).
    #[arg(long, env = "STYLOS_NO_MDNS")]
    no_mdns: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("stylos_server=info")),
        )
        .init();

    let cli = Cli::parse();
    let mut config = Config::new(cli.data_dir);
    config.listen = cli.listen;
    config.admin_password = cli.admin_password;
    config.label = cli.label;
    config.mdns = !cli.no_mdns;

    let server = start(config).await?;
    println!("stylos-server listening on {}", server.local_url());
    println!("  name:                 {}", server.label);
    println!("  certificate sha256:   {}", server.cert_sha256);
    println!("  approve pairing at:   {}", server.approve_url());
    println!("  admin password:       {}", server.admin_password);
    println!();
    println!("Press Ctrl-C to stop.");

    tokio::signal::ctrl_c().await?;
    server.shutdown().await;
    Ok(())
}
