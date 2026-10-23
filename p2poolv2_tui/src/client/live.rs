// SPDX-FileCopyrightText: 2024 PDM Authors
// SPDX-FileCopyrightText: 2026 P2Poolv2 Developers (see AUTHORS)
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The node's `/ws` live feed, ported from PDM's `P2PoolWebSocketClient`.
//!
//! It subscribes to the `shares` and `peers` topics and passes each event
//! on through a bounded channel, so a slow screen holds the feed back
//! instead of queueing without end. Frames over [`MAX_FRAME_BYTES`] end
//! the feed.

use crate::client::http::REQUEST_TIMEOUT;
use crate::model::LiveEvent;
use futures::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

/// The largest frame or message taken from the feed.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// What the feed passes on.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveMessage {
    Event(LiveEvent),
    /// A frame that is no event this tool knows, with why.
    Malformed(String),
    /// The feed stopped, with why. Nothing follows it.
    Ended(String),
}

/// The frame subscribing to `topic`.
fn subscribe(topic: &str) -> String {
    format!(r#"{{"action":"subscribe","topic":"{topic}"}}"#)
}

/// Streams the feed at `url` into `sender` until it fails or closes, then
/// sends [`LiveMessage::Ended`].
pub async fn run(url: String, sender: mpsc::Sender<LiveMessage>) {
    let reason = stream(&url, &sender).await;
    // When this send fails the screen has stopped listening, and there is
    // nobody left to tell.
    let _ = sender.send(LiveMessage::Ended(reason)).await;
}

async fn stream(url: &str, sender: &mpsc::Sender<LiveMessage>) -> String {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES));
    let connecting = connect_async_with_config(url, Some(config), false);
    let socket = match timeout(REQUEST_TIMEOUT, connecting).await {
        Ok(Ok((socket, _))) => socket,
        Ok(Err(error)) => return format!("could not connect: {error}"),
        Err(_) => return "could not connect: timed out".to_string(),
    };
    let (mut write, mut read) = socket.split();
    for topic in ["shares", "peers"] {
        if let Err(error) = write.send(Message::text(subscribe(topic))).await {
            return format!("could not subscribe to {topic}: {error}");
        }
    }
    while let Some(frame) = read.next().await {
        let message = match frame {
            Ok(Message::Text(text)) => Some(
                serde_json::from_str::<LiveEvent>(text.as_str()).map_or_else(
                    |error| LiveMessage::Malformed(error.to_string()),
                    LiveMessage::Event,
                ),
            ),
            Ok(Message::Close(_)) => return "the node closed the live feed".to_string(),
            // Pings are answered by tungstenite itself; binary frames and
            // pongs carry no events.
            Ok(Message::Binary(_) | Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                None
            }
            Err(error) => return format!("the live feed failed: {error}"),
        };
        if let Some(message) = message
            && sender.send(message).await.is_err()
        {
            return "nobody is listening".to_string();
        }
    }
    "the node closed the live feed".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PeerEvent, PeerStatus};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[tokio::test]
    async fn test_subscribes_then_passes_events_on() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let first = socket.next().await.unwrap().unwrap();
            let second = socket.next().await.unwrap().unwrap();
            socket
                .send(Message::text(
                    r#"{"topic":"Peer","data":{"peer_id":"12D3","status":"Connected"}}"#,
                ))
                .await
                .unwrap();
            socket.send(Message::text("not json")).await.unwrap();
            socket.close(None).await.unwrap();
            (first.into_text().unwrap(), second.into_text().unwrap())
        });

        let (sender, mut receiver) = mpsc::channel(8);
        run(format!("ws://{address}/ws"), sender).await;
        let (first, second) = server.await.unwrap();

        assert_eq!(first.as_str(), subscribe("shares"));
        assert_eq!(second.as_str(), subscribe("peers"));
        assert_eq!(
            receiver.recv().await.unwrap(),
            LiveMessage::Event(LiveEvent::Peer(PeerEvent {
                peer_id: "12D3".into(),
                status: PeerStatus::Connected,
            }))
        );
        assert!(matches!(
            receiver.recv().await.unwrap(),
            LiveMessage::Malformed(_)
        ));
        assert_eq!(
            receiver.recv().await.unwrap(),
            LiveMessage::Ended("the node closed the live feed".into())
        );
    }

    #[tokio::test]
    async fn test_a_refused_connection_ends_the_feed() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let (sender, mut receiver) = mpsc::channel(8);
        run(format!("ws://{address}/ws"), sender).await;
        assert!(matches!(
            receiver.recv().await.unwrap(),
            LiveMessage::Ended(reason) if reason.starts_with("could not connect")
        ));
    }
}
