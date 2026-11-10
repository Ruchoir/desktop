//! Ruchoir for Windows, macOS and Linux.
//!
//! The main window is a small launcher bundled with the app (`ui/`): the instances the person added
//! and, for each, the products its manifest lists. Opening a product opens a window on the
//! product's own host, signed in by the product's own session (`windows.rs`). The app itself keeps
//! its tokens for what the pages do not do: the hub's live socket and notifications (`live.rs`) and
//! the local copies of drive files (`files.rs`). The logic that needs no window lives in
//! `ruchoir-desktop-core`.

// No console window behind the app on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod diagnostics;
mod files;
mod live;
mod shell;
mod state;
mod updater;
mod usage;
mod windows;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

fn main() {
    // A small runtime: the app mostly waits on a few sockets (spec, "Lightness budget").
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("the async runtime starts");
    tauri::async_runtime::set(runtime.handle().clone());

    tauri::Builder::default()
        // First, so a second launch is caught before anything else starts.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            shell::show_window(app);
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(diagnostics::plugin())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![shell::HIDDEN_ARG]),
        ))
        .invoke_handler(tauri::generate_handler![
            state::instances_state,
            state::instance_add,
            state::instance_sign_in,
            state::instance_sign_out,
            state::instance_remove,
            state::product_open,
            shell::desktop_settings,
            shell::set_desktop_settings,
            shell::set_shell_labels,
            shell::app_quit,
            files::file_open_local,
            files::file_dismiss,
            files::file_send_version,
            files::file_keep_copy,
            diagnostics::about_app,
            diagnostics::diagnostics,
            diagnostics::open_log_folder,
            updater::update_state,
            updater::update_install,
        ])
        .setup(|app| {
            log::info!(
                "Ruchoir {} starting on {} {}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            let list_path = app.path().app_data_dir()?.join("instances.json");
            app.manage(state::Desktop::new(
                list_path,
                ruchoir_desktop_core::vault::Keychain::open(),
            ));
            app.manage(live::Live::default());
            app.manage(shell::Shell::default());
            app.manage(files::Watcher::default());
            app.manage(updater::Updates::default());
            updater::watch(app.handle());
            live::listen_for_clicks(app.handle());
            files::start(app.handle());
            live::reconcile(app.handle());

            let opener = app.handle().clone();
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("Ruchoir")
                    .inner_size(420.0, 640.0)
                    .min_inner_size(320.0, 420.0)
                    // The launcher is the app's own page: a link out of it opens in the system browser.
                    .on_navigation(move |url| {
                        let own = matches!(url.scheme(), "tauri")
                            || url.host_str() == Some("tauri.localhost")
                            || url.host_str() == Some("localhost");
                        if !own && matches!(url.scheme(), "http" | "https" | "mailto") {
                            let _ = opener.opener().open_url(url.as_str(), None::<&str>);
                        }
                        own
                    });
            // Launch at login starts the app in the tray.
            let hidden = std::env::args().any(|a| a == shell::HIDDEN_ARG);
            let window = window.visible(!hidden).build()?;
            shell::build_tray(app.handle())?;
            // Closing hides to the tray when the settings say so.
            let handle = app.handle().clone();
            window.on_window_event(move |event| shell::on_window_event(&handle, event));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("the app builds")
        .run(|_app, _event| {
            // The dock icon clicked while the window is hidden brings it back, as a Mac app does.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                shell::show_window(_app);
            }
        });
}
