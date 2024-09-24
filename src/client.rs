mod channels;
mod tasks;
mod utils;

use channels::{Channels, MainTaskMessage, OutgoingMessage};
use loro::{cursor::Cursor, LoroDoc, SubID};
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
    SetCursor {
        document_id: String,
        location: usize,
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
    channels: Channels,
    main_channel_rx: Receiver<MainTaskMessage>,
    active_documents: HashMap<String, DocumentInfo>,
}

impl Client {
    pub async fn begin_event_loop(mut self) {
        info!("Entering main event loop");

        while let Ok(message) = self.main_channel_rx.try_recv() {
            match message {
                MainTaskMessage::NewConnection(connection) => {
                    self.accept_new_connection(connection).await;
                }
                MainTaskMessage::ClientMessage(c_message) => {
                    self.handle_stdin_message(c_message).await;
                }
                MainTaskMessage::DocumentData(data) => {
                    info!("Main task importing data");
                    self.doc.import(&data).unwrap();
                }
                MainTaskMessage::UpdateCursor(id) => {
                    info!("Updating cursor locations");
                    if let Some(ref cursor) = self.active_documents.get(&id).unwrap().cursor {
                        self.channels
                            .stdout_tx
                            .send(ClientMessage::SetCursor {
                                document_id: id,
                                location: self.doc.get_cursor_pos(cursor).unwrap().current.pos,
                            })
                            .await
                            .unwrap();
                    }
                }
            }
        }
    }

    fn new(builder: ClientBuilder) -> Self {
        let listener = builder.listener;

        // Setup tasks
        let (main_task_channel_tx, main_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (stdout_task_channel_tx, stdout_task_channel_rx) = tokio::sync::mpsc::channel(10);
        let (incoming_task_to_channel_tx, incoming_task_to_channel_rx) =
            tokio::sync::mpsc::channel(1);
        let (outgoing_task_channel_tx, outgoing_task_channel_rx) = tokio::sync::mpsc::channel(10);
        info!("Channels created");

        let channels = Channels {
            main_tx: main_task_channel_tx.clone(),
            incoming_to_tx: incoming_task_to_channel_tx,
            outgoing_tx: outgoing_task_channel_tx,
            stdout_tx: stdout_task_channel_tx,
        };

        begin_incoming_task(main_task_channel_tx.clone(), incoming_task_to_channel_rx);
        begin_outgoing_task(outgoing_task_channel_rx);
        begin_stdin_task(channels.main_tx.clone());
        begin_stdout_task(stdout_task_channel_rx);
        begin_listening_task(listener, main_task_channel_tx.clone());
        info!("Tasks started");

        Client {
            doc: LoroDoc::new(),
            channels,
            main_channel_rx: main_task_channel_rx,
            active_documents: HashMap::new(),
        }
    }

    fn add_doc_change_subscription(&mut self, id: &str) -> SubID {
        let c_id = self.doc.get_text(id).id();
        let id = id.to_owned();
        let channel = self.channels.stdout_tx.clone();
        let notify_channel = self.channels.main_tx.clone();
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
                let notify_channel = notify_channel.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    for change in changes {
                        let message = ClientMessage::Change {
                            document_id: id.clone(),
                            change,
                        };
                        stdout_task_channel_tx.send(message).await.unwrap();
                    }

                    notify_channel
                        .send(MainTaskMessage::UpdateCursor(id))
                        .await
                        .unwrap();
                });
            }),
        )
    }

    async fn broadcast_all_data(&mut self) {
        self.channels
            .outgoing_tx
            .send(OutgoingMessage::DocumentData(
                self.doc.export_from(&Default::default()),
            ))
            .await
            .unwrap();
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
        self.broadcast_all_data().await;
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
                self.broadcast_all_data().await;
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
                self.active_documents.insert(
                    id.clone(),
                    DocumentInfo {
                        sub_id: subscription,
                        cursor: None,
                    },
                );

                info!("Created new document with id {}", id);

                self.broadcast_all_data().await;
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
                self.active_documents.insert(
                    id.clone(),
                    DocumentInfo {
                        sub_id: subscription,
                        cursor: None,
                    },
                );

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
            ClientMessage::SetCursor {
                document_id,
                location,
            } => {
                let doc_info = self.active_documents.get_mut(&document_id).unwrap();
                let text = self.doc.get_text(document_id.as_str());
                doc_info.cursor = text.get_cursor(location, Default::default());

                // TODO Cursors for other users.
            }
        }
    }
}

struct DocumentInfo {
    sub_id: SubID,
    cursor: Option<Cursor>,
}
