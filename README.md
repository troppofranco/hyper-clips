# Hyper Clips

**EN** — A lightweight, open-source, privacy-first clip recorder for Windows 10 22H2 / Windows 11, written in Rust. No telemetry, no accounts, no cloud: everything stays on your PC. The only network request ever made is the optional update check to GitHub Releases.

**IT** — Un registratore di clip leggero, open source e privacy-first per Windows 10 22H2 / Windows 11, scritto in Rust. Niente telemetria, niente account, niente cloud: tutto resta sul tuo PC. L'unica richiesta di rete è il controllo aggiornamenti (opzionale) verso GitHub Releases.

## Features / Funzioni (implemented)

| | |
|---|---|
| Instant replay | 15 s – 10 min buffer, hotkey **Alt+F10** saves the last N seconds |
| Manual recording | **Alt+F9** start/stop, written straight to MP4 |
| Screenshots | **Alt+F8**, PNG |
| Audio | System audio (WASAPI loopback) and microphone as separate AAC tracks |
| No yellow border | Borderless Windows Graphics Capture, falls back to Desktop Duplication |
| Game detection | Clips are tagged with the fullscreen program in front |
| Library | SQLite: favorites, tags, search, open, delete |
| Settings | Language (IT/EN), theme (dark/light/OLED), replay length, fps, bitrate, audio |
| Updates | GitHub Releases, SHA-256 + minisign signature, rollback (see `docs/RELEASING.md`) |
| Install | Per-user installer (IT/EN, silent mode) or portable zip |

Not done yet / Non ancora fatto: see `STATO.md`.

## Install

Download `HyperClips-Setup-<version>.exe` (or the portable zip) from the
[Releases](https://github.com/troppofranco/hyper-clips/releases) page. Windows SmartScreen may warn
because the binaries are not code-signed: choose "More info" → "Run anyway", and compare the file with
`checksums.txt`.

## Build

```
cargo build --release --workspace
```

Rust stable (≥ 1.89), Windows target. `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`,
`cargo deny check` must pass (CI enforces it).

## Layout

- `apps/hyperclips` — background service (tray, hotkeys, capture, IPC)
- `apps/hyperclips-ui` — egui window, started on demand
- `apps/hc-updater-helper` — swaps files during an update, with rollback
- `crates/hc-core` (config, i18n, IPC, rules) · `hc-buffer` (replay ring buffer) · `hc-capture-win` ·
  `hc-audio-win` · `hc-encode-win` · `hc-library` · `hc-os-win` · `hc-updater`

## License

GPL-3.0-or-later (the license text file still has to be added, see `STATO.md`).
