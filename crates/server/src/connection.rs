//! One task per WebSocket: handshake, then shuttle bytes between the socket and the room.

use axum::extract::ws::{Message, WebSocket};
use bomber_core::protocol::{ClientMsg, PROTOCOL_VERSION, ServerMsg, decode, encode};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, oneshot};

use crate::room::{self, RoomCmd};

const MAX_NAME_CHARS: usize = 16;

pub async fn run(mut socket: WebSocket, room: Option<mpsc::Sender<RoomCmd>>) {
    let Some(room) = room else {
        return reject(socket, "Room not found").await;
    };

    // 1. Handshake: the first message must be a Hello with our protocol version.
    let (name, token) = match socket.recv().await {
        Some(Ok(Message::Binary(b))) => match decode::<ClientMsg>(&b) {
            Ok(ClientMsg::Hello { version, name, token }) if version == PROTOCOL_VERSION => (name, token),
            _ => return reject(socket, "Game was updated, please reload the page").await,
        },
        _ => return,
    };

    // 2. Ask the room for a slot.
    let (out_tx, mut out_rx) = room::client_queue();
    let (reply_tx, reply_rx) = oneshot::channel();
    let join = RoomCmd::Join { name: clean_name(&name), token, out: out_tx, reply: reply_tx };
    if room.send(join).await.is_err() {
        return reject(socket, "Room has closed").await;
    }
    let (id, conn) = match reply_rx.await {
        Ok(Ok(joined)) => (joined.id, joined.conn),
        Ok(Err(reason)) => return reject(socket, &reason).await,
        Err(_) => return reject(socket, "Room has closed").await,
    };

    // 3. Pump: split the socket so reading and writing run independently.
    let (mut sink, mut stream) = socket.split();
    let writer = tokio::spawn(async move {
        while let Some(bytes) = out_rx.recv().await {
            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        let cmd = match msg {
            Message::Binary(b) => match decode::<ClientMsg>(&b) {
                Ok(ClientMsg::Input { seq, input }) => RoomCmd::Input { id, conn, seq, input },
                Ok(ClientMsg::StartMatch) => RoomCmd::StartMatch { id, conn },
                Ok(ClientMsg::SetWinsNeeded(wins)) => RoomCmd::SetWinsNeeded { id, conn, wins },
                _ => continue,
            },
            Message::Close(_) => break,
            _ => continue,
        };
        if room.send(cmd).await.is_err() {
            break;
        }
    }

    let _ = room.send(RoomCmd::Leave { id, conn }).await;
    writer.abort();
}

async fn reject(mut socket: WebSocket, reason: &str) {
    let msg = encode(&ServerMsg::Rejected { reason: reason.into() });
    let _ = socket.send(Message::Binary(msg.into())).await;
    let _ = socket.send(Message::Close(None)).await;
}

fn clean_name(raw: &str) -> String {
    let name: String = raw.trim().chars().filter(|c| !c.is_control()).take(MAX_NAME_CHARS).collect();
    if name.is_empty() { "Player".into() } else { name }
}
