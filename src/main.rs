mod client;

use client::Client;
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() {
    let stream: TcpStream;

    let args = std::env::args().collect::<Vec<String>>();
    let server = args.contains(&String::from("server"));
    if server {
        let listener = TcpListener::bind("0.0.0.0:6969").await.unwrap();
        stream = listener.accept().await.unwrap().0;
    } else {
        let addr = args[1].clone();
        stream = TcpStream::connect(&addr).await.unwrap();
    }

    eprintln!("Connected");

    let client = Client::new(stream);

    eprintln!("Beginning event loop");
    client.begin_event_loop().await;
}
