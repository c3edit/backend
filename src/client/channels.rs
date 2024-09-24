use super::{BackendMessage, ClientMessage, ReadSocket, WriteSocket};
use std::net::SocketAddr;
use tokio::{net::TcpStream, sync::mpsc::Sender};

pub enum MainTaskMessage {
    NewConnection((TcpStream, SocketAddr)),
    ClientMessage(ClientMessage),
    BackendMessage(BackendMessage),
    DocumentChanged(String),
}

pub enum OutgoingMessage {
    BackendMessage(BackendMessage),
    NewSocket(WriteSocket),
}

#[derive(Clone)]
pub struct Channels {
    pub main_tx: Sender<MainTaskMessage>,
    pub incoming_to_tx: Sender<ReadSocket>,
    pub outgoing_tx: Sender<OutgoingMessage>,
    pub stdout_tx: Sender<ClientMessage>,
}
