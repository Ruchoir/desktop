//! One live socket per signed-in instance, to its hub (`GET /api/v1/live/ws`), held by the app.
//!
//! The hub settles who opens a socket on the upgrade request: a page brings a single-use ticket its
//! product's backend got for it, an app its own access token for the hub, as a bearer and without
//! `Origin`. The app is the second kind: it asks a token for each connection (a reconnection after an
//! hour gets a fresh one) and never needs a ticket. The socket only receives; the app reacts to the
//! hub's own frames (`hub.inbox.new`, `hub.inbox.read`) with notifications and the badge.

use std::future::Future;
use std::pin::Pin;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

/// What a watcher tells the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatcherEvent {
    /// The socket is open (again).
    Open,
    /// A frame from the instance, as the API sent it.
    Message(String),
    /// The socket dropped; the watcher is already trying again.
    Closed,
    /// The hub refused the token, or no token can be had any more: the watcher has stopped for good.
    SignedOut,
}

/// Why no token could be had for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenError {
    /// The session ended: the person signs in again.
    SignedOut,
    /// Propolis could not be reached right now: try again later.
    Unavailable,
}

/// What gives the watcher a hub access token for each connection.
pub type Tokens =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<String, TokenError>> + Send>> + Send + Sync>;

/// The live socket's address for a hub origin.
pub fn socket_url(hub: &str) -> String {
    format!(
        "{}/api/v1/live/ws",
        hub.trim_end_matches('/')
            .replacen("https://", "wss://", 1)
            .replacen("http://", "ws://", 1)
    )
}

/// The wait before reconnection attempt `attempt` (0 first): 1 s doubling up to 30 s, plus up to
/// 20 % of jitter so many apps waking at once do not knock together.
pub fn backoff(attempt: u32) -> Duration {
    let base_ms = 1000u64.saturating_mul(1u64 << attempt.min(5)).min(30_000);
    let mut byte = [0u8; 1];
    let _ = getrandom::fill(&mut byte);
    let jitter = base_ms * u64::from(byte[0]) / 255 / 5;
    Duration::from_millis(base_ms + jitter)
}

/// A running watcher. Dropping it does not stop it; `stop` does.
pub struct Watcher {
    task: JoinHandle<()>,
    open: Arc<AtomicBool>,
}

impl Watcher {
    /// Connect to `hub`'s live socket with a token from `tokens`, and keep connecting until stopped
    /// or signed out.
    pub fn spawn<F>(hub: String, tokens: Tokens, sink: F) -> Watcher
    where
        F: Fn(WatcherEvent) + Send + Sync + 'static,
    {
        let open = Arc::new(AtomicBool::new(false));
        let is_open = open.clone();
        let url = socket_url(&hub);
        let task = tokio::spawn(async move {
            let mut attempt = 0u32;
            loop {
                let token = match tokens().await {
                    Ok(token) => Some(token),
                    Err(TokenError::SignedOut) => {
                        sink(WatcherEvent::SignedOut);
                        return;
                    }
                    Err(TokenError::Unavailable) => None,
                };
                if let Some(token) = token {
                    let Ok(mut request) = url.as_str().into_client_request() else {
                        return;
                    };
                    if let Ok(value) = format!("Bearer {token}").parse() {
                        request.headers_mut().insert("authorization", value);
                    }
                    match tokio_tungstenite::connect_async(request).await {
                        Ok((mut socket, _)) => {
                            attempt = 0;
                            is_open.store(true, Ordering::Relaxed);
                            sink(WatcherEvent::Open);
                            loop {
                                match socket.next().await {
                                    Some(Ok(Message::Text(text))) => {
                                        sink(WatcherEvent::Message(text.to_string()))
                                    }
                                    // Pings are answered by the library as they are read.
                                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                                    Some(Ok(_)) => {}
                                }
                            }
                            is_open.store(false, Ordering::Relaxed);
                            sink(WatcherEvent::Closed);
                        }
                        Err(WsError::Http(response)) if response.status().as_u16() == 401 => {
                            sink(WatcherEvent::SignedOut);
                            return;
                        }
                        Err(_) => {}
                    }
                }
                tokio::time::sleep(backoff(attempt)).await;
                attempt = attempt.saturating_add(1);
            }
        });
        Watcher { task, open }
    }

    /// Whether the socket is open right now.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    /// Close the socket and stop reconnecting.
    pub fn stop(self) {
        self.task.abort();
    }
}
