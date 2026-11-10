//! Local copies of drive files (decision 9 of the shells' plan). A drive page asks the app to open a
//! file with the computer's own app: the app reads its bytes from the drive with the drive access
//! token, keeps a copy in its cache and opens it; when the person saves it, the drive window is
//! offered to send it back, which goes up as a new version (`POST /api/v1/files/{id}/versions`).
//!
//! These commands are the only ones a drive window may call (`windows.rs`); the instance is the one
//! of the calling window, never a parameter, so a page acts on its own instance's files only.

use std::collections::HashSet;
use std::sync::{Mutex, PoisonError};
use std::time::{Instant, SystemTime};

use ruchoir_desktop_core::files::{
    download, forget_instance_copies, hash_bytes, hash_file, is_unsafe_to_open, local_path,
    open_root, safe_name, send_version, step, sweep_old_copies, DriveError, Seen, SessionEnd,
    Watched, Watches, LIFETIME, POLL,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, Webview};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

use crate::state::Desktop;

/// The instance of a drive window calling a command.
fn calling_instance<R: Runtime>(webview: &Webview<R>) -> Result<String, String> {
    match crate::windows::parse_label(webview.label()) {
        Some((instance, "drive")) => Ok(instance.to_owned()),
        _ => Err("refused".into()),
    }
}

/// A drive access token for the instance.
async fn drive_token<R: Runtime>(app: &AppHandle<R>, instance: &str) -> Result<String, String> {
    use ruchoir_native::NativeError;
    match app.state::<Desktop>().token(instance, "drive").await {
        Ok(token) => Ok(token),
        Err(NativeError::SessionEnded | NativeError::SignedOut) => {
            crate::state::session_expired(app, instance);
            Err("signed_out".into())
        }
        Err(_) => Err("unreachable".into()),
    }
}

/// The drive's origin for the instance.
fn drive_origin<R: Runtime>(app: &AppHandle<R>, instance: &str) -> Result<String, String> {
    app.state::<Desktop>()
        .instance(instance)
        .and_then(|i| i.product("drive").map(str::to_owned))
        .ok_or_else(|| "refused".to_owned())
}

/// Ask where to save `name` with the system's "Save as" dialog; `None` when the person cancelled.
async fn choose_save_path<R: Runtime>(
    app: &AppHandle<R>,
    name: &str,
) -> Result<Option<std::path::PathBuf>, String> {
    let name = safe_name(name);
    let asking = app.clone();
    // The dialog waits for the person: off the async workers, which keep the app's sockets.
    let chosen = tauri::async_runtime::spawn_blocking(move || {
        asking
            .dialog()
            .file()
            .set_file_name(name)
            .blocking_save_file()
    })
    .await
    .map_err(|_| "failed")?;
    match chosen {
        Some(chosen) => Ok(Some(chosen.into_path().map_err(|_| "failed")?)),
        None => Ok(None),
    }
}

/// What the interface is told when an opened file is saved (`file:saved`).
#[derive(Clone, Serialize)]
pub struct Saved {
    pub instance: String,
    pub file_id: String,
    pub name: String,
    pub base_version: String,
}

/// The copies being watched, and the uploads running. One upload per file at a time: a second
/// `file_send_version` for the same file answers `busy` rather than queueing (the banner is gone
/// as soon as the first one is accepted, so a second call is a double click).
#[derive(Default)]
pub struct Watcher {
    watches: Mutex<Watches>,
    sending: Mutex<HashSet<(String, String)>>,
}

impl Watcher {
    fn is_sending(&self, instance: &str, file_id: &str) -> bool {
        self.sending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&(instance.to_owned(), file_id.to_owned()))
    }
}

/// Holds a file's upload slot and gives it back however the upload ends.
struct Sending<'a> {
    watcher: &'a Watcher,
    key: (String, String),
}

impl<'a> Sending<'a> {
    fn take(watcher: &'a Watcher, instance: &str, file_id: &str) -> Option<Self> {
        let key = (instance.to_owned(), file_id.to_owned());
        let first = watcher
            .sending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key.clone());
        first.then_some(Self { watcher, key })
    }
}

impl Drop for Sending<'_> {
    fn drop(&mut self) {
        self.watcher
            .sending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// Tell the instance's drive window a copy was saved (`file:saved`).
pub fn emit_saved<R: Runtime>(app: &AppHandle<R>, saved: &Saved) {
    let label = crate::windows::label(&saved.instance, "drive");
    let _ = app.emit_to(label.as_str(), "file:saved", saved);
}

fn open_folder<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    Ok(open_root(
        &app.path().app_cache_dir().map_err(|_| "failed")?,
    ))
}

/// Download a file to the cache, open it with the computer's app and, when `base_version` is given
/// (the person may add a version), watch it for saves. The copy is replaced at each opening.
/// A copy holding an unsent edit is opened as it is, not replaced. Errors: `unsafe_type` (a file
/// the system would run), `refused` (not a drive window, or a bad id), `signed_out`, `forbidden`,
/// `unreachable`, `no_app` (nothing on the computer opens it), `failed`.
#[tauri::command]
pub async fn file_open_local<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    file_id: String,
    name: String,
    base_version: Option<String>,
) -> Result<(), String> {
    let instance = calling_instance(&webview)?;
    // The name as given and as it will be kept (`safe_name` may cut it down to an executable one).
    if is_unsafe_to_open(&name) {
        return Err("unsafe_type".into());
    }
    let root = open_folder(&app)?;
    let path = local_path(&root, &instance, &file_id, &name).ok_or("refused")?;
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_unsafe_to_open)
    {
        return Err("unsafe_type".into());
    }
    let watcher = app.state::<Watcher>();
    // A copy holding an edit that was not sent is never overwritten: it is opened as it is, and
    // its watch stays.
    if has_unsent_edit(&watcher, &instance, &file_id).await {
        return open_copy(&app, &path);
    }
    let drive = drive_origin(&app, &instance)?;
    let token = drive_token(&app, &instance).await?;
    let http = app.state::<Desktop>().http.clone();
    let bytes = match download(&http, &drive, &token, &file_id).await {
        Ok(bytes) => bytes,
        Err(DriveError::SignedOut) => {
            crate::state::session_expired(&app, &instance);
            return Err("signed_out".into());
        }
        Err(error) => return Err(error.code().into()),
    };
    // The watch first, with the new content's hash: rewriting the copy under an older watch would
    // let the poller read the new bytes as the person's edit.
    let watching = base_version.is_some();
    if let Some(base_version) = base_version {
        let watched = Watched {
            instance: instance.clone(),
            file_id: file_id.clone(),
            name,
            path: path.clone(),
            base_version,
            hash: hash_bytes(&bytes),
            mtime: None,
            since: Instant::now(),
            settling: None,
        };
        let evicted = watcher
            .watches
            .lock()
            .map_err(|_| "failed")?
            .insert(watched);
        // A copy no longer watched is not kept around.
        if let Some(parent) = evicted.as_ref().and_then(|e| e.path.parent()) {
            if parent.starts_with(&root) {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }
    // Opened without the right to add a version: no watch, and an earlier one for this file ends.
    if !watching {
        if let Ok(mut list) = watcher.watches.lock() {
            list.remove(&instance, &file_id);
        }
    }
    let stop = |app: &AppHandle<R>| {
        if watching {
            if let Ok(mut list) = app.state::<Watcher>().watches.lock() {
                list.remove(&instance, &file_id);
            }
        }
    };
    let folder = path.parent().ok_or("failed")?;
    if tokio::fs::create_dir_all(folder).await.is_err()
        || tokio::fs::write(&path, &bytes).await.is_err()
    {
        stop(&app);
        return Err("failed".into());
    }
    if let Err(error) = open_copy(&app, &path) {
        stop(&app);
        return Err(error);
    }
    Ok(())
}

fn open_copy<R: Runtime>(app: &AppHandle<R>, path: &std::path::Path) -> Result<(), String> {
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|_| "no_app".to_owned())
}

/// Whether the file is watched and its copy holds something not sent: a save waiting to settle, or
/// content that differs from what the watch last accepted.
async fn has_unsent_edit(watcher: &Watcher, instance: &str, file_id: &str) -> bool {
    let known = {
        let Ok(mut list) = watcher.watches.lock() else {
            return false;
        };
        list.find(instance, file_id)
            .map(|w| (w.path.clone(), w.hash, w.settling.is_some()))
    };
    let Some((path, hash, settling)) = known else {
        return false;
    };
    if settling {
        return true;
    }
    tauri::async_runtime::spawn_blocking(move || hash_file(&path).is_ok_and(|h| h != hash))
        .await
        .unwrap_or(false)
}

/// The person turned the offer down: the file is no longer watched (opening it again restarts it).
#[tauri::command]
pub fn file_dismiss<R: Runtime>(
    webview: Webview<R>,
    watcher: tauri::State<'_, Watcher>,
    file_id: String,
) -> Result<(), String> {
    let instance = calling_instance(&webview)?;
    if let Ok(mut list) = watcher.watches.lock() {
        list.remove(&instance, &file_id);
    }
    Ok(())
}

/// Send the saved copy as a new version of its file, with the drive access token. On success the
/// watch ends, unless a newer save is already waiting. Errors: `busy` (an upload of this file is
/// running), `not_watched` (the watch ended or never began), `unchanged` (the copy equals the version
/// it came from: nothing is uploaded), `signed_out` (the session is dropped, the copies stay),
/// `forbidden`, `too_large`, `unreachable`, `failed`.
#[tauri::command]
pub async fn file_send_version<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    file_id: String,
) -> Result<(), String> {
    let instance = calling_instance(&webview)?;
    let watcher = app.state::<Watcher>();
    let _slot = Sending::take(&watcher, &instance, &file_id).ok_or("busy")?;
    let (path, name) = {
        let mut list = watcher.watches.lock().map_err(|_| "failed")?;
        let w = list.find(&instance, &file_id).ok_or("not_watched")?;
        (w.path.clone(), w.name.clone())
    };
    let bytes = tokio::fs::read(&path).await.map_err(|_| "failed")?;
    let hash = hash_bytes(&bytes);
    // Nothing differs from the version the copy came from (an edit undone): nothing to upload.
    let same = watcher
        .watches
        .lock()
        .map_err(|_| "failed")?
        .find(&instance, &file_id)
        .is_some_and(|w| w.hash == hash);
    if same {
        return Err("unchanged".into());
    }
    let drive = drive_origin(&app, &instance)?;
    let token = drive_token(&app, &instance).await?;
    let http = app.state::<Desktop>().http.clone();
    match send_version(&http, &drive, &token, &file_id, &name, &bytes).await {
        Ok(sent) => {
            if let Ok(mut list) = watcher.watches.lock() {
                if let Some(w) = list.find(&instance, &file_id) {
                    w.mark_sent(hash);
                    // The answer is the file with its new current version: the next check is made
                    // against that one, not the version the copy was opened from.
                    if let Some(version) = sent {
                        w.base_version = version;
                    }
                    // A save made while this one was going up stays offered, or one the poller
                    // has not seen yet (the file's time moved since it looked).
                    let moved =
                        std::fs::metadata(&w.path).and_then(|m| m.modified()).ok() != w.mtime;
                    if w.settling.is_none() && !moved {
                        list.remove(&instance, &file_id);
                    }
                }
            }
            Ok(())
        }
        Err(DriveError::SignedOut) => {
            // The session is over: the copies stay, to be sent once signed in again.
            crate::state::session_expired(&app, &instance);
            Err("signed_out".into())
        }
        // Only the people allowed to edit the file may add a version: the drive decides.
        Err(error) => Err(error.code().into()),
    }
}

/// "Save as" for the local copy, nothing uploaded. False when the person cancelled.
#[tauri::command]
pub async fn file_keep_copy<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    file_id: String,
) -> Result<bool, String> {
    let instance = calling_instance(&webview)?;
    let (path, name) = {
        let watcher = app.state::<Watcher>();
        let mut list = watcher.watches.lock().map_err(|_| "failed")?;
        let w = list.find(&instance, &file_id).ok_or("not_watched")?;
        (w.path.clone(), w.name.clone())
    };
    let Some(target) = choose_save_path(&app, &name).await? else {
        return Ok(false);
    };
    tokio::fs::copy(&path, &target)
        .await
        .map_err(|_| "not_saved")?;
    Ok(true)
}

/// A session ended. A deliberate sign-out or removal stops the instance's watches, drops its save
/// notices and deletes its copies, and tells the interface (`file:forgotten`) to drop the banners.
/// An expired one keeps all of it: the person signs in again and the edited file can still be sent
/// or kept.
pub fn session_ended<R: Runtime>(app: &AppHandle<R>, instance: &str, end: SessionEnd) {
    if end.keeps_copies() {
        return;
    }
    if let Some(watcher) = app.try_state::<Watcher>() {
        if let Ok(mut list) = watcher.watches.lock() {
            list.forget_instance(instance);
        }
    }
    if let Ok(root) = open_folder(app) {
        forget_instance_copies(&root, instance);
    }
    let label = crate::windows::label(instance, "drive");
    let _ = app.emit_to(label.as_str(), "file:forgotten", instance);
}

/// The offer for a file that is still watched, or `None`.
pub fn current_save<R: Runtime>(
    app: &AppHandle<R>,
    instance: &str,
    file_id: &str,
) -> Option<Saved> {
    let watcher = app.state::<Watcher>();
    let mut list = watcher.watches.lock().ok()?;
    let w = list.find(instance, file_id)?;
    Some(Saved {
        instance: w.instance.clone(),
        file_id: w.file_id.clone(),
        name: w.name.clone(),
        base_version: w.base_version.clone(),
    })
}

/// Start the watching task and clear the copies left by earlier runs. Called once, in `setup`.
pub fn start<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(root) = open_folder(app) {
        // Off the setup path: a big cache folder must not hold the window back.
        tauri::async_runtime::spawn_blocking(move || {
            sweep_old_copies(&root, SystemTime::now(), LIFETIME);
        });
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(POLL);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            round(&app).await;
        }
    });
}

/// The modification time and, only when it moved since `last`, the hash. `None` when the time is
/// unchanged or the file cannot be read right now (an editor may swap it for a moment).
fn look(path: &std::path::Path, last: Option<SystemTime>) -> Option<(SystemTime, [u8; 32])> {
    let mtime = std::fs::metadata(path).ok()?.modified().ok()?;
    if Some(mtime) == last {
        return None;
    }
    Some((mtime, hash_file(path).ok()?))
}

async fn round<R: Runtime>(app: &AppHandle<R>) {
    let watcher = app.state::<Watcher>();
    // Nothing watched: nothing to do but wait for the next tick.
    let snapshot = match watcher.watches.lock() {
        Ok(list) if !list.is_empty() => list.snapshot(),
        _ => return,
    };
    for (instance, file_id, path, last) in snapshot {
        // An upload is running: its outcome decides what comes next, so this file is left alone.
        if watcher.is_sending(&instance, &file_id) {
            continue;
        }
        let observed = tauri::async_runtime::spawn_blocking(move || look(&path, last))
            .await
            .ok()
            .flatten();
        let now = Instant::now();
        let saved = {
            let Ok(mut list) = watcher.watches.lock() else {
                return;
            };
            let Some(w) = list.find(&instance, &file_id) else {
                continue;
            };
            match step(w, now, observed) {
                Seen::Saved(_) => {
                    let saved = Saved {
                        instance: w.instance.clone(),
                        file_id: w.file_id.clone(),
                        name: w.name.clone(),
                        base_version: w.base_version.clone(),
                    };
                    // Emitted while the watch is known to exist (the lock is held).
                    emit_saved(app, &saved);
                    Some(saved)
                }
                Seen::Expired => {
                    list.remove(&instance, &file_id);
                    None
                }
                Seen::Unchanged | Seen::Settling => None,
            }
        };
        if let Some(saved) = saved {
            // The notice only if the watch is still there (a sign-out may have come in between).
            if current_save(app, &saved.instance, &saved.file_id).is_some() {
                crate::live::announce_saved(app, &saved);
            }
        }
    }
}
