use super::{
    channels::{MainTaskMessage, OutgoingMessage},
    BackendMessage, ClientMessage, ReadSocket,
};
use futures::{SinkExt, TryStreamExt};
use std::io::Write as _;
use tokio::{
    io::{self, AsyncBufReadExt, BufReader},
    net::TcpListener,
    sync::mpsc::{Receiver, Sender},
};
use tracing::info;

pub fn begin_incoming_task(tx: Sender<MainTaskMessage>, mut rx: Receiver<ReadSocket>) {
    tokio::spawn(async move {
        while let Some(mut socket) = rx.recv().await {
            let tx = tx.clone();

            // TODO store join handles so we can cancel tasks when disconnecting.
            tokio::spawn(async move {
                while let Some(message) = socket.try_next().await.unwrap() {
                    info!("Received from network: {:?}", message);
                    let BackendMessage::DocumentSync { data } = message;
                    tx.send(MainTaskMessage::DocumentData(data)).await.unwrap();
                }
            });
        }
    });
}

pub fn begin_outgoing_task(mut rx: Receiver<OutgoingMessage>) {
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

pub fn begin_stdin_task(tx: Sender<MainTaskMessage>) {
    tokio::spawn(async move {
        let stdin = BufReader::new(io::stdin());
        let mut lines = stdin.lines();

        while let Ok(Some(line)) = lines.next_line().await {
            info!("Received message from stdin: {}", line);
            let message = serde_json::from_str::<ClientMessage>(&line).unwrap();
            tx.send(MainTaskMessage::ClientMessage(message))
                .await
                .unwrap();
        }
    });
}

pub fn begin_stdout_task(mut rx: Receiver<ClientMessage>) {
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

pub fn begin_listening_task(listener: TcpListener, tx: Sender<MainTaskMessage>) {
    tokio::spawn(async move {
        while let Ok(message) = listener.accept().await {
            tx.send(MainTaskMessage::NewConnection(message))
                .await
                .unwrap();
        }
    });
}
