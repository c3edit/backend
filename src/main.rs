use futures::{SinkExt, TryStreamExt};
use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use std::{
    io::{self},
    sync::Arc,
};
use tokio::{
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::Mutex,
};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[tokio::main]
async fn main() {
    let stream: TcpStream;

    let args = std::env::args().collect::<Vec<String>>();
    let server = args.contains(&String::from("server"));
    if server {
        let listener = TcpListener::bind("0.0.0.0:6969").await.unwrap();
        stream = listener.accept().await.unwrap().0;
    } else {
        let mut address = String::new();
        io::stdin().read_line(&mut address).unwrap();
        stream = TcpStream::connect(address.trim()).await.unwrap();
    }

    let mut text = Text::new(stream);

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

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Message {
    data: Vec<u8>,
}

struct Text {
    doc: Arc<Mutex<LoroDoc>>,
    // I hate Rust sometimes.
    write_socket: tokio_serde::SymmetricallyFramed<
        FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
        Message,
        SymmetricalJson<Message>,
    >,
    read_socket: Option<
        tokio_serde::SymmetricallyFramed<
            FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
            Message,
            SymmetricalJson<Message>,
        >,
    >,
}

impl Text {
    fn new(socket: TcpStream) -> Self {
        socket.set_nodelay(true).unwrap();
        let (read, write) = socket.into_split();
        let read_framed = tokio_serde::SymmetricallyFramed::new(
            FramedRead::new(read, LengthDelimitedCodec::new()),
            SymmetricalJson::<Message>::default(),
        );
        let write_framed = tokio_serde::SymmetricallyFramed::new(
            FramedWrite::new(write, LengthDelimitedCodec::new()),
            SymmetricalJson::<Message>::default(),
        );
        Text {
            doc: Arc::new(Mutex::new(LoroDoc::new())),
            write_socket: write_framed,
            read_socket: Some(read_framed),
        }
    }

    async fn broadcast_changes(&mut self) {
        let message = Message {
            data: self.doc.lock().await.export_from(&Default::default()),
        };
        self.write_socket.send(message).await.unwrap();
    }

    async fn append_string(&mut self, s: &str) {
        let doc = self.doc.lock().await;
        doc.get_text("text").insert(0, s).unwrap();
    }

    fn begin_update_task(&mut self) {
        let mut socket = self.read_socket.take().unwrap();
        let data = self.doc.clone();

        tokio::spawn(async move {
            while let Some(message) = socket.try_next().await.unwrap() {
                println!("Received message: {:?}", message);
                data.lock().await.import(&message.data).unwrap();
            }
        });
    }

    async fn read(&self) -> String {
        self.doc.lock().await.get_text("text").to_string()
    }
}
