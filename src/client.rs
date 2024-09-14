use futures::{SinkExt, TryStreamExt};
use loro::LoroDoc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::{
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::Mutex,
};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Message {
    data: Vec<u8>,
}

pub struct Client {
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

impl Client {
    pub fn new(socket: TcpStream) -> Self {
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

        Client {
            doc: Arc::new(Mutex::new(LoroDoc::new())),
            write_socket: write_framed,
            read_socket: Some(read_framed),
        }
    }

    pub async fn broadcast_changes(&mut self) {
        let message = Message {
            data: self.doc.lock().await.export_from(&Default::default()),
        };
        self.write_socket.send(message).await.unwrap();
    }

    pub async fn insert_string(&mut self, idx: usize, s: &str) {
        let doc = self.doc.lock().await;
        doc.get_text("text").insert(idx, s).unwrap();
    }

    pub async fn delete_string(&mut self, idx: usize, len: usize) {
        let doc = self.doc.lock().await;
        doc.get_text("text").delete(idx, len).unwrap();
    }

    pub fn begin_update_task(&mut self) {
        let mut socket = self.read_socket.take().unwrap();
        let data = self.doc.clone();

        tokio::spawn(async move {
            while let Some(message) = socket.try_next().await.unwrap() {
                println!("Received message: {:?}", message);
                data.lock().await.import(&message.data).unwrap();
            }
        });
    }

    pub async fn read(&self) -> String {
        self.doc.lock().await.get_text("text").to_string()
    }
}
