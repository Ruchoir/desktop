//! Reading the hub's inbox against a stand-in hub.

use axum::extract::RawQuery;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use ruchoir_desktop_core::inbox::{fetch_unread, FetchError};

async fn stand_in() -> String {
    let app = Router::new().route(
        "/api/v1/me/inbox",
        get(|headers: HeaderMap, RawQuery(query): RawQuery| async move {
            let bearer = headers.get("authorization").and_then(|v| v.to_str().ok());
            if bearer != Some("Bearer hub-token") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            assert_eq!(query.as_deref(), Some("unread=true&limit=20"));
            Json(serde_json::json!({
                "entries": [{
                    "id": "e1", "product": "drive", "kind": "drive.share", "category": "share",
                    "organisation": null, "actor": null, "object": "f1",
                    "link": "https://drive.example.org/f/f1",
                    "variables": { "title": "Léa" },
                    "created_at": "2026-11-10T10:00:00Z", "read_at": null
                }],
                "next_before": null,
                "unread_count": 3
            }))
            .into_response()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn the_unread_entries_are_read_with_the_hub_token() {
    let hub = stand_in().await;
    let http = ruchoir_desktop_core::http_client();
    let page = fetch_unread(&http, &hub, "hub-token").await.unwrap();
    assert_eq!(page.unread_count, 3);
    assert_eq!(page.entries[0].id, "e1");
    assert_eq!(page.entries[0].link, "https://drive.example.org/f/f1");
    assert_eq!(
        fetch_unread(&http, &hub, "stale").await,
        Err(FetchError::Unauthorized)
    );
}
