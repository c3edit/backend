mod client;

use clap::Parser;
use client::Client;
use tokio::net::TcpListener;
use tracing::debug;

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
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    let addr = format!("{}:{}", args.address, args.port);
    let listener = TcpListener::bind(&addr).await.unwrap();
    debug!("Listening on {addr}");

    let client = Client::new(listener);

    debug!("Entering client event loop");
    client.begin_event_loop().await;

    debug!("Client exited, returning");
}
