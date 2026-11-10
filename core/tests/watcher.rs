//! The watcher against a stand-in hub live socket.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use ruchoir_desktop_core::watcher::{
    backoff, socket_url, TokenError, Tokens, Watcher, WatcherEvent,
};

#[test]
fn backoff_caps_at_30s() {
    assert!(backoff(0) >= Duration::from_secs(1));
    assert!(backoff(0) <= Duration::from_millis(1200));
    assert!(backoff(3) >= Duration::from_secs(8));
    assert!(backoff(10) <= Duration::from_secs(36));
    assert!(backoff(40) <= Duration::from_secs(36));
}

#[test]
fn the_socket_is_the_hubs_live_one() {
    assert_eq!(
        socket_url("https://hub.example.org"),
        "wss://hub.example.org/api/v1/live/ws"
    );
    assert_eq!(
        socket_url("http://127.0.0.1:9/"),
        "ws://127.0.0.1:9/api/v1/live/ws"
    );
}

/// The hub's live socket: refuses without the right bearer (or with an `Origin`, which would make
/// it a page needing a ticket), sends one inbox frame, then closes, counting connections.
async fn stand_in(token: &'static str) -> (String, Arc<AtomicUsize>) {
    let connections = Arc::new(AtomicUsize::new(0));
    let c = connections.clone();
    let app = Router::new().route(
        "/api/v1/live/ws",
        get(move |headers: HeaderMap, ws: WebSocketUpgrade| {
            let c = c.clone();
            async move {
                let bearer = headers.get("authorization").and_then(|v| v.to_str().ok());
                if bearer != Some(&format!("Bearer {token}")) || headers.contains_key("origin") {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                c.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(move |mut socket| async move {
                    let _ = socket
                        .send(Message::Text(
                            r#"{"v":1,"type":"hub.inbox.new","payload":{}}"#.into(),
                        ))
                        .await;
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    let _ = socket.send(Message::Close(None)).await;
                })
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), connections)
}

fn collect() -> (
    Arc<Mutex<Vec<WatcherEvent>>>,
    impl Fn(WatcherEvent) + Send + Sync + 'static,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let e = events.clone();
    (events, move |event| e.lock().unwrap().push(event))
}

/// Tokens that answer `answer`, counting how often they were asked.
fn tokens(answer: Result<&'static str, TokenError>) -> (Tokens, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    let a = asked.clone();
    let tokens: Tokens = Arc::new(move || {
        a.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { answer.map(str::to_owned) })
    });
    (tokens, asked)
}

#[tokio::test]
async fn relays_frames_with_the_hub_token() {
    let (hub, _) = stand_in("t").await;
    let (events, sink) = collect();
    let (tokens, _) = tokens(Ok("t"));
    let watcher = Watcher::spawn(hub, tokens, sink);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let seen = events.lock().unwrap().clone();
    assert_eq!(seen[0], WatcherEvent::Open);
    assert_eq!(
        seen[1],
        WatcherEvent::Message(r#"{"v":1,"type":"hub.inbox.new","payload":{}}"#.into())
    );
    watcher.stop();
}

#[tokio::test]
async fn reconnects_after_a_drop_with_a_token_asked_again() {
    let (hub, connections) = stand_in("t").await;
    let (events, sink) = collect();
    let (tokens, asked) = tokens(Ok("t"));
    let watcher = Watcher::spawn(hub, tokens, sink);
    tokio::time::sleep(Duration::from_millis(2000)).await;
    assert!(connections.load(Ordering::SeqCst) >= 2);
    assert!(asked.load(Ordering::SeqCst) >= 2);
    assert!(events.lock().unwrap().contains(&WatcherEvent::Closed));
    watcher.stop();
}

#[tokio::test]
async fn a_refused_token_reports_signed_out_and_stops() {
    let (hub, connections) = stand_in("right").await;
    let (events, sink) = collect();
    let (tokens, _) = tokens(Ok("wrong"));
    let _watcher = Watcher::spawn(hub, tokens, sink);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(events.lock().unwrap().as_slice(), [WatcherEvent::SignedOut]);
    assert_eq!(connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_ended_session_stops_before_connecting() {
    let (hub, connections) = stand_in("t").await;
    let (events, sink) = collect();
    let (tokens, _) = tokens(Err(TokenError::SignedOut));
    let _watcher = Watcher::spawn(hub, tokens, sink);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(events.lock().unwrap().as_slice(), [WatcherEvent::SignedOut]);
    assert_eq!(connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn reports_whether_it_is_open() {
    let (hub, _) = stand_in("t").await;
    let (_, sink) = collect();
    let (tokens, _) = tokens(Ok("t"));
    let watcher = Watcher::spawn(hub, tokens, sink);
    assert!(!watcher.is_open());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(watcher.is_open());
    // The stand-in closes after 300 ms; the watcher is then between two attempts.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!watcher.is_open());
    watcher.stop();
}
