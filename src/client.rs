mod channels;
mod tasks;
mod utils;

use channels::{Channels, OutgoingMessage};
use loro::{LoroDoc, SubID};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tasks::*;
use tokio::{
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpListener, TcpStream,
    },
    sync::mpsc::Receiver,
};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tracing::{error, info};
use utils::*;

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

pub struct ClientBuilder {
    listener: TcpListener,
}

impl ClientBuilder {
    pub fn new(listener: TcpListener) -> Self {
        ClientBuilder { listener }
    }

    pub fn build(self) -> Client {
        Client::new(self)
    }
}

pub struct Client {
    doc: LoroDoc,
    listener: TcpListener,
    channels: Channels,
    stdin_rx: Receiver<ClientMessage>,
    network_rx: Receiver<Vec<u8>>,
    active_documents: HashMap<String, SubID>,
}

impl Client {
    pub async fn begin_event_loop(mut self) {
        info!("Entering main event loop");

        loop {
            tokio::select! {
                Ok(socket) = self.listener.accept() => {
                    self.accept_new_connection(socket).await;
                }

                Some(message) = self.stdin_rx.recv() => {
                    self.handle_stdin_message(message).await;
                }

                Some(data) = self.network_rx.recv() => {
                    info!("Main task importing data");
                    self.doc.import(&data).unwrap();
                }
            }
        }
    }

    fn new(builder: ClientBuilder) -> Self {
        let listener = builder.listener;

        // Setup tasks
        let (stdin_task_channel_tx, stdin_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (stdout_task_channel_tx, stdout_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (incoming_task_from_channel_tx, incoming_task_from_channel_rx) =
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

        Client {
            doc: LoroDoc::new(),
            listener,
            channels,
            stdin_rx: stdin_task_channel_rx,
            network_rx: incoming_task_from_channel_rx,
            active_documents: HashMap::new(),
        }
    }

    fn add_doc_change_subscription(&mut self, id: &str) -> SubID {
        let c_id = self.doc.get_text(id).id();
        let id = id.to_owned();
        let channel = self.channels.stdout_tx.clone();
        self.doc.subscribe(
            &c_id,
            Arc::new(move |change| {
                if !change.triggered_by.is_import() {
                    return;
                }

                let changes = diffs_to_changes(&change.events);

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

    async fn accept_new_connection(&mut self, (socket, addr): (TcpStream, std::net::SocketAddr)) {
        let (read, write) = socket.into_split();

        let read_framed = tokio_serde::SymmetricallyFramed::new(
            FramedRead::new(read, LengthDelimitedCodec::new()),
            SymmetricalJson::<BackendMessage>::default(),
        );
        let write_framed = tokio_serde::SymmetricallyFramed::new(
            FramedWrite::new(write, LengthDelimitedCodec::new()),
            SymmetricalJson::<BackendMessage>::default(),
        );

        self.channels
            .incoming_to_tx
            .send(read_framed)
            .await
            .unwrap();
        self.channels
            .outgoing_tx
            .send(OutgoingMessage::NewSocket(write_framed))
            .await
            .unwrap();

        info!("Accepted connection from peer at {}", addr);
        self.channels
            .stdout_tx
            .send(ClientMessage::AddPeerResponse {
                address: addr.to_string(),
            })
            .await
            .unwrap();
    }

    async fn handle_stdin_message(&mut self, message: ClientMessage) {
        info!("Main task received from stdin: {:?}", message);

        match message {
            // Messages that should only ever be sent to the self.
            ClientMessage::AddPeerResponse { .. }
            | ClientMessage::CreateDocumentResponse { .. }
            | ClientMessage::JoinDocumentResponse { .. } => {
                error!(
                    "Received message which should only be sent to the self: {:?}",
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

                self.channels
                    .incoming_to_tx
                    .send(read_framed)
                    .await
                    .unwrap();
                self.channels
                    .outgoing_tx
                    .send(OutgoingMessage::NewSocket(write_framed))
                    .await
                    .unwrap();

                info!("Connected to peer at {}", address);
                self.channels
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
                        self.doc.get_text(document_id).insert(index, &text).unwrap();
                    }
                    Change::Delete { index, len } => {
                        self.doc.get_text(document_id).delete(index, len).unwrap();
                    }
                }

                self.channels
                    .outgoing_tx
                    .send(OutgoingMessage::DocumentData(
                        self.doc.export_from(&Default::default()),
                    ))
                    .await
                    .unwrap();
            }
            ClientMessage::CreateDocument {
                name,
                initial_content,
            } => {
                let id = generate_unique_id(&name, &mut self.doc);

                self.doc.get_text(id.as_str()).update(&initial_content);

                let subscription = self.add_doc_change_subscription(&id);
                self.active_documents.insert(id.clone(), subscription);

                info!("Created new document with id {}", id);

                self.channels
                    .outgoing_tx
                    .send(OutgoingMessage::DocumentData(
                        self.doc.export_from(&Default::default()),
                    ))
                    .await
                    .unwrap();
                self.channels
                    .stdout_tx
                    .send(ClientMessage::CreateDocumentResponse { id })
                    .await
                    .unwrap();
            }
            ClientMessage::JoinDocument { id } => {
                if self.active_documents.contains_key(&id) {
                    error!(
                        "Client attempted to join document that is already active: {}",
                        id
                    );
                    // TODO Broadcast error to frontend.

                    return;
                }
                if self.doc.get_text(id.as_str()).is_empty() {
                    error!("Client attempted to join document with no content: {}", id);
                    // TODO Broadcast error to frontend.

                    return;
                }

                let subscription = self.add_doc_change_subscription(&id);
                self.active_documents.insert(id.clone(), subscription);

                info!("Joined document with id {}", id);

                self.channels
                    .stdout_tx
                    .send(ClientMessage::JoinDocumentResponse {
                        id: id.clone(),
                        current_content: self.doc.get_text(id.as_str()).to_string(),
                    })
                    .await
                    .unwrap();
            }
        }
    }
}
