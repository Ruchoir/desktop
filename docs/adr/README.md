# Decisions of this repository

Numbered from `0001`: `NNNN-short-title.md`, with a status, a date, the context, the decision and its
consequences. An accepted ADR is never rewritten: a later one supersedes it.

| ADR | Decision |
|---|---|
| [0001](0001-desktop-apps.md) | Desktop apps with Tauri, tokens kept by the core, distributed from Ruchoir's own server (moved from the former monolith) |

ADR 0001 was written for the monolith's desktop app, which embedded its web interface behind one app
origin. The app has since become a launcher opening each product in its own window
([`docs/desktop.md`](../desktop.md)), and the app-origin proxy is gone. Its choices of Tauri, of the
system's keychain, of signing in through the system browser and of distribution from Ruchoir's own
server still hold.

Decisions that touch several repositories live in
[Ruchoir/.github](https://github.com/Ruchoir/.github/tree/main/docs/adr).
