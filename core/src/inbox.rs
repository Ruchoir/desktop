//! Native notifications from the hub's inbox.
//!
//! When the live socket brings `hub.inbox.new`, the app reads the person's unread entries
//! (`GET /api/v1/me/inbox?unread=true`, with the hub access token) and draws those newer than the last
//! one drawn. The hub already applied the person's settings when the entry was published (`App`
//! delivery), so the app only keeps from drawing the same entry twice. `unread_count` feeds the badge;
//! `hub.inbox.read` refreshes it.

use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// How many unread entries one round reads.
pub const ROUND_LIMIT: u32 = 20;

/// The whole exchange with the hub may take this long.
pub const FETCH_WITHIN: std::time::Duration = std::time::Duration::from_secs(15);

/// One inbox entry, as its reader sees it (the hub's `Entry`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Entry {
    pub id: String,
    /// The product that published it (`drive`, `chat`...).
    pub product: String,
    pub kind: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub variables: serde_json::Value,
    /// RFC 3339.
    pub created_at: String,
}

/// A page of the inbox.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub unread_count: i64,
}

/// Why the inbox could not be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchError {
    /// The hub refused the token.
    Unauthorized,
    /// The hub could not be reached, or answered something else.
    Unreachable,
}

/// The type of a live frame (`hub.inbox.new`...), when it is one.
pub fn frame_type(frame: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(frame)
        .ok()?
        .get("type")?
        .as_str()
        .map(str::to_owned)
}

/// The person's unread entries, newest first, and how many are unread in all.
pub async fn fetch_unread(
    http: &reqwest::Client,
    hub: &str,
    token: &str,
) -> Result<Page, FetchError> {
    let answer = http
        .get(format!(
            "{hub}/api/v1/me/inbox?unread=true&limit={ROUND_LIMIT}"
        ))
        .bearer_auth(token)
        .timeout(FETCH_WITHIN)
        .send()
        .await
        .map_err(|_| FetchError::Unreachable)?;
    match answer.status().as_u16() {
        200 => answer.json().await.map_err(|_| FetchError::Unreachable),
        401 => Err(FetchError::Unauthorized),
        _ => Err(FetchError::Unreachable),
    }
}

fn instant(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).ok()
}

/// The entries strictly newer than `since`, oldest first (the order to draw them in).
pub fn fresh(entries: &[Entry], since: Option<&str>) -> Vec<Entry> {
    let floor = since.and_then(instant);
    let mut kept: Vec<(OffsetDateTime, Entry)> = entries
        .iter()
        .filter_map(|e| Some((instant(&e.created_at)?, e.clone())))
        .filter(|(at, _)| floor.is_none_or(|floor| *at > floor))
        .collect();
    kept.sort_by_key(|(at, _)| *at);
    kept.into_iter().map(|(_, e)| e).collect()
}

/// The new `since` once `drawn` were drawn: the newest of them, never earlier than `since`.
pub fn advance(since: Option<&str>, drawn: &[Entry]) -> Option<String> {
    let mut best: Option<(OffsetDateTime, String)> =
        since.and_then(|s| Some((instant(s)?, s.to_string())));
    for entry in drawn {
        if let Some(at) = instant(&entry.created_at) {
            if best.as_ref().is_none_or(|(b, _)| at > *b) {
                best = Some((at, entry.created_at.clone()));
            }
        }
    }
    best.map(|(_, text)| text)
}

/// A product's name as a title: `drive` reads `Drive`.
fn product_title(product: &str) -> String {
    let mut chars = product.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Ruchoir".to_owned(),
    }
}

/// The title and body of an entry's notification. The products word their entries in the
/// variables they publish (`title`, `body`), already in the person's language; without them the
/// product's name and the kind say what happened.
pub fn notification(entry: &Entry) -> (String, String) {
    let text = |key: &str| {
        entry
            .variables
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let title = text("title").unwrap_or_else(|| product_title(&entry.product));
    let body = text("body")
        .or_else(|| text("name"))
        .unwrap_or_else(|| entry.kind.clone());
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, at: &str) -> Entry {
        Entry {
            id: id.into(),
            product: "drive".into(),
            kind: "drive.share".into(),
            link: String::new(),
            variables: serde_json::json!({}),
            created_at: at.into(),
        }
    }

    #[test]
    fn only_newer_entries_are_drawn_oldest_first() {
        let entries = [
            entry("c", "2026-11-10T10:02:00Z"),
            entry("b", "2026-11-10T10:01:00Z"),
            entry("a", "2026-11-10T10:00:00Z"),
            entry("x", "not a date"),
        ];
        let ids = |v: Vec<Entry>| v.into_iter().map(|e| e.id).collect::<Vec<_>>();
        assert_eq!(ids(fresh(&entries, None)), ["a", "b", "c"]);
        assert_eq!(
            ids(fresh(&entries, Some("2026-11-10T10:00:00Z"))),
            ["b", "c"]
        );
        let drawn = fresh(&entries, Some("2026-11-10T10:00:00Z"));
        assert_eq!(
            advance(Some("2026-11-10T10:00:00Z"), &drawn).as_deref(),
            Some("2026-11-10T10:02:00Z")
        );
        assert_eq!(
            advance(Some("2026-11-10T11:00:00Z"), &drawn).as_deref(),
            Some("2026-11-10T11:00:00Z")
        );
    }

    #[test]
    fn a_notification_uses_the_products_words_or_its_name() {
        let mut e = entry("a", "2026-11-10T10:00:00Z");
        assert_eq!(notification(&e), ("Drive".into(), "drive.share".into()));
        e.variables =
            serde_json::json!({ "title": "Léa", "body": "a partagé «\u{a0}Devis\u{a0}»" });
        assert_eq!(
            notification(&e),
            ("Léa".into(), "a partagé «\u{a0}Devis\u{a0}»".into())
        );
    }

    #[test]
    fn a_frames_type_is_read() {
        assert_eq!(
            frame_type(r#"{"v":1,"type":"hub.inbox.new","payload":{}}"#).as_deref(),
            Some("hub.inbox.new")
        );
        assert_eq!(frame_type("nope"), None);
    }
}
