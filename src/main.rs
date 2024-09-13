use crdts::{list::Op, CmRDT, List};
use futures::{SinkExt, TryStreamExt};
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
    let id: usize;

    let args = std::env::args().collect::<Vec<String>>();
    let server = args.contains(&String::from("server"));
    if server {
        let listener = TcpListener::bind("0.0.0.0:6969").await.unwrap();
        stream = listener.accept().await.unwrap().0;
        id = 1;
    } else {
        let mut address = String::new();
        io::stdin().read_line(&mut address).unwrap();
        stream = TcpStream::connect(address.trim()).await.unwrap();
        id = 2;
    }

    let mut text = Text::new(id, stream);

    text.begin_update_task();

    if server {
        text.append_string(String::from("Hello world")).await;
    } else {
        text.append_string(String::from("Foobar")).await;
    }

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    println!("After update: {:?}", text.read().await);

    // if server {
    //     let mut s = String::new();
    //     stream.read_to_string(&mut s).unwrap();
    //     println!("{:?}", s);
    // } else {
    //     write!(stream, "Hello world").unwrap();
    // }
    //
    // "Hello world"
    //     .chars()
    //     .for_each(|c| text.apply(text.append(c, 1)));

    //     (6..11)
    //     .rev()
    //     .for_each(|i| text.apply(text.delete_index(i, 1).unwrap()));
    // "mom".chars().for_each(|c| text.apply(text.append(c, 1)));
    //
    // text.apply(text.delete_index(4, 2).unwrap());
    //
    // let s: String = text.read();
    // println!("{:?}", s);
}

struct Text {
    id: usize,
    data: Arc<Mutex<List<char, usize>>>,
    // I hate Rust sometimes.
    write_socket: tokio_serde::SymmetricallyFramed<
        FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
        Op<char, usize>,
        SymmetricalJson<Op<char, usize>>,
    >,
    read_socket: Option<
        tokio_serde::SymmetricallyFramed<
            FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
            Op<char, usize>,
            SymmetricalJson<Op<char, usize>>,
        >,
    >,
}

impl Text {
    fn new(id: usize, socket: TcpStream) -> Self {
        socket.set_nodelay(true).unwrap();
        let (read, write) = socket.into_split();
        let read_framed = tokio_serde::SymmetricallyFramed::new(
            FramedRead::new(read, LengthDelimitedCodec::new()),
            SymmetricalJson::<Op<char, usize>>::default(),
        );
        let write_framed = tokio_serde::SymmetricallyFramed::new(
            FramedWrite::new(write, LengthDelimitedCodec::new()),
            SymmetricalJson::<Op<char, usize>>::default(),
        );
        Text {
            id,
            data: Arc::new(Mutex::new(List::new())),
            write_socket: write_framed,
            read_socket: Some(read_framed),
        }
    }

    async fn apply_op(&mut self, op: Op<char, usize>) {
        self.data.lock().await.apply(op);
    }

    async fn broadcast_op(&mut self, op: Op<char, usize>) {
        self.write_socket.send(op).await.unwrap();
    }

    async fn append_string(&mut self, s: String) {
        let mut ops: Vec<Op<_, _>> = Vec::new();

        for c in s.chars() {
            let op = self.data.lock().await.append(c, self.id);
            self.apply_op(op.clone()).await;
            ops.push(op);
        }

        for op in ops {
            self.broadcast_op(op).await;
        }
    }

    fn begin_update_task(&mut self) {
        let mut socket = self.read_socket.take().unwrap();
        let data = self.data.clone();

        tokio::spawn(async move {
            while let Some(op) = socket.try_next().await.unwrap() {
                println!("Received op: {:?}", op);
                data.lock().await.apply(op);
            }
        });
    }

    async fn read(&self) -> String {
        self.data.lock().await.iter().collect()
    }
}
