//! The products' windows. Opening a product opens a window on its own host
//! (`https://drive.<domain>`...), which signs in by the product's own browser session: the app adds
//! no token to its pages. A window stays on the instance's origins; a link anywhere else opens in
//! the system browser.
//!
//! A drive window may call the local copies' commands (`files.rs`), and nothing else: a capability
//! made at run time for that window and the drive's origin only.

use ruchoir_desktop_core::instances::Instance;
use ruchoir_desktop_core::navigation::{navigation, Navigation};
use tauri::ipc::CapabilityBuilder;
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

/// The commands a drive page may call.
const DRIVE_PERMISSIONS: &[&str] = &[
    "allow-file-open-local",
    "allow-file-dismiss",
    "allow-file-send-version",
    "allow-file-keep-copy",
];

/// The label of an instance's product window: `p-<instance id>-<product>`.
pub fn label(instance: &str, product: &str) -> String {
    format!("p-{instance}-{product}")
}

/// The instance id and product of a product window's label.
pub fn parse_label(label: &str) -> Option<(&str, &str)> {
    label.strip_prefix("p-")?.split_once('-')
}

/// A product's name as a window title: `drive` reads `Drive`.
fn title(product: &str, domain: &str) -> String {
    let mut chars = product.chars();
    let name: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    };
    format!("{name} - {domain}")
}

fn follow<R: Runtime>(app: &AppHandle<R>, url: &tauri::Url, origins: &[String]) -> bool {
    match navigation(url, origins) {
        Navigation::Stay => true,
        Navigation::OpenOutside => {
            let _ = app.opener().open_url(url.as_str(), None::<&str>);
            false
        }
        Navigation::Refuse => false,
    }
}

/// Open `product` of `instance` (at `at`, an address on its host, else its home), or bring forward
/// the window already showing it.
pub fn open<R: Runtime>(
    app: &AppHandle<R>,
    instance: &Instance,
    product: &str,
    at: Option<&str>,
) -> Result<(), String> {
    let origin = instance.product(product).ok_or("unknown_product")?;
    let home: tauri::Url = format!("{origin}/").parse().map_err(|_| "refused")?;
    let target = match at.map(str::parse::<tauri::Url>) {
        Some(Ok(url)) if url.origin() == home.origin() => url,
        Some(_) => return Err("refused".into()),
        None => home,
    };
    let label = label(&instance.id, product);
    if let Some(window) = app.get_webview_window(&label) {
        if at.is_some() {
            let _ = window.navigate(target);
        }
        let _ = window.show();
        let _ = window.unminimize();
        return window.set_focus().map_err(|_| "failed".to_owned());
    }
    if product == "drive" {
        let mut capability = CapabilityBuilder::new(format!("drive-{}", instance.id))
            .remote(format!("{origin}/*"))
            .window(label.clone());
        for permission in DRIVE_PERMISSIONS {
            capability = capability.permission(*permission);
        }
        app.add_capability(capability).map_err(|_| "failed")?;
    }
    let origins = instance.origins();
    let new_window_origins = origins.clone();
    let (nav_app, new_app) = (app.clone(), app.clone());
    WebviewWindowBuilder::new(app, &label, WebviewUrl::External(target))
        .title(title(product, &instance.domain))
        .inner_size(1200.0, 800.0)
        .min_inner_size(360.0, 480.0)
        .on_navigation(move |url| follow(&nav_app, url, &origins))
        // A link asked to open a new window: the instance's own stay out of a second window (the
        // product's tabs are its own pages), anything else goes to the system browser.
        .on_new_window(move |url, _| {
            follow(&new_app, &url, &new_window_origins);
            NewWindowResponse::Deny
        })
        .build()
        .map(|_| ())
        .map_err(|_| "failed".to_owned())
}

/// Close every window of an instance (its sign-out or removal).
pub fn close_instance<R: Runtime>(app: &AppHandle<R>, id: &str) {
    for (label, window) in app.webview_windows() {
        if parse_label(&label).is_some_and(|(instance, _)| instance == id) {
            let _ = window.destroy();
        }
    }
}

/// Whether a window of the instance is focused, where its pages show what is new themselves.
pub fn instance_focused<R: Runtime>(app: &AppHandle<R>, id: &str) -> bool {
    app.webview_windows().iter().any(|(label, window)| {
        parse_label(label).is_some_and(|(instance, _)| instance == id)
            && window.is_focused().unwrap_or(false)
    })
}
