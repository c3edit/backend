use futures::{SinkExt, TryStreamExt};
use loro::{LoroDoc, TextDelta};
use serde::{Deserialize, Serialize};
use std::{io::Write, sync::Arc};
use tokio::{
    io::{self, AsyncBufReadExt, BufReader},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::mpsc::{Receiver, Sender},
};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tracing::{debug, error};

// I hate Rust sometimes.
type WriteSocket = tokio_serde::SymmetricallyFramed<
    FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
    BackendMessage,
    SymmetricalJson<BackendMessage>,
>;
type ReadSocket = tokio_serde::SymmetricallyFramed<
    FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    BackendMessage,
    SymmetricalJson<BackendMessage>,
>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "snake_case"))]
#[serde(tag = "type")]
enum ClientMessage {
    AddPeer { address: String },
    PeerAdded { address: String },
    CreateDocument { initial_content: String },
    Change { change: Change },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "snake_case"))]
#[serde(tag = "type")]
enum Change {
    Insert { index: usize, text: String },
    Delete { index: usize, len: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum BackendMessage {
    DocumentSync { data: Vec<u8> },
}

pub struct Client {
    doc: LoroDoc,
    listener: TcpListener,
}

impl Client {
    pub fn new(listener: TcpListener) -> Self {
        Client {
            doc: LoroDoc::new(),
            listener,
        }
    }

    pub async fn begin_event_loop(mut self) {
        let (stdin_task_channel_tx, mut stdin_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (stdout_task_channel_tx, stdout_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (incoming_task_channel_tx, mut incoming_task_channel_rx) =
            tokio::sync::mpsc::channel(10);
        let (incoming_task_socket_channel_tx, incoming_task_socket_channel_rx) =
            tokio::sync::mpsc::channel(1);
        let (outgoing_task_channel_tx, outgoing_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (outgoing_task_socket_channel_tx, outgoing_task_socket_channel_rx) =
            tokio::sync::mpsc::channel(1);
        debug!("Channels created");

        begin_incoming_task(incoming_task_channel_tx, incoming_task_socket_channel_rx);
        begin_outgoing_task(outgoing_task_channel_rx, outgoing_task_socket_channel_rx);
        begin_stdin_task(stdin_task_channel_tx);
        begin_stdout_task(stdout_task_channel_rx);
        debug!("Tasks started");

        let channel = stdout_task_channel_tx.clone();
        add_doc_change_subsription(&mut self.doc, channel);
        debug!("Subscribed to document");

        debug!("Entering main event loop");
        loop {
            tokio::select! {
                Ok((socket, addr)) = self.listener.accept() => {
                    let (read, write) = socket.into_split();

                    let read_framed = tokio_serde::SymmetricallyFramed::new(
                        FramedRead::new(read, LengthDelimitedCodec::new()),
                        SymmetricalJson::<BackendMessage>::default(),
                    );
                    let write_framed = tokio_serde::SymmetricallyFramed::new(
                        FramedWrite::new(write, LengthDelimitedCodec::new()),
                        SymmetricalJson::<BackendMessage>::default(),
                    );

                    incoming_task_socket_channel_tx.send(read_framed).await.unwrap();
                    outgoing_task_socket_channel_tx.send(write_framed).await.unwrap();

                    debug!("Accepted connection from peer at {}", addr);
                    stdout_task_channel_tx.send(ClientMessage::PeerAdded{address: addr.to_string()}).await.unwrap();
                },
                Some(message) = stdin_task_channel_rx.recv() => {
                    debug!("Main task received from stdin: {:?}", message);

                    match message {
                        // Messages that should only ever be sent to the client.
                        ClientMessage::PeerAdded{..} => {
                            error!("Received message which should only be sent to the client: {:?}", message);
                        },
                        ClientMessage::AddPeer{address} => {
                            debug!("Connecting to peer at {}", address);
                            let socket = TcpStream::connect(&address).await.unwrap();
                            socket.set_nodelay(true).unwrap();

                            let (read, write) = socket.into_split();
                            let read_framed = tokio_serde::SymmetricallyFramed::new(
                                FramedRead::new(read, LengthDelimitedCodec::new()),
                                SymmetricalJson::<BackendMessage>::default(),
                            );
                            let write_framed = tokio_serde::SymmetricallyFramed::new(
                                FramedWrite::new(write, LengthDelimitedCodec::new()),
                                SymmetricalJson::<BackendMessage>::default(),
                            );

                            incoming_task_socket_channel_tx.send(read_framed).await.unwrap();
                            outgoing_task_socket_channel_tx.send(write_framed).await.unwrap();

                            debug!("Connected to peer at {}", address);
                            stdout_task_channel_tx.send(ClientMessage::PeerAdded{address}).await.unwrap();
                        },
                        ClientMessage::Change{change} =>  {
                            match change {
                                Change::Insert { index, text } => {
                                    self.doc.get_text("text").insert(index, &text).unwrap();
                                }
                                Change::Delete { index, len } => {
                                    self.doc.get_text("text").delete(index, len).unwrap();
                                }
                            }

                            outgoing_task_channel_tx
                            .send(self.doc.export_from(&Default::default()))
                            .await
                            .unwrap();
                        },
                        ClientMessage::CreateDocument{initial_content} => {
                            self.doc.get_text("text").update(&initial_content);
                            outgoing_task_channel_tx
                            .send(self.doc.export_from(&Default::default()))
                            .await
                            .unwrap();
                        }
                    }
                },

                Some(data) = incoming_task_channel_rx.recv() => {
                    debug!("Main task importing data");
                    self.doc.import(&data).unwrap();
                }
            }
        }
    }
}

fn begin_incoming_task(tx: Sender<Vec<u8>>, mut socket_rx: Receiver<ReadSocket>) {
    tokio::spawn(async move {
        while let Some(mut socket) = socket_rx.recv().await {
            let tx = tx.clone();

            // TODO store join handles so we can cancel tasks when disconnecting.
            tokio::spawn(async move {
                while let Some(message) = socket.try_next().await.unwrap() {
                    debug!("Received from network: {:?}", message);
                    let BackendMessage::DocumentSync { data } = message;
                    tx.send(data).await.unwrap();
                }
            });
        }
    });
}

fn begin_outgoing_task(mut rx: Receiver<Vec<u8>>, mut socket_rx: Receiver<WriteSocket>) {
    tokio::spawn(async move {
        let mut sockets = Vec::new();

        loop {
            tokio::select! {
                Some(socket) = socket_rx.recv() => {
                    sockets.push(socket);
                },
                Some(data) = rx.recv() => {
                    let message = BackendMessage::DocumentSync { data };
                    debug!("Sending to network: {:?}", message);

                    for socket in sockets.iter_mut() {
                        socket.send(message.clone()).await.unwrap();
                    }
                }
            }
        }
    });
}

fn begin_stdin_task(tx: Sender<ClientMessage>) {
    tokio::spawn(async move {
        let stdin = BufReader::new(io::stdin());
        let mut lines = stdin.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            let message = serde_json::from_str::<ClientMessage>(&line).unwrap();
            debug!("Received message from stdin: {:?}", message);
            tx.send(message).await.unwrap();
        }
    });
}

fn begin_stdout_task(mut rx: Receiver<ClientMessage>) {
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let serialized = serde_json::to_string(&message).unwrap();
            debug!("Sending message to stdout: {:?}", serialized);
            // TODO should this be using Tokio's stdout?
            let mut stdout = std::io::stdout();
            stdout.write_all(serialized.as_bytes()).unwrap();
            stdout.write_all(b"\n").unwrap();
        }
    });
}

fn add_doc_change_subsription(doc: &mut LoroDoc, channel: Sender<ClientMessage>) {
    doc.subscribe_root(Arc::new(move |change| {
        if !change.triggered_by.is_import() {
            return;
        }

        let mut changes = Vec::new();
        for event in change.events {
            let diffs = event.diff.as_text().unwrap();
            let mut index = 0;

            for diff in diffs {
                match diff {
                    TextDelta::Retain { retain, .. } => {
                        index += retain;
                    }
                    TextDelta::Insert { insert, .. } => {
                        changes.push(Change::Insert {
                            index,
                            text: insert.to_string(),
                        });
                    }
                    TextDelta::Delete { delete, .. } => {
                        changes.push(Change::Delete {
                            index,
                            len: *delete,
                        });
                    }
                }
            }
        }

        // We have to spawn a new task here because this callback can't
        // be async, and we can't use `blocking_send` because this runs
        // inside a Tokio thread, which should never block (and will
        // panic if it does).
        let stdout_task_channel_tx = channel.clone();
        tokio::spawn(async move {
            for change in changes {
                let message = ClientMessage::Change { change };
                stdout_task_channel_tx.send(message).await.unwrap();
            }
        });
    }));
}
