//! The live socket of each signed-in instance, to its hub, and what the app draws from it.
//!
//! - `hub.inbox.new`: the unread entries newer than the last one drawn become system notifications,
//!   unless a window of that instance is focused (its pages show them). A click opens the product's
//!   window at the entry's link.
//! - `hub.inbox.read`, and each (re)connection: the unread count is read again for the badge.
//! - A refused token: the session is over (`state::session_expired`).
//!
//! `since` is kept per instance across restarts (`notify-since.json`), and starts when the socket
//! first opens, so a backlog is never drawn.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use ruchoir_desktop_core::inbox::{advance, fetch_unread, frame_type, fresh, notification, Entry};
use ruchoir_desktop_core::watcher::{TokenError, Tokens, Watcher, WatcherEvent};
use ruchoir_native::NativeError;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_store::StoreExt;

use crate::state::Desktop;

/// Where `since` is kept.
const SINCE_STORE: &str = "notify-since.json";

/// How many clicked-notification targets are remembered.
const KEEP_TARGETS: usize = 200;

/// What a click on a notification opens.
#[derive(Clone)]
pub enum Target {
    /// An inbox entry: the instance, the product and the link.
    Entry(String, String, String),
    /// A saved local copy: the instance and the file.
    Save(String, String),
}

/// The live state of every instance.
#[derive(Default)]
pub struct Live {
    watchers: Mutex<HashMap<String, Watcher>>,
    unread: Mutex<HashMap<String, i64>>,
    busy: Mutex<HashSet<String>>,
    targets: Mutex<Vec<(i32, Target)>>,
    next_id: AtomicI32,
}

impl Live {
    /// A new notification id, its click leading to `target`.
    pub fn remember(&self, target: Target) -> i32 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut targets) = self.targets.lock() {
            targets.push((id, target));
            let excess = targets.len().saturating_sub(KEEP_TARGETS);
            targets.drain(..excess);
        }
        id
    }

    /// Every unread entry of every instance, for the badge.
    pub fn total_unread(&self) -> i64 {
        self.unread
            .lock()
            .map(|u| u.values().sum())
            .unwrap_or_default()
    }
}

/// The sound each system's notification server plays for a new message. On Windows, the instant
/// message sound by name: "Default" adds no `<audio>` to the toast and leaves it to the app's
/// registration, which an app installed by NSIS may not have.
fn system_sound() -> &'static str {
    if cfg!(target_os = "macos") {
        "default"
    } else if cfg!(windows) {
        "IM"
    } else {
        "message-new-instant"
    }
}

fn read_since<R: Runtime>(app: &AppHandle<R>, id: &str) -> Option<String> {
    let store = app.store(SINCE_STORE).ok()?;
    store.get(id)?.as_str().map(str::to_string)
}

fn write_since<R: Runtime>(app: &AppHandle<R>, id: &str, since: &str) {
    if let Ok(store) = app.store(SINCE_STORE) {
        store.set(id, since);
        let _ = store.save();
    }
}

/// The hub tokens of an instance, as the watcher asks them.
fn tokens<R: Runtime>(app: &AppHandle<R>, id: &str) -> Tokens {
    let (app, id) = (app.clone(), id.to_owned());
    Arc::new(move || {
        let (app, id) = (app.clone(), id.clone());
        Box::pin(async move {
            match app.state::<Desktop>().token(&id, "hub").await {
                Ok(token) => Ok(token),
                Err(NativeError::SessionEnded | NativeError::SignedOut) => {
                    Err(TokenError::SignedOut)
                }
                Err(_) => Err(TokenError::Unavailable),
            }
        })
    })
}

/// Start a socket for each signed-in instance with a hub, and stop the others'.
pub fn reconcile<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let desktop = app.state::<Desktop>();
        let mut wanted = HashMap::new();
        for instance in desktop.instances() {
            if let Some(hub) = instance.product("hub") {
                if desktop.signed_in(&instance).await {
                    wanted.insert(instance.id.clone(), hub.to_owned());
                }
            }
        }
        let live = app.state::<Live>();
        let Ok(mut watchers) = live.watchers.lock() else {
            return;
        };
        let gone: Vec<String> = watchers
            .keys()
            .filter(|id| !wanted.contains_key(*id))
            .cloned()
            .collect();
        for id in gone {
            if let Some(watcher) = watchers.remove(&id) {
                watcher.stop();
            }
        }
        for (id, hub) in wanted {
            if watchers.contains_key(&id) {
                continue;
            }
            let sink_app = app.clone();
            let sink_id = id.clone();
            let watcher = Watcher::spawn(hub, tokens(&app, &id), move |event| {
                on_event(&sink_app, &sink_id, event)
            });
            watchers.insert(id, watcher);
        }
    });
}

/// Stop an instance's socket and forget its count.
pub fn forget<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let live = app.state::<Live>();
    if let Some(watcher) = live.watchers.lock().ok().and_then(|mut w| w.remove(id)) {
        watcher.stop();
    }
    if let Ok(mut unread) = live.unread.lock() {
        unread.remove(id);
    }
    if let Ok(mut targets) = live.targets.lock() {
        targets.retain(|(_, t)| match t {
            Target::Entry(i, _, _) | Target::Save(i, _) => i != id,
        });
    }
    crate::shell::update_badge(app);
}

fn on_event<R: Runtime>(app: &AppHandle<R>, id: &str, event: WatcherEvent) {
    match event {
        WatcherEvent::Open => {
            if read_since(app, id).is_none() {
                let now = time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                write_since(app, id, &now);
            }
            round(app, id, false);
        }
        WatcherEvent::Message(frame) => match frame_type(&frame).as_deref() {
            Some("hub.inbox.new") => round(app, id, true),
            Some("hub.inbox.read") => round(app, id, false),
            _ => {}
        },
        WatcherEvent::Closed => {}
        WatcherEvent::SignedOut => {
            if let Some(watcher) = app
                .state::<Live>()
                .watchers
                .lock()
                .ok()
                .and_then(|mut w| w.remove(id))
            {
                watcher.stop();
            }
            crate::state::session_expired(app, id);
        }
    }
}

/// Read the inbox: the count always, and the new entries drawn when `draw`. One round at a time per
/// instance; a later frame's entries are in the next `since` window anyway.
fn round<R: Runtime>(app: &AppHandle<R>, id: &str, draw: bool) {
    let first = app
        .state::<Live>()
        .busy
        .lock()
        .map(|mut b| b.insert(id.to_string()))
        .unwrap_or(false);
    if !first {
        return;
    }
    let (app, id) = (app.clone(), id.to_owned());
    tauri::async_runtime::spawn(async move {
        read_inbox(&app, &id, draw).await;
        if let Ok(mut b) = app.state::<Live>().busy.lock() {
            b.remove(&id);
        }
    });
}

async fn read_inbox<R: Runtime>(app: &AppHandle<R>, id: &str, draw: bool) {
    let desktop = app.state::<Desktop>();
    let (Some(instance), Ok(token)) = (desktop.instance(id), desktop.token(id, "hub").await) else {
        return;
    };
    let Some(hub) = instance.product("hub") else {
        return;
    };
    let Ok(page) = fetch_unread(&desktop.http, hub, &token).await else {
        return;
    };
    let live = app.state::<Live>();
    if let Ok(mut unread) = live.unread.lock() {
        unread.insert(id.to_owned(), page.unread_count);
    }
    crate::shell::update_badge(app);
    if !draw {
        return;
    }
    let since = read_since(app, id);
    let entries = fresh(&page.entries, since.as_deref());
    if entries.is_empty() {
        return;
    }
    // Whatever is decided, these are seen: never drawn later, out of their moment.
    if let Some(next) = advance(since.as_deref(), &entries) {
        write_since(app, id, &next);
    }
    if crate::windows::instance_focused(app, id) {
        return;
    }
    for entry in entries {
        show(app, &live, id, &entry);
    }
}

fn show<R: Runtime>(app: &AppHandle<R>, live: &Live, id: &str, entry: &Entry) {
    let nid = live.remember(Target::Entry(
        id.to_owned(),
        entry.product.clone(),
        entry.link.clone(),
    ));
    let (title, body) = notification(entry);
    if let Err(error) = app
        .notification()
        .builder()
        .id(nid)
        .title(title)
        .body(body)
        .sound(system_sound())
        .show()
    {
        log::warn!("a notification could not be shown: {error}");
    }
}

/// Listen for clicks on the app's notifications.
pub fn listen_for_clicks<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let _ = app.notification().on_action(move |action| {
        if action.action_id() != "tap" {
            return;
        }
        let Some(nid) = action.notification().map(|n| n.id()) else {
            return;
        };
        let target = handle
            .state::<Live>()
            .targets
            .lock()
            .ok()
            .and_then(|t| t.iter().find(|(id, _)| *id == nid).map(|(_, t)| t.clone()));
        match target {
            Some(Target::Entry(instance, product, link)) => {
                let Some(found) = handle.state::<Desktop>().instance(&instance) else {
                    return;
                };
                let link = (!link.is_empty()).then_some(link.as_str());
                if crate::windows::open(&handle, &found, &product, link).is_err() {
                    let _ = crate::windows::open(&handle, &found, &product, None);
                }
            }
            Some(Target::Save(instance, file_id)) => {
                // The watch may have ended since (sent, dismissed, signed out): nothing to offer.
                if let Some(saved) = crate::files::current_save(&handle, &instance, &file_id) {
                    if let Some(found) = handle.state::<Desktop>().instance(&instance) {
                        let _ = crate::windows::open(&handle, &found, "drive", None);
                    }
                    crate::files::emit_saved(&handle, &saved);
                }
            }
            None => {}
        }
    });
}

/// Draw the notice of a saved file; a click brings the drive window forward and the offer back.
pub fn announce_saved<R: Runtime>(app: &AppHandle<R>, saved: &crate::files::Saved) {
    let (title, body) = crate::shell::saved_notice(app, &saved.name);
    let nid = app
        .state::<Live>()
        .remember(Target::Save(saved.instance.clone(), saved.file_id.clone()));
    if let Err(error) = app
        .notification()
        .builder()
        .id(nid)
        .title(title)
        .body(body)
        .show()
    {
        log::warn!("a notification could not be shown: {error}");
    }
}
