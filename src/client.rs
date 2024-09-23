mod channels;

use channels::{Channels, OutgoingMessage};
use futures::{SinkExt, TryStreamExt};
use loro::{LoroDoc, SubID, TextDelta};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io::Write, sync::Arc};
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
use tracing::{error, info};

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
    AddPeer {
        address: String,
    },
    AddPeerResponse {
        address: String,
    },
    CreateDocument {
        name: String,
        initial_content: String,
    },
    CreateDocumentResponse {
        id: String,
    },
    Change {
        document_id: String,
        change: Change,
    },
    JoinDocument {
        id: String,
    },
    JoinDocumentResponse {
        id: String,
        current_content: String,
    },
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
    active_documents: HashMap<String, SubID>,
}

impl Client {
    pub fn new(listener: TcpListener) -> Self {
        Client {
            doc: LoroDoc::new(),
            listener,
            active_documents: HashMap::new(),
        }
    }

    pub async fn begin_event_loop(mut self) {
        let (stdin_task_channel_tx, mut stdin_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (stdout_task_channel_tx, stdout_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (incoming_task_from_channel_tx, mut incoming_task_from_channel_rx) =
            tokio::sync::mpsc::channel(10);
        let (incoming_task_to_channel_tx, incoming_task_to_channel_rx) =
            tokio::sync::mpsc::channel(1);
        let (outgoing_task_channel_tx, outgoing_task_channel_rx) = tokio::sync::mpsc::channel(10);
        info!("Channels created");

        let channels = Channels {
            stdin_tx: stdin_task_channel_tx,
            stdout_tx: stdout_task_channel_tx,
            incoming_to_tx: incoming_task_to_channel_tx,
            outgoing_tx: outgoing_task_channel_tx,
        };

        begin_incoming_task(incoming_task_from_channel_tx, incoming_task_to_channel_rx);
        begin_outgoing_task(outgoing_task_channel_rx);
        begin_stdin_task(channels.stdin_tx.clone());
        begin_stdout_task(stdout_task_channel_rx);
        info!("Tasks started");

        info!("Entering main event loop");
        loop {
            tokio::select! {
                Ok(socket) = self.listener.accept() => {
                    accept_new_connection(
                        socket,
                        channels.clone(),
                    ).await;
                }

                Some(message) = stdin_task_channel_rx.recv() => {
                    handle_stdin_message(&mut self, channels.clone(), message).await;
                }

                Some(data) = incoming_task_from_channel_rx.recv() => {
                    info!("Main task importing data");
                    self.doc.import(&data).unwrap();
                }
            }
        }
    }
}

fn begin_incoming_task(tx: Sender<Vec<u8>>, mut rx: Receiver<ReadSocket>) {
    tokio::spawn(async move {
        while let Some(mut socket) = rx.recv().await {
            let tx = tx.clone();

            // TODO store join handles so we can cancel tasks when disconnecting.
            tokio::spawn(async move {
                while let Some(message) = socket.try_next().await.unwrap() {
                    info!("Received from network: {:?}", message);
                    let BackendMessage::DocumentSync { data } = message;
                    tx.send(data).await.unwrap();
                }
            });
        }
    });
}

fn begin_outgoing_task(mut rx: Receiver<OutgoingMessage>) {
    tokio::spawn(async move {
        let mut sockets = Vec::new();

        loop {
            if let Some(message) = rx.recv().await {
                match message {
                    OutgoingMessage::NewSocket(socket) => {
                        sockets.push(socket);
                    }
                    OutgoingMessage::DocumentData(data) => {
                        let message = BackendMessage::DocumentSync { data };
                        info!("Sending to network: {:?}", message);

                        for socket in sockets.iter_mut() {
                            socket.send(message.clone()).await.unwrap();
                        }
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
            info!("Received message from stdin: {}", line);
            let message = serde_json::from_str::<ClientMessage>(&line).unwrap();
            tx.send(message).await.unwrap();
        }
    });
}

fn begin_stdout_task(mut rx: Receiver<ClientMessage>) {
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let serialized = serde_json::to_string(&message).unwrap();
            info!("Sending message to stdout: {:?}", serialized);
            // TODO should this be using Tokio's stdout?
            let mut stdout = std::io::stdout();
            stdout.write_all(serialized.as_bytes()).unwrap();
            stdout.write_all(b"\n").unwrap();
        }
    });
}

fn add_doc_change_subscription(
    doc: &mut LoroDoc,
    id: &str,
    channel: Sender<ClientMessage>,
) -> SubID {
    let c_id = doc.get_text(id).id();
    let id = id.to_owned();
    doc.subscribe(
        &c_id,
        Arc::new(move |change| {
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
            let id = id.clone();
            tokio::spawn(async move {
                for change in changes {
                    let message = ClientMessage::Change {
                        document_id: id.clone(),
                        change,
                    };
                    stdout_task_channel_tx.send(message).await.unwrap();
                }
            });
        }),
    )
}

async fn accept_new_connection(
    (socket, addr): (TcpStream, std::net::SocketAddr),
    channels: Channels,
) {
    let (read, write) = socket.into_split();

    let read_framed = tokio_serde::SymmetricallyFramed::new(
        FramedRead::new(read, LengthDelimitedCodec::new()),
        SymmetricalJson::<BackendMessage>::default(),
    );
    let write_framed = tokio_serde::SymmetricallyFramed::new(
        FramedWrite::new(write, LengthDelimitedCodec::new()),
        SymmetricalJson::<BackendMessage>::default(),
    );

    channels.incoming_to_tx.send(read_framed).await.unwrap();
    channels
        .outgoing_tx
        .send(OutgoingMessage::NewSocket(write_framed))
        .await
        .unwrap();

    info!("Accepted connection from peer at {}", addr);
    channels
        .stdout_tx
        .send(ClientMessage::AddPeerResponse {
            address: addr.to_string(),
        })
        .await
        .unwrap();
}

async fn handle_stdin_message(client: &mut Client, channels: Channels, message: ClientMessage) {
    info!("Main task received from stdin: {:?}", message);

    match message {
        // Messages that should only ever be sent to the client.
        ClientMessage::AddPeerResponse { .. }
        | ClientMessage::CreateDocumentResponse { .. }
        | ClientMessage::JoinDocumentResponse { .. } => {
            error!(
                "Received message which should only be sent to the client: {:?}",
                message
            );
        }
        ClientMessage::AddPeer { address } => {
            info!("Connecting to peer at {}", address);
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

            channels.incoming_to_tx.send(read_framed).await.unwrap();
            channels
                .outgoing_tx
                .send(OutgoingMessage::NewSocket(write_framed))
                .await
                .unwrap();

            info!("Connected to peer at {}", address);
            channels
                .stdout_tx
                .send(ClientMessage::AddPeerResponse { address })
                .await
                .unwrap();
        }
        ClientMessage::Change {
            document_id,
            change,
        } => {
            match change {
                Change::Insert { index, text } => {
                    client
                        .doc
                        .get_text(document_id)
                        .insert(index, &text)
                        .unwrap();
                }
                Change::Delete { index, len } => {
                    client.doc.get_text(document_id).delete(index, len).unwrap();
                }
            }

            channels
                .outgoing_tx
                .send(OutgoingMessage::DocumentData(
                    client.doc.export_from(&Default::default()),
                ))
                .await
                .unwrap();
        }
        ClientMessage::CreateDocument {
            name,
            initial_content,
        } => {
            let id = generate_unique_id(&name, &mut client.doc);

            client.doc.get_text(id.as_str()).update(&initial_content);
            client.active_documents.insert(
                id.clone(),
                add_doc_change_subscription(&mut client.doc, &id, channels.stdout_tx.clone()),
            );

            info!("Created new document with id {}", id);

            channels
                .outgoing_tx
                .send(OutgoingMessage::DocumentData(
                    client.doc.export_from(&Default::default()),
                ))
                .await
                .unwrap();
            channels
                .stdout_tx
                .send(ClientMessage::CreateDocumentResponse { id })
                .await
                .unwrap();
        }
        ClientMessage::JoinDocument { id } => {
            if client.active_documents.contains_key(&id) {
                error!(
                    "Client attempted to join document that is already active: {}",
                    id
                );
                // TODO Broadcast error to frontend.

                return;
            }
            if client.doc.get_text(id.as_str()).is_empty() {
                error!("Client attempted to join document with no content: {}", id);
                // TODO Broadcast error to frontend.

                return;
            }

            client.active_documents.insert(
                id.clone(),
                add_doc_change_subscription(&mut client.doc, &id, channels.stdout_tx.clone()),
            );
            info!("Joined document with id {}", id);

            channels
                .stdout_tx
                .send(ClientMessage::JoinDocumentResponse {
                    id: id.clone(),
                    current_content: client.doc.get_text(id.as_str()).to_string(),
                })
                .await
                .unwrap();
        }
    }
}

fn generate_unique_id(name: &str, doc: &mut LoroDoc) -> String {
    let mut i = 0;
    let mut unique_name = name.to_string();

    while !doc.get_text(unique_name.as_str()).is_empty() {
        i += 1;
        unique_name = format!("{} {}", name, i);
    }

    unique_name
}
