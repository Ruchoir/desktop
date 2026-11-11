# Ruchoir desktop

The desktop app of Ruchoir, a self-hosted, European collaboration suite: chat, files, calendar, with
one identity service. It is a light Tauri shell for Windows, macOS and Linux: a small launcher lists
the Ruchoir instances you belong to, and opens each of their products in its own window, with
system notifications and local copies of drive files.

[Propolis](https://github.com/Ruchoir/propolis) signs you in, in the system browser; [the
hub](https://github.com/Ruchoir/hub) describes each instance in its manifest and brings the live
socket and the inbox the notifications come from; [the installer](https://github.com/Ruchoir/installer)
registers the app as a client of every instance; [core](https://github.com/Ruchoir/core) holds the
sign-in library it builds on (`ruchoir-native`). The windows show the products' own pages: [the
drive](https://github.com/Ruchoir/drive), [the calendar](https://github.com/Ruchoir/calendar) and
[the chat](https://github.com/Ruchoir/chat), and the app reads and sends back the drive's files for
the local copies. How the pieces fit: [architecture](https://github.com/Ruchoir/.github/blob/main/docs/architecture.md).

## Status

The app is built for the three systems: instances added from their hub's manifest, signing in with
Propolis in the system browser, the tokens in the system's keychain, a window per product, system
notifications and the unread badge from the hub, the tray and launch at login, the update check, and
the local copies of drive files on the app's side. A tag builds, signs and publishes a release
(`.github/workflows/release.yml`), and installed apps update from `https://ruchoir.com/desktop/`,
never from GitHub.

Not there yet: the launcher's settings, "About" and update offer (the app's commands exist, the
launcher does not show them), the drive's side of the local copies, a product rail in one window,
and an end to end run on the launcher (`e2e/run.sh` still drives the former monolith's app). Windows
builds are not signed yet. [`docs/desktop.md`](docs/desktop.md) has the details.

## Prerequisites

- Rust: the version pinned in `rust-toolchain.toml` (rustup installs it on first use).
- Node 24 (`.nvmrc`), for the Tauri CLI that `package.json` pins.
- On Linux (Debian, Ubuntu), the system's webview development files: `libwebkit2gtk-4.1-dev`,
  `libayatana-appindicator3-dev`, `librsvg2-dev`, `libsecret-1-dev` (and `xdg-utils`,
  `desktop-file-utils` to bundle). On Windows, `OPENSSL_DIR` set; on macOS, the Xcode command line
  tools.

## Running it

```bash
npx --yes @tauri-apps/cli@2.12.3 dev
```

builds the app in debug and opens the launcher. Type an instance's domain to add it: the app reads
`https://hub.<domain>/.well-known/ruchoir-manifest`, then signs in through the system browser. A
debug build also takes `http://localhost:<port>`, for a stand-in hub on the same machine. To have an
instance to try it on, the installer's development mode runs one locally: see [getting
started](https://github.com/Ruchoir/.github/blob/main/docs/getting-started.md).

A release build, as the release workflow makes it:

```bash
npx --yes @tauri-apps/cli@2.12.3 build --no-bundle    # the app alone, target/release/ruchoir-desktop
npx --yes @tauri-apps/cli@2.12.3 build                # installers for this system, in target/release/bundle/
```

## Testing it

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The logic that needs no window (`core/`) is tested on any machine, against stand-ins. The lightness
budget, on Linux, against a release build (it fails above 250 MB of memory or 1 % processor at
rest):

```bash
RUCHOIR_DESKTOP_BIN=$PWD/target/release/ruchoir-desktop e2e/budget.sh
```

The pull request CI runs the conventions only; the release workflow runs the checks above, so run
them before pushing.

## Settings

The app reads no environment variable: its own settings (launch at login, keep in the tray, the
anonymous usage statistics) are kept in its data folder. What a build needs is in
[`AGENTS.md`](AGENTS.md), "Layout" and "Checking the work".

## Documentation

- [`docs/desktop.md`](docs/desktop.md): how the app fits together, signing in, the windows,
  notifications, local copies, updates, logs, usage statistics, building, signing and releasing
  (the secrets the release needs are in [`AGENTS.md`](AGENTS.md), "Releasing").
- [`docs/adr/`](docs/adr/README.md): the decisions of this repository.
- [`AGENTS.md`](AGENTS.md): how to work here, written for contributors and their tools. It opens
  with the common rules of the organization.
- The decisions that span several repositories:
  [Ruchoir/.github, `docs/adr`](https://github.com/Ruchoir/.github/tree/main/docs/adr).

## Contributing

Contributions are welcome. Read [`CONTRIBUTING.md`](https://github.com/Ruchoir/.github/blob/main/CONTRIBUTING.md):
your first pull request asks you to sign the [contributor licence agreement](https://github.com/Ruchoir/.github/blob/main/CLA.md).
Report a vulnerability privately, never in an issue: [`SECURITY.md`](https://github.com/Ruchoir/.github/blob/main/SECURITY.md).

## Licence

`AGPL-3.0-only` (`LICENSE`).
