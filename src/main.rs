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

    let mut client = Client::new(stream);

    client.begin_update_task();

    // if server {
    //     text.append_string("Hello world").await;
    // } else {
    //     text.append_string("Foobar").await;
    // }
    //
    // text.broadcast_changes().await;
    // tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    // println!("After update: {:?}", text.read().await);

    let mut input = String::new();
    loop {
        println!("Current text: {:?}", client.read().await);

        input.clear();
        std::io::stdin().read_line(&mut input).unwrap();

        if input.trim().is_empty() {
            continue;
        }

        let (command, rest) = input.split_at(2);
        match command {
            "i " => {
                let (index, text) = rest.split_at(rest.find(' ').unwrap());
                let index = index.trim().parse::<usize>().unwrap();
                let text = text.trim();
                client.insert_string(index, text).await;
            }
            "d " => {
                let (begin, len) = rest.split_at(rest.find(' ').unwrap());
                let begin = begin.trim().parse::<usize>().unwrap();
                let len = len.trim().parse::<usize>().unwrap();
                client.delete_string(begin, len).await;
            }
            _ => {
                eprintln!("Invalid command");
                continue;
            }
        }

        eprintln!("Broadcasting changes");
        client.broadcast_changes().await;
    }
}
