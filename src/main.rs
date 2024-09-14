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
        stream = TcpStream::connect("localhost:6969").await.unwrap();
    }

    let mut text = Client::new(stream);

    text.begin_update_task();

    if server {
        text.append_string("Hello world").await;
    } else {
        text.append_string("Foobar").await;
    }

    text.broadcast_changes().await;
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    println!("After update: {:?}", text.read().await);
}
