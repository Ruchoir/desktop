//! The desktop app's logic that needs no window.
//!
//! Everything here runs without Tauri or a webview, so it is tested on any machine. `src-tauri` is
//! the thin glue that wires these pieces to the windows and the system's keychain.

pub mod accounts;
pub mod files;
pub mod inbox;
pub mod instances;
pub mod navigation;
pub mod vault;
pub mod watcher;

/// The HTTP client of the app's own requests to the products (the inbox, the drive's bytes and
/// versions): rustls with the ring provider, as `ruchoir-native`'s, no redirect followed, no cookie
/// store, and a user agent that names the app.
pub fn http_client() -> reqwest::Client {
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(format!(
            "Ruchoir-Desktop/{} ({})",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        ))
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("the HTTP client builds")
}

/// The HTTP client's type, for the app to hold one without naming `reqwest` itself.
pub use reqwest::Client as HttpClient;
