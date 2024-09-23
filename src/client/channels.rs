use tokio::sync::mpsc::Sender;

use super::{ClientMessage, ReadSocket, WriteSocket};

pub enum OutgoingMessage {
    DocumentData(Vec<u8>),
    NewSocket(WriteSocket),
}

#[derive(Clone)]
pub struct Channels {
    pub stdin_tx: Sender<ClientMessage>,
    pub stdout_tx: Sender<ClientMessage>,
    pub incoming_to_tx: Sender<ReadSocket>,
    pub outgoing_tx: Sender<OutgoingMessage>,
}
