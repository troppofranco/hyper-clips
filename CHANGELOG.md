# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/it/1.1.0/) · Versioni: [SemVer](https://semver.org/lang/it/).

## [Unreleased]

### Fase 1 — Core
- Workspace Cargo (edizione 2024), profilo release ottimizzato (LTO fat, `panic=abort`, strip).
- `hc-core`: configurazione TOML con default e clamp, percorsi per la versione installata e quella portable, log locali a rotazione con percorsi anonimizzati, IPC su named pipe protetta da ACL, localizzazione Fluent (it/en), utility Win32.
- `hc-hotkeys`: parsing e normalizzazione delle hotkey, rilevamento dei duplicati, `RegisterHotKey` con segnalazione dei conflitti, tasti mouse tramite Raw Input.
- `hyperclips.exe`: tray, menu contestuale, notifiche, hotkey globali, server IPC, istanza singola, autostart HKCU, esclusione da WER, CLI (`--save-clip`, `--status`, `--quit`, …).
- `hyperclips-studio.exe`: finestra egui avviata su richiesta, impostazioni (lingua, tema chiaro/scuro/OLED, cattura, hotkey con validazione, aggiornamenti), pagina privacy, barra di stato con la RAM.
- `hc-bench`: misure di RAM e CPU e spike sul device D3D11.
- CI GitHub Actions: fmt, clippy `-D warnings`, test, build release, `cargo deny`, `cargo audit`.
