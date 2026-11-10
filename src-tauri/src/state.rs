//! What the app holds while it runs: the instance list, the native client and its grants, and the
//! commands the launcher calls to drive them.
//!
//! Adding an instance is: the domain the person typed, discovery from the hub's manifest, then a
//! sign-in through the system browser (RFC 8252, loopback redirect) asking a token for the hub and
//! every installed product. The launcher learns of every change through `instances:changed`, and
//! never receives a token: no command returns one, and no event carries one.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use ruchoir_desktop_core::accounts::{self, confirmation_page, error_code, SIGN_IN_WITHIN};
use ruchoir_desktop_core::files::SessionEnd;
use ruchoir_desktop_core::instances::{discovery_address, Instance, InstanceList};
use ruchoir_desktop_core::vault::Keychain;
use ruchoir_native::{Browser, Loopback, NativeError};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State, Webview};
use tauri_plugin_opener::OpenerExt;

/// Everything the app keeps between two commands.
pub struct Desktop {
    pub list: Mutex<InstanceList>,
    pub client: accounts::Client,
    pub http: ruchoir_desktop_core::HttpClient,
    pub list_path: PathBuf,
    persistent: bool,
    /// The instances whose sign-in is waiting on the browser.
    signing_in: Mutex<HashSet<String>>,
}

/// A product of an instance, as the launcher lists it.
#[derive(Debug, Clone, Serialize)]
pub struct ProductView {
    pub name: String,
    pub url: String,
}

/// What the launcher knows of an instance.
#[derive(Debug, Clone, Serialize)]
pub struct InstanceView {
    pub id: String,
    pub domain: String,
    pub signed_in: bool,
    pub signing_in: bool,
    /// The signed-in person's name, from the ID token.
    pub account: Option<String>,
    pub products: Vec<ProductView>,
    /// Whether the grant survives quitting the app (false without a keychain).
    pub vault_persistent: bool,
}

/// The system browser, through the opener plugin.
struct SystemBrowser<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Browser for SystemBrowser<R> {
    fn open(&self, url: &str) -> Result<(), String> {
        self.0
            .opener()
            .open_url(url, None::<&str>)
            .map_err(|e| e.to_string())
    }
}

impl Desktop {
    pub fn new(list_path: PathBuf, keychain: Keychain) -> Self {
        Self {
            list: Mutex::new(InstanceList::load(&list_path)),
            persistent: keychain.persistent(),
            client: accounts::client(keychain),
            http: ruchoir_desktop_core::http_client(),
            list_path,
            signing_in: Mutex::new(HashSet::new()),
        }
    }

    /// An instance by id.
    pub fn instance(&self, id: &str) -> Option<Instance> {
        self.list.lock().ok()?.get(id).cloned()
    }

    /// Every instance, in the list's order.
    pub fn instances(&self) -> Vec<Instance> {
        self.list
            .lock()
            .map(|l| l.instances.clone())
            .unwrap_or_default()
    }

    /// Whether a grant is kept for the instance.
    pub async fn signed_in(&self, instance: &Instance) -> bool {
        matches!(self.client.account(&instance.issuer).await, Ok(Some(_)))
    }

    /// Every instance as the launcher sees it.
    pub async fn views(&self) -> Vec<InstanceView> {
        let signing_in = self
            .signing_in
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        let mut views = Vec::new();
        for instance in self.instances() {
            let account = self.client.account(&instance.issuer).await.ok().flatten();
            views.push(InstanceView {
                signed_in: account.is_some(),
                signing_in: signing_in.contains(&instance.id),
                account: account.and_then(|a| a.name.or(a.email)),
                products: instance
                    .products
                    .iter()
                    .map(|p| ProductView {
                        name: p.name.clone(),
                        url: p.url.clone(),
                    })
                    .collect(),
                id: instance.id,
                domain: instance.domain,
                vault_persistent: self.persistent,
            });
        }
        views
    }

    /// An access token for the instance's `product` (`hub`, `drive`...), from memory or refreshed.
    pub async fn token(&self, id: &str, product: &str) -> Result<String, NativeError> {
        let instance = self.instance(id).ok_or(NativeError::SignedOut)?;
        let resource = instance.product(product).ok_or(NativeError::SignedOut)?;
        self.client.access_token(&instance.issuer, resource).await
    }

    fn save(&self, list: &InstanceList) -> Result<(), String> {
        list.save(&self.list_path)
            .map_err(|_| "internal".to_string())
    }
}

/// Tell the launcher the instances changed, and start or stop the live sockets to match.
pub fn changed<R: Runtime>(app: &AppHandle<R>) {
    crate::live::reconcile(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let views = app.state::<Desktop>().views().await;
        let _ = app.emit_to("main", "instances:changed", views);
    });
}

/// Refuse a command from anything but the launcher: a product's page may call the files commands
/// only.
pub fn launcher_only<R: Runtime>(webview: &Webview<R>) -> Result<(), String> {
    if webview.label() == "main" {
        Ok(())
    } else {
        Err("refused".into())
    }
}

#[tauri::command]
pub async fn instances_state<R: Runtime>(
    webview: Webview<R>,
    desktop: State<'_, Desktop>,
) -> Result<Vec<InstanceView>, String> {
    launcher_only(&webview)?;
    Ok(desktop.views().await)
}

/// Add the instance at `address` (its domain), from its hub's manifest, then sign in to it. `lang`
/// is the launcher's language, for the page the browser shows once back. Errors are codes the
/// launcher translates.
#[tauri::command]
pub async fn instance_add<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    desktop: State<'_, Desktop>,
    address: String,
    lang: String,
) -> Result<String, String> {
    launcher_only(&webview)?;
    let address =
        discovery_address(&address, cfg!(debug_assertions)).map_err(|e| e.code().to_owned())?;
    let found = ruchoir_native::discovery::discover_instance(&desktop.http, &address)
        .await
        .map_err(|e| error_code(&e).to_owned())?;
    if !found.products.iter().any(|p| p.name == "hub") {
        return Err("not_ruchoir".into());
    }
    let instance = {
        let mut list = desktop.list.lock().map_err(|_| "internal")?;
        let instance = list.add(Instance::from(&found));
        desktop.save(&list)?;
        instance
    };
    changed(&app);
    sign_in(&app, &desktop, &instance.id, &lang).await?;
    Ok(instance.id)
}

/// Sign in again to an instance already in the list.
#[tauri::command]
pub async fn instance_sign_in<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    desktop: State<'_, Desktop>,
    id: String,
    lang: String,
) -> Result<(), String> {
    launcher_only(&webview)?;
    sign_in(&app, &desktop, &id, &lang).await
}

async fn sign_in<R: Runtime>(
    app: &AppHandle<R>,
    desktop: &Desktop,
    id: &str,
    lang: &str,
) -> Result<(), String> {
    let instance = desktop.instance(id).ok_or("unknown_instance")?;
    let first = desktop
        .signing_in
        .lock()
        .map(|mut s| s.insert(id.to_owned()))
        .unwrap_or(false);
    if !first {
        return Err("busy".into());
    }
    changed(app);
    let result = async {
        let loopback = Loopback::bind(confirmation_page(lang), SIGN_IN_WITHIN)
            .await
            .map_err(|_| "browser".to_owned())?;
        desktop
            .client
            .sign_in(
                &accounts::target(&instance),
                &SystemBrowser(app.clone()),
                loopback,
            )
            .await
            .map_err(|e| {
                log::warn!("a sign-in failed: {}", error_code(&e));
                error_code(&e).to_owned()
            })
    }
    .await;
    if let Ok(mut s) = desktop.signing_in.lock() {
        s.remove(id);
    }
    changed(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    result.map(|_| ())
}

/// End the session on an instance: the grant is revoked and forgotten, its windows close, its
/// copies go. The instance stays in the list, signed out.
#[tauri::command]
pub async fn instance_sign_out<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    desktop: State<'_, Desktop>,
    id: String,
) -> Result<(), String> {
    launcher_only(&webview)?;
    sign_out(&app, &desktop, &id).await;
    changed(&app);
    Ok(())
}

async fn sign_out<R: Runtime>(app: &AppHandle<R>, desktop: &Desktop, id: &str) {
    if let Some(instance) = desktop.instance(id) {
        let _ = desktop.client.sign_out(&instance.issuer).await;
    }
    crate::windows::close_instance(app, id);
    crate::files::session_ended(app, id, SessionEnd::Deliberate);
    crate::live::forget(app, id);
}

/// Forget an instance: its session, its windows, its copies, its place in the list.
#[tauri::command]
pub async fn instance_remove<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    desktop: State<'_, Desktop>,
    id: String,
) -> Result<(), String> {
    launcher_only(&webview)?;
    sign_out(&app, &desktop, &id).await;
    {
        let mut list = desktop.list.lock().map_err(|_| "internal")?;
        list.remove(&id);
        desktop.save(&list)?;
    }
    changed(&app);
    Ok(())
}

/// Open one of an instance's products in its own window, on the product's host.
#[tauri::command]
pub fn product_open<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    desktop: State<'_, Desktop>,
    id: String,
    product: String,
) -> Result<(), String> {
    launcher_only(&webview)?;
    let instance = desktop.instance(&id).ok_or("unknown_instance")?;
    crate::windows::open(&app, &instance, &product, None)
}

/// The session of an instance ended on its own (a refused token): the copies stay, so an edited
/// file can still be sent once the person signs in again.
pub fn session_expired<R: Runtime>(app: &AppHandle<R>, id: &str) {
    crate::files::session_ended(app, id, SessionEnd::Expired);
    changed(app);
}
