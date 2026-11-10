//! The local copies' drive routes against a stand-in drive.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use ruchoir_desktop_core::files::{download, send_version, DriveError};

const FILE: &str = "0b9d9c5e-5e1a-4267-85f4-bfcec5b0bd3b";

fn authorised(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer drive-token")
}

/// A drive serving one file's bytes and taking its versions, keeping each upload's body and type.
async fn stand_in(status: StatusCode) -> (String, Arc<Mutex<Vec<(String, Vec<u8>)>>>) {
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let kept = uploads.clone();
    let app = Router::new()
        .route(
            "/api/v1/files/{id}/download",
            get(|Path(id): Path<String>, headers: HeaderMap| async move {
                if !authorised(&headers) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                if id != FILE {
                    return StatusCode::NOT_FOUND.into_response();
                }
                b"the bytes".to_vec().into_response()
            }),
        )
        .route(
            "/api/v1/files/{id}/versions",
            post(
                move |Path(id): Path<String>, headers: HeaderMap, body: Bytes| {
                    let kept = kept.clone();
                    async move {
                        if !authorised(&headers) {
                            return StatusCode::UNAUTHORIZED.into_response();
                        }
                        if status != StatusCode::CREATED {
                            return status.into_response();
                        }
                        let kind = headers
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default()
                            .to_owned();
                        kept.lock().unwrap().push((kind, body.to_vec()));
                        (
                            StatusCode::CREATED,
                            Json(serde_json::json!({ "id": id, "version_id": "v2" })),
                        )
                            .into_response()
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), uploads)
}

#[tokio::test]
async fn a_file_is_read_with_the_drive_token() {
    let (drive, _) = stand_in(StatusCode::CREATED).await;
    let http = ruchoir_desktop_core::http_client();
    assert_eq!(
        download(&http, &drive, "drive-token", FILE).await.unwrap(),
        b"the bytes"
    );
    assert_eq!(
        download(&http, &drive, "other", FILE).await,
        Err(DriveError::SignedOut)
    );
    assert_eq!(
        download(&http, &drive, "drive-token", "../me").await,
        Err(DriveError::Failed(400))
    );
}

#[tokio::test]
async fn a_modified_copy_goes_up_as_a_new_version() {
    let (drive, uploads) = stand_in(StatusCode::CREATED).await;
    let http = ruchoir_desktop_core::http_client();
    let version = send_version(&http, &drive, "drive-token", FILE, "Devis.odt", b"edited")
        .await
        .unwrap();
    assert_eq!(version.as_deref(), Some("v2"));
    let uploads = uploads.lock().unwrap();
    let (kind, body) = &uploads[0];
    let boundary = kind
        .strip_prefix("multipart/form-data; boundary=")
        .expect("a multipart body");
    let body = String::from_utf8_lossy(body);
    assert!(body.starts_with(&format!("--{boundary}\r\n")));
    assert!(body.contains("name=\"file\"; filename=\"Devis.odt\""));
    assert!(body.contains("\r\n\r\nedited\r\n"));
}

#[tokio::test]
async fn the_drives_refusals_are_codes() {
    let http = ruchoir_desktop_core::http_client();
    for (status, error) in [
        (StatusCode::FORBIDDEN, DriveError::Forbidden),
        (StatusCode::PAYLOAD_TOO_LARGE, DriveError::TooLarge),
        (StatusCode::BAD_GATEWAY, DriveError::Unreachable),
    ] {
        let (drive, _) = stand_in(status).await;
        let answer = send_version(&http, &drive, "drive-token", FILE, "a.txt", b"x").await;
        assert_eq!(answer, Err(error.clone()));
    }
    let (drive, _) = stand_in(StatusCode::CREATED).await;
    assert_eq!(
        send_version(&http, &drive, "stale", FILE, "a.txt", b"x").await,
        Err(DriveError::SignedOut)
    );
    assert_eq!(DriveError::SignedOut.code(), "signed_out");
}
