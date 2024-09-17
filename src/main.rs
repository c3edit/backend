mod client;

use clap::Parser;
use client::Client;
use tokio::net::TcpListener;

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
    let args = Args::parse();

    let addr = format!("{}:{}", args.address, args.port);
    let listener = TcpListener::bind(&addr).await.unwrap();

    let client = Client::new(listener);

    client.begin_event_loop().await;
}
