/// Every command of `generate_handler!` (`src/main.rs`). With an app manifest each one needs a
/// permission, granted to the launcher (`capabilities/main.json`); a drive window gets the files
/// commands only, by a capability made at run time (`src/windows.rs`). A product window can call
/// nothing else. Add a command here and in the capability.
const COMMANDS: &[&str] = &[
    "instances_state",
    "instance_add",
    "instance_sign_in",
    "instance_sign_out",
    "instance_remove",
    "product_open",
    "desktop_settings",
    "set_desktop_settings",
    "set_shell_labels",
    "app_quit",
    "file_open_local",
    "file_dismiss",
    "file_send_version",
    "file_keep_copy",
    "about_app",
    "diagnostics",
    "open_log_folder",
    "update_state",
    "update_install",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("the app's command permissions are generated");
}
