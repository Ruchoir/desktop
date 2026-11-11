# The desktop app (Windows, macOS, Linux)

Moved from `docs/desktop.md` of the former monolith (`Ruchoir/ruchoir`) on 2026-11-11, and brought up to date with this repository.

This repository is Ruchoir for the desktop, built with Tauri 2: a small launcher bundled with the
app, a window per product on the product's own host, and a Rust core that holds each instance's
sign-in, live socket and notifications. Why Tauri, the keychain and Ruchoir's own server for
updates: [ADR 0001](adr/0001-desktop-apps.md). That ADR was written for the former monolith's app,
which embedded its web interface behind an app origin and a proxy; the app described here no longer
does.

## How it fits together

```
launcher (ui/, bundled) ── commands, never a token ──► Rust app (src-tauri/)
                                                         ├─ instances, from each hub's manifest
                                                         ├─ sign-in with Propolis in the system browser
                                                         ├─ per signed-in instance: the hub's live socket
                                                         │     ──► system notifications, the badge
                                                         └─ a window per product ──► https://<product>.<domain>
```

- `core/`: `ruchoir-desktop-core`, the logic that needs no window: the instances and their manifests
  (`instances.rs`), signing in (`accounts.rs`, on `ruchoir-native` from `Ruchoir/core`), the
  keychain (`vault.rs`), the live socket (`watcher.rs`), the inbox (`inbox.rs`), where a product's
  window may go (`navigation.rs`) and the local copies of drive files (`files.rs`). Tested on any
  machine, against stand-ins.
- `src-tauri/`: `ruchoir-desktop`, the app: the commands of the launcher (`state.rs`), the
  products' windows (`windows.rs`), the live sockets and notifications (`live.rs`), the tray, the
  badge and launch at login (`shell.rs`), the local copies (`files.rs`), updates (`updater.rs`), the
  usage statistics (`usage.rs`) and the log (`diagnostics.rs`). `build.rs` lists the commands and
  `capabilities/main.json` grants them to the launcher.
- `ui/`: the launcher, plain HTML and a JavaScript module with no build step, and its six
  dictionaries in `ui/i18n/`, French first.

## Adding an instance and signing in

- The person types their instance's domain. The app reads its hub's manifest
  (`https://hub.<domain>/.well-known/ruchoir-manifest`): Propolis's origin and the installed
  products with their origins. The list is kept in a small JSON file of the app's data folder;
  nothing secret is in it.
- Signing in opens the system browser on Propolis (RFC 8252: PKCE and a loopback redirect,
  `http://127.0.0.1:<port>/callback`), and asks at once for a token for the hub and for every
  installed product. The refresh token, one per issuer, goes to the system's keychain (macOS
  Keychain, Windows Credential Manager, the Secret Service on Linux); access tokens stay in memory.
  A Linux desktop without a Secret Service keeps them in memory only, and the launcher says so.
- No command returns a token and no event carries one: the launcher never sees one.
- A release build reaches instances over `https` only. A debug build also takes
  `http://127.0.0.1:<port>` or `http://localhost:<port>`, for a stand-in hub on the same machine.

## The products' windows

- Opening a product opens a window on its own host (`https://drive.<domain>`...), signed in by the
  product's own browser session: the app adds no token to its pages. A window stays on the
  instance's origins (its products' and Propolis's); a link anywhere else opens in the system
  browser, so a remote page never sits in a Ruchoir window.
- A product's window can call no command, except a drive window, which gets the local copies'
  commands through a capability made at run time for that window and the drive's origin only.

## Notifications and the badge

- Each signed-in instance has one live socket to its hub (`/api/v1/live/ws`), opened with the app's
  own access token for the hub, without `Origin`; a reconnection asks a fresh token.
- On `hub.inbox.new`, the app reads the unread entries (`GET /api/v1/me/inbox?unread=true`) and
  draws those newer than the last one drawn as system notifications, unless a window of that
  instance is focused. The hub already applied the person's settings; the app only keeps from
  drawing an entry twice. A click opens the product's window at the entry's link. Where it stopped
  is kept per instance across restarts, and starts when the socket first opens, so a backlog is
  never drawn.
- On `hub.inbox.read`, and at each (re)connection, the unread count is read again for the badge. A
  refused token ends the instance's session.

## On the desktop

- A tray icon, with "Open Ruchoir" and "Quit", and the total of unread notifications of every
  instance on the app's icon (macOS label, Windows overlay, Linux launcher count).
- Closing the window hides it: on Windows and Linux while "keep in the tray" is on (the default), on
  macOS always. The first time, a notification says where it went.
- Launch at login starts the app with `--hidden`: no window until the tray is clicked.
- A second launch brings the running app's window forward.
- Launch at login, keep in the tray and the usage statistics are the app's settings
  (`desktop_settings`, `set_desktop_settings`). The launcher does not show them yet.

## Local copies of drive files

`core/src/files.rs` and `src-tauri/src/files.rs`. A drive window asks the app to open a file with
the computer's own app (`file_open_local`): the app reads the bytes from the drive
(`GET /api/v1/files/{id}/download`, with its drive access token), keeps a copy in its cache
(`<cache>/<instance>/<file id>/<name>`) and hands it to the system.

- The copy is replaced at each opening, except one holding an edit not sent, which is opened as it
  is. A file whose name ends in an executable extension is never opened (`unsafe_type`).
- When the person may add a version (the page passes the version it opened), the copy is watched: a
  new content counts as a save once it stood two seconds unchanged, a copy is watched for a day, and
  twenty at most at once. A save is told to the drive window (`file:saved`) and as a system
  notification ("File saved"), whose click brings the drive window forward.
- `file_send_version` sends the copy as a new version (`POST /api/v1/files/{id}/versions`), and
  uploads nothing when it equals the version it came from (`unchanged`); the drive decides who may
  add one. `file_keep_copy` saves it elsewhere through the system's "Save as" dialog, and
  `file_dismiss` stops watching it.
- Signing out of an instance or removing it deletes its copies and tells its windows
  (`file:forgotten`); an expired session keeps them, to be sent once signed in again.

The drive's pages do not call these commands yet: the app's side is ready, the drive's is to come.

## Updates

The app asks `https://ruchoir.com/desktop/latest.json` at launch and every six hours
(`src-tauri/src/updater.rs`); a debug build never does. What it offers is signed with the updater's
own key (minisign), whose public half is in `src-tauri/tauri.conf.json`, so nothing but a Ruchoir
release installs. A newer version is announced to the launcher (`update:available`) and installed
on the person's request (`update_install`): downloaded, its signature checked, installed, and the
app started again. The launcher does not show the offer yet. Windows (NSIS), macOS and the AppImage
update themselves; a `.deb` or `.rpm` install is not updated by the app (it would need signed
packages in the manifest, and the administrator's password each time), and is pointed at the
download page instead. A failed check or download changes nothing and is tried again at the next
check.

The updater's key pair is the app's own, apart from the systems' code signing certificates. The
private key and its password are the repository's secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, with a copy kept by the maintainer. **Losing it means no
installed app can ever update again**: every install would have to be replaced by hand with a build
carrying a new public key.

## Logs and diagnostics

The app writes `ruchoir.log` in the system's log folder for it (rotated at 1 MB, three old files
kept): one line per thing worth knowing (start, an update found, a check or a notification that
failed), never what a message says and never a token. `about_app` gives the version, the system and
the web engine; `diagnostics` the same with the installation format, the instances counted and named
by host, and the log's last 200 lines, as text the person copies where they choose;
`open_log_folder` opens the folder. Nothing is sent anywhere by itself. The launcher does not offer
these yet.

## Usage statistics

Ruchoir counts its desktop apps, anonymously (`src-tauri/src/usage.rs`). The first time an app
checks for updates it draws a random identifier (128 bits), unrelated to any account, instance or
person, and sends it with each check (`X-Ruchoir-Install`), with its version (`X-Ruchoir-Version`)
and installation format (`X-Ruchoir-Format`). The website's server logs those three fields without
the address, and counts the identifiers seen in a day, a week and a month. Turning the setting off
(`usage_count`) sends nothing more and erases the identifier; turning it on again draws a new one.

## Building

On Linux (Debian, Ubuntu), the system's webview development files:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libsecret-1-dev xdg-utils desktop-file-utils
```

Windows needs WebView2 (part of Windows 10 and 11) and `OPENSSL_DIR` set; macOS needs the Xcode
command line tools. The Tauri CLI is the one `package.json` pins, run as the release workflow runs
it:

```bash
npx --yes @tauri-apps/cli@2.12.3 dev                  # the app, in a debug build
npx --yes @tauri-apps/cli@2.12.3 build --no-bundle    # the app alone, target/release/ruchoir-desktop
npx --yes @tauri-apps/cli@2.12.3 build                # installers for this system, in target/release/bundle/
```

## Checks

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

and `cargo deny --config "$RULES/deny.toml" check`, with a clone of `Ruchoir/.github` at `$RULES`.
The pull request CI runs the conventions only (the shared Rust workflow has no webview libraries);
the release workflow runs the Rust checks, so run them before pushing.

### The lightness budget

`e2e/budget.sh` starts the release build with no instance, window shown, and fails above 250 MB of
proportional memory (the app and its WebKit processes) or 1 % processor over 60 idle seconds
(`BUDGET_RSS_MB`, `BUDGET_CPU_PERCENT`):

```bash
RUCHOIR_DESKTOP_BIN=$PWD/target/release/ruchoir-desktop e2e/budget.sh
```

Under Xvfb it turns WebKit's DMA-BUF renderer off: without a GPU it falls back to software OpenGL,
which no desktop with a graphics card pays. `e2e/budget-report.mjs` measures the same on Windows (the
app and its WebView2 processes) and macOS (the app and the WebKit services), and only reports: the
release workflow writes its figures in the run's summary. Its runners have no GPU either, so the
figures are an upper bound. On real machines, read the memory in the task manager at each release.

### The end to end check

`e2e/run.sh` drives the app through `tauri-driver` (WebDriver) on a headless Linux machine. It still
drives the former monolith's app (signing in through its `/n/` page, a channel of its embedded
interface) and does not fit the launcher yet. Native notifications, their click, the badge and the
tray are checked by hand in any case: WebDriver cannot see them. macOS is checked by a volunteer
before each release.

## Signing

- **macOS**: a release is signed with the "Developer ID Application" certificate (valid until
  2027-12-02) and notarised by Apple with an App Store Connect key (`APPLE_API_*`). Secrets:
  `APPLE_CERTIFICATE` (the `.p12`, base64), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
  `APPLE_API_ISSUER_ID`, `APPLE_API_KEY_ID`, `APPLE_API_KEY_P8`. The release checks the signature,
  Gatekeeper's verdict and the stapled ticket before publishing. Renew the certificate before it
  expires.
- **Every file**: each release has `SHA256SUMS.txt`, signed (`SHA256SUMS.txt.asc`) with Ruchoir's
  release key, `Ruchoir Releases <contact@ruchoir.com>`, fingerprint
  `D868 1A64 9CBE 0E81 1CA1 7BE3 9A78 091C DFC5 D3CB`, valid until 2029-11-01. Its public half is
  `release/ruchoir-release.asc`, published at `https://ruchoir.com/desktop/ruchoir-release.asc`; the
  private half and its passphrase are the secrets `RELEASE_GPG_KEY` and `RELEASE_GPG_PASSPHRASE`. To
  check a download:

  ```bash
  curl -sO https://ruchoir.com/desktop/ruchoir-release.asc && gpg --import ruchoir-release.asc
  gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt && sha256sum --ignore-missing -c SHA256SUMS.txt
  ```

- **Windows**: the installer is not signed yet (an application for SignPath's free signing of
  open-source projects was made on 2026-11-02), so SmartScreen warns at the first install ("More
  info", "Run anyway").

## Releasing

A tag `vX.Y.Z` runs `.github/workflows/release.yml`:

1. On Linux, the checks above, a release build and the lightness budget.
2. One build per system with the tag's version, the updater's archives signed: an AppImage, a `.deb`
   and an `.rpm`; an NSIS installer; a universal `.app` and `.dmg`, signed and notarised.
3. In the `production` environment, `release/assemble.mjs` writes `latest.json` (the updater's
   manifest), `downloads.json` and `index.html` (a plain download page), `SHA256SUMS.txt` is signed,
   and the whole goes to the website's server through its `deploy` account (`DESKTOP_DEPLOY_SSH_KEY`,
   `DEPLOY_KNOWN_HOSTS`, and the variable `DEPLOY_HOST`).

There, the key is bound to `release/desktop-publish-entry`, which can only start
`release/desktop-publish` with a version (both in `/usr/local/sbin`, their headers say how they are
installed). It puts the files in `/srv/ruchoir-desktop/vX.Y.Z/`, then `downloads.json`, `index.html`
and `latest.json` at the top, last, so an app never reads a manifest naming files not there yet.
Older versions stay. nginx serves the folder as `https://ruchoir.com/desktop/`, and the log is
`/var/log/desktop-publish.log`.

The repository has no Actions minutes yet: the workflow runs once billing or a self-hosted runner
exists, and until then a release is built by hand. The secrets are recreated in the repository's
settings, never in a file ([`AGENTS.md`](../AGENTS.md), "Releasing").

A person installs the first version by hand, from `https://ruchoir.com/desktop/`.
