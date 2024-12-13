mod client;

use clap::Parser;
use client::ClientBuilder;
use color_eyre::Result;
use std::io;
use tokio::net::TcpListener;
use tracing::{info, level_filters::LevelFilter};

/// Real-time cross-editor collaborative editing backend.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Address to listen to incoming connections on.
    #[arg(short, long, default_value = "0.0.0.0")]
    address: String,

    /// Port to listen to incoming connections on.
    #[arg(short, long, default_value = "6969")]
    port: u16,

    /// Print debug information to stderr.
    #[arg(long, default_value = "false")]
    debug: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut filter = tracing_subscriber::filter::EnvFilter::from_default_env();
    if args.debug {
        filter = filter.add_directive(LevelFilter::DEBUG.into());
    }
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(io::stderr)
        .with_ansi(false)
        .init();

    color_eyre::config::HookBuilder::new()
        .theme(color_eyre::config::Theme::new())
        .install()?;

    let addr = format!("{}:{}", args.address, args.port);
    let listener = TcpListener::bind(&addr).await.unwrap();
    info!("Listening on {addr}");

    let client = ClientBuilder::new(listener).build();

    info!("Entering client event loop");
    client.begin_event_loop().await;

    info!("Client exited, returning");
    Ok(())
}
