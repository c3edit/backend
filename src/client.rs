mod channels;
mod tasks;
mod types;
mod utils;

use channels::{Channels, MainTaskMessage, OutgoingMessage};
use std::{collections::HashMap, sync::Arc};
use tasks::*;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::mpsc::Receiver,
};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};
use tracing::{error, info};
use types::*;
use utils::*;

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
    doc: loro::LoroDoc,
    channels: Channels,
    main_channel_rx: Receiver<MainTaskMessage>,
    active_documents: HashMap<DocumentID, DocumentInfo>,
}

impl Client {
    pub async fn begin_event_loop(mut self) {
        info!("Entering main event loop");

        while let Some(message) = self.main_channel_rx.recv().await {
            match message {
                MainTaskMessage::NewConnection(connection) => {
                    self.accept_new_connection(connection).await;
                }
                MainTaskMessage::ClientMessage(c_message) => {
                    self.handle_client_message(c_message).await;
                }
                MainTaskMessage::BackendMessage(data) => {
                    self.handle_backend_message(data).await;
                }
                MainTaskMessage::DocumentChanged(id) => {
                    info!("Updating cursor locations for document {}", id);

                    let doc_info = self.active_documents.get(&id).unwrap();

                    self.broadcast_cursor_update(&id).await;
                    for peer_id in doc_info.cursors.keys() {
                        self.update_frontend_cursor(&id, Some(*peer_id)).await;
                    }
                    // for peer_id in doc_info.marks.keys() {
                    //     self.update_frontend_cursor(&id, Some(*peer_id), true).await;
                    // }
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
            doc: loro::LoroDoc::new(),
            channels,
            main_channel_rx: main_task_channel_rx,
            active_documents: HashMap::new(),
        }
    }

    fn add_doc_change_subscription(&mut self, id: &str) -> loro::SubID {
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
                        .send(MainTaskMessage::DocumentChanged(id))
                        .await
                        .unwrap();
                });
            }),
        )
    }

    async fn broadcast_cursor_update(&self, document_id: &str) {
        let doc_info = self.active_documents.get(document_id).unwrap();
        let peer_id = self.doc.peer_id();

        if let Some(ref cursor) = doc_info.cursor {
            self.channels
                .outgoing_tx
                .send(OutgoingMessage::BackendMessage(
                    BackendMessage::CursorUpdate {
                        document_id: document_id.to_owned(),
                        peer_id,
                        cursor: cursor.clone(),
                    },
                ))
                .await
                .unwrap();
        }
    }

    async fn broadcast_all_data(&mut self) {
        self.channels
            .outgoing_tx
            .send(OutgoingMessage::BackendMessage(
                BackendMessage::DocumentSync {
                    data: self.doc.export_from(&Default::default()),
                },
            ))
            .await
            .unwrap();

        for id in self.active_documents.keys() {
            self.broadcast_cursor_update(id).await;
        }
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

    // TODO Refactor
    async fn update_frontend_cursor(&self, document_id: &str, peer_id: Option<loro::PeerID>) {
        let doc_info = self.active_documents.get(document_id).unwrap();
        let cursor = if let Some(peer_id) = peer_id {
            doc_info.cursors.get(&peer_id)
        } else {
            doc_info.cursor.as_ref()
        };
        let Some(cursor) = cursor else {
            error!(
                "Attempted to update non-existent cursor for {} for peer (is this expected?): {:?}",
                document_id, peer_id
            );
            return;
        };

        let pos = self.doc.get_cursor_pos(cursor).unwrap().current.pos;
        self.channels
            .stdout_tx
            .send(ClientMessage::SetCursor {
                document_id: document_id.to_owned(),
                peer_id,
                location: pos,
            })
            .await
            .unwrap();

        // let mark = if let Some(peer_id) = peer_id {
        //     doc_info.marks.get(&peer_id)
        // } else {
        //     doc_info.mark.as_ref()
        // };

        //          if let Some(mark) = mark {
        //     let pos = self.doc.get_cursor_pos(mark).unwrap().current.pos;
        //     self.channels
        //         .stdout_tx
        //         .send(ClientMessage::SetCursor {
        //             document_id: document_id.to_owned(),
        //             peer_id,
        //             location: pos,
        //             mark: true,
        //         })
        //         .await
        //         .unwrap();
        // } else {
        //     self.channels
        //         .stdout_tx
        //         .send(ClientMessage::UnsetMark {
        //             document_id: document_id.to_owned(),
        //             peer_id,
        //         })
        //         .await
        //         .unwrap();
        // }
    }

    async fn handle_client_message(&mut self, message: ClientMessage) {
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
                        self.doc
                            .get_text(document_id.as_str())
                            .insert(index, &text)
                            .unwrap();
                    }
                    Change::Delete { index, len } => {
                        self.doc
                            .get_text(document_id.as_str())
                            .delete(index, len)
                            .unwrap();
                    }
                }

                // TODO Only send deltas to other clients.
                self.broadcast_all_data().await;
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
                    // TODO Use Default trait
                    DocumentInfo {
                        sub_id: subscription,
                        cursor: None,
                        mark: None,
                        cursors: HashMap::new(),
                        marks: HashMap::new(),
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
                        mark: None,
                        cursors: HashMap::new(),
                        marks: HashMap::new(),
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
                ..
            } => {
                let doc_info = self.active_documents.get_mut(&document_id).unwrap();
                let text = self.doc.get_text(document_id.as_str());

                // doc_info.mark = text.get_cursor(location, Default::default());

                doc_info.cursor = text.get_cursor(location, Default::default());

                self.broadcast_cursor_update(&document_id).await;
            }
            ClientMessage::SetSelection {
                document_id,
                peer_id,
                point,
                mark,
            } => {
                todo!()
            }
            ClientMessage::UnsetMark {
                document_id,
                peer_id,
            } => {
                let doc_info = self.active_documents.get_mut(&document_id).unwrap();

                if let Some(peer_id) = peer_id {
                    doc_info.marks.remove(&peer_id);
                } else {
                    doc_info.mark = None;
                }

                self.broadcast_cursor_update(&document_id).await;
            }
            ClientMessage::UnsetSelection {
                document_id,
                peer_id,
            } => {
                todo!()
            }
        }
    }

    async fn handle_backend_message(&mut self, message: BackendMessage) {
        match message {
            BackendMessage::DocumentSync { data } => {
                info!("Received document sync data");
                self.doc.import(&data).unwrap();
            }
            BackendMessage::CursorUpdate {
                document_id,
                peer_id,
                cursor,
                ..
            } => {
                info!("Received cursor update for document {}", document_id,);

                let Some(doc_info) = self.active_documents.get_mut(&document_id) else {
                    // Document not active.
                    return;
                };

                // doc_info.marks.insert(peer_id, cursor);

                doc_info.cursors.insert(peer_id, cursor);

                self.update_frontend_cursor(&document_id, Some(peer_id))
                    .await;
            }
            BackendMessage::SelectionUpdate {
                document_id,
                peer_id,
                point,
                mark,
            } => {
                todo!()
            }
            BackendMessage::UnsetMark {
                document_id,
                peer_id,
            } => {
                info!(
                    "Received unset mark for document {} from peer {}",
                    document_id, peer_id
                );

                let Some(doc_info) = self.active_documents.get_mut(&document_id) else {
                    // Document not active.
                    return;
                };

                doc_info.marks.remove(&peer_id);

                // self.update_frontend_cursor(&document_id, Some(peer_id), true)
                //     .await;
            }
            BackendMessage::UnsetSelection {
                document_id,
                peer_id,
            } => {
                todo!()
            }
        }
    }
}
