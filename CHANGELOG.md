# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/it/1.1.0/) · Versioni: [SemVer](https://semver.org/lang/it/).

## [Unreleased]

### Fase 2 — Completamento
- Cattura **finestra del gioco** (Windows Graphics Capture) e **regione** (ritaglio con letterbox); la cattura segue il gioco su finestra e monitor.
- **Cursore** composto nel video (Desktop Duplication non lo include); **HDR**: duplicazione in FP16 e tone-mapping verso SDR con shader D3D11.
- **Audio per processo**: tracce separate Mix, Game, Chat (Discord/TeamSpeak/…) e Other (tutto tranne il gioco), nomi delle tracce nel file MP4. **Push-to-talk**, **noise gate**, **soppressione del rumore** (RNNoise, locale).
- **Buffer di replay su disco** (file ad anello temporaneo, cancellato in automatico; Auto oltre 2 minuti).
- **AV1** hardware (NVIDIA/AMD/Intel via Media Foundation) con muxing `av01`/`av1C`.
- **Overlay in-game** per le notifiche, mai registrato; **webcam** in sovrimpressione (spenta di default); screenshot **WebP** lossless.
- Studio: impostazioni per tutte le opzioni sopra.
- Correzioni: fallback a `DuplicateOutput` per i processi non DPI-aware V2; crash dell'overlay al primo `WM_PAINT` con testo vuoto.
- Diagnostica: `hc-bench levels`, `audio-sessions`; `HC_NATIVE_CRASH_LOG=1` scrive in `crash.log` il backtrace degli access violation (solo in locale).

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
