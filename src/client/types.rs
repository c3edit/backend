use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio_serde::formats::SymmetricalJson;
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

pub(crate) type DocumentID = String;
// I hate Rust sometimes.
pub(crate) type WriteSocket = tokio_serde::SymmetricallyFramed<
    FramedWrite<OwnedWriteHalf, LengthDelimitedCodec>,
    BackendMessage,
    SymmetricalJson<BackendMessage>,
>;
pub(crate) type ReadSocket = tokio_serde::SymmetricallyFramed<
    FramedRead<OwnedReadHalf, LengthDelimitedCodec>,
    BackendMessage,
    SymmetricalJson<BackendMessage>,
>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "snake_case"))]
#[serde(tag = "type")]
pub(crate) enum ClientMessage {
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
        /// This field should be omitted for the client's cursor.
        peer_id: Option<loro::PeerID>,
        location: usize,
    },
    SetSelection {
        document_id: String,
        /// This field should be omitted for the client's selection.
        peer_id: Option<loro::PeerID>,
        point: usize,
        mark: usize,
    },
UnsetSelection {
        document_id: String,
        peer_id: Option<loro::PeerID>,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all(serialize = "snake_case", deserialize = "snake_case"))]
#[serde(tag = "type")]
pub(crate) enum Change {
    Insert { index: usize, text: String },
    Delete { index: usize, len: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum BackendMessage {
    DocumentSync {
        data: Vec<u8>,
    },
    CursorUpdate {
        document_id: String,
        peer_id: loro::PeerID,
        cursor: loro::cursor::Cursor,
    },
    SelectionUpdate {
        document_id: String,
        peer_id: loro::PeerID,
        selection: Selection,
    },
    UnsetSelection {
        document_id: String,
        peer_id: loro::PeerID,
    },
}

pub(crate) struct DocumentInfo {
    pub sub_id: loro::SubID,
    // TODO Merge into HashMaps?
    pub cursor: Option<loro::cursor::Cursor>,
    pub selection: Option<Selection>,
    pub cursors: HashMap<loro::PeerID, loro::cursor::Cursor>,
    pub selections: HashMap<loro::PeerID, Selection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Selection {
    pub point: loro::cursor::Cursor,
    pub mark: loro::cursor::Cursor,
}
