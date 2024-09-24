use std::net::SocketAddr;
use tokio::{net::TcpStream, sync::mpsc::Sender};
use super::{ClientMessage, ReadSocket, WriteSocket};

pub enum MainTaskMessage {
    NewConnection((TcpStream, SocketAddr)),
    ClientMessage(ClientMessage),
    DocumentData(Vec<u8>),
    UpdateCursor(String),
}

pub enum OutgoingMessage {
    DocumentData(Vec<u8>),
    NewSocket(WriteSocket),
}

#[derive(Clone)]
pub struct Channels {
    pub main_tx: Sender<MainTaskMessage>,
    pub incoming_to_tx: Sender<ReadSocket>,
    pub outgoing_tx: Sender<OutgoingMessage>,
    pub stdout_tx: Sender<ClientMessage>,
}
