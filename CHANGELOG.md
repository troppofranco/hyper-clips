# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/it/1.1.0/) · Versioni: [SemVer](https://semver.org/lang/it/).

## [Unreleased]

### Fase 2 — Cattura e replay
- `hc-capture`: Desktop Duplication per monitor (nessun bordo, nessuna iniezione), conversione BGRA→NV12 e scaling sul video processor D3D11, screenshot PNG/JPG via WIC, cattura WASAPI (loopback di sistema e microfono) con mixer allineato al clock.
- `hc-encode`: encoder Media Foundation (hardware NVIDIA/AMD/Intel sull'adattatore del monitor, fallback software), CBR/VBR/CQP, GOP di 1 s, niente B-frame, keyframe su richiesta. Encoder AAC di Windows. Muxer MP4 proprio senza metadati. Probe e decodifica di un frame per i test.
- `hc-buffer`: ring buffer Instant Replay di pacchetti codificati, eviction per GOP, limite di byte, snapshot senza copie.
- `hc-recorder`: servizio di cattura (thread video, audio e sink), clip salvate in background, registrazione con pre-roll dall'ultimo keyframe, marker in JSON.
- `hc-detect`: gioco in primo piano da un database locale modificabile (`games.toml`) più un'euristica per le app a schermo intero. La cattura segue il gioco sul suo monitor.
- Core: hotkey, tray e CLI ora salvano clip, registrano, mettono marker e fanno screenshot; notifiche localizzate; `--status` mostra la dimensione del buffer.
- Studio: impostazioni per codec, controllo bitrate, monitor, formato screenshot, audio di sistema e microfono con volumi.
- `hc-bench`: `encoders`, `video-test`, `service-test`, `probe`, `frame`, `dd-diag`, `spike-pipeline`.

### Fase 1 — Core
- Workspace Cargo (edizione 2024), profilo release ottimizzato (LTO fat, `panic=abort`, strip).
- `hc-core`: configurazione TOML con default e clamp, percorsi per la versione installata e quella portable, log locali a rotazione con percorsi anonimizzati, IPC su named pipe protetta da ACL, localizzazione Fluent (it/en), utility Win32.
- `hc-hotkeys`: parsing e normalizzazione delle hotkey, rilevamento dei duplicati, `RegisterHotKey` con segnalazione dei conflitti, tasti mouse tramite Raw Input.
- `hyperclips.exe`: tray, menu contestuale, notifiche, hotkey globali, server IPC, istanza singola, autostart HKCU, esclusione da WER, CLI (`--save-clip`, `--status`, `--quit`, …).
- `hyperclips-studio.exe`: finestra egui avviata su richiesta, impostazioni (lingua, tema chiaro/scuro/OLED, cattura, hotkey con validazione, aggiornamenti), pagina privacy, barra di stato con la RAM.
- `hc-bench`: misure di RAM e CPU e spike sul device D3D11.
- CI GitHub Actions: fmt, clippy `-D warnings`, test, build release, `cargo deny`, `cargo audit`.
