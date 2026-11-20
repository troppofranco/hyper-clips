# Architettura di Hyper Clips

Questo documento riassume il piano iniziale (Fase 0) e le decisioni prese durante
lo sviluppo (Fasi 1–7). Le misure stanno in [BENCHMARKS.md](BENCHMARKS.md).

## Processi

```
hyperclips.exe (core, sempre attivo)
  tray Win32 · hotkey (RegisterHotKey + Raw Input) · server IPC (named pipe)
  cattura Desktop Duplication / WGC → compositor D3D11 → NV12 → encoder HW (MF)
  audio WASAPI + process loopback → AAC · ring buffer replay · muxer MP4
  indicizzatore (libreria, miniature, momenti salienti, plugin WASM)
  controllo aggiornamenti · API HTTP locale opzionale (127.0.0.1)
  Nessun toolkit UI e nessuna libreria multimediale esterna.

hyperclips-studio.exe (avviato su richiesta, termina alla chiusura)
  egui (glow) · libreria · player MF · editor · vault · condivisione LAN
  impostazioni · privacy audit · ruolo --export-worker per la coda di export

hc-updater-helper.exe (copiato in %TEMP% durante un aggiornamento)
  backup, Setup silenzioso o estrazione dello zip, health check, rollback
```

Comunicazione: named pipe `\\.\pipe\HyperClips.<sessione>`, DACL che ammette
solo l'utente corrente, `PIPE_REJECT_REMOTE_CLIENTS`, prima istanza esclusiva
(anti-squatting), client con `SECURITY_IDENTIFICATION` (niente impersonation).
Framing: `u32` LE + `postcard`. Configurazione condivisa via `config.toml`
(il core la ricarica su `Request::ReloadConfig`). La configurazione "effettiva"
usata dalla cattura è quella globale + profilo del gioco in primo piano + politica
a batteria (`Config::effective`).

## Workspace

| Crate | Ruolo |
|---|---|
| `crates/hc-core` | config, profili, percorsi, logging, IPC, i18n, backup impostazioni, audit di rete, utility Win32 |
| `crates/hc-hotkeys` | parsing e registrazione hotkey |
| `crates/hc-capture` | Desktop Duplication (anche HDR), WGC, regione, compositor D3D11 (cursore, tone-map, webcam), WASAPI + process loopback, DSP microfono, screenshot |
| `crates/hc-encode` | encoder Media Foundation (HW/SW, H.264/HEVC/AV1), AAC, muxer MP4, probe |
| `crates/hc-buffer` | ring buffer di pacchetti codificati (RAM o disco) |
| `crates/hc-recorder` | thread video/audio/sink, clip, registrazione, marker |
| `crates/hc-detect` | gioco in primo piano (DB locale + euristica fullscreen) |
| `crates/hc-library` | indice SQLite, tag, marker, quota, scansione, miniature |
| `crates/hc-editor` | progetto, decodifica, rendering ed export su GPU, WebM, GIF/WebP, coda, momenti salienti |
| `crates/hc-updater` | controllo release, verifica minisign + SHA-256, download |
| `crates/hc-share` | condivisione LAN temporanea (tiny_http) e QR code |
| `crates/hc-vault` | vault cifrato (XChaCha20-Poly1305 a blocchi, Argon2id) |
| `crates/hc-plugins` | host di plugin WebAssembly in sandbox (wasmi) |
| `apps/hyperclips` | core, tray, CLI, API locale |
| `apps/hyperclips-studio` | UI egui |
| `apps/hc-updater-helper` | applicazione degli aggiornamenti con rollback |
| `tools/hc-bench`, `tools/hc-sign` | misure e diagnostica; chiavi e firma dei rilasci |

Scelte rispetto al piano iniziale: nessun FFmpeg (decodifica ed export usano
Media Foundation e Direct3D 11, quindi niente DLL di terze parti e nessun problema
di licenze dei codec); il player usa MF Media Engine; i plugin usano un interprete
WebAssembly invece di un JIT, per un sandbox più semplice da verificare.
## Percorsi

| | Installata | Portable (`portable.flag` accanto all'exe) |
|---|---|---|
| Config | `%APPDATA%\HyperClips` | `.\data\config` |
| Libreria, cache, log, tmp | `%LOCALAPPDATA%\HyperClips\…` | `.\data\…` |
| Clip | `Video\Hyper Clips` | `.\data\clips` |
| Registro (autostart, esclusione WER) | sì, solo HKCU | mai |

## Decisioni (con motivazione)

| Area | Scelta | Note |
|---|---|---|
| Licenza | GPL-3.0-or-later | compatibile con x264/FFmpeg GPL; i fork restano verificabili |
| UI | egui 0.37 | renderer **glow** (misurato, vedi BENCHMARKS); painter D3D11 custom in Fase 3 |
| Tray | Win32 puro | nessun crate extra |
| Hotkey | `RegisterHotKey` + Raw Input (`RIDEV_INPUTSINK`) per i tasti mouse | nessun hook, sicuro con gli anti-cheat; i conflitti vengono rilevati |
| Encoder (core) | Media Foundation, poi NVENC/AMF nativi | nessun codec di terze parti nel core |
| Decode/editor | FFmpeg in DLL, solo nello studio | decode HEVC/AV1 senza estensioni dello Store |
| Rete | `ureq` + rustls, solo nell'updater; niente tokio | |
| Log | `tracing`, file locali a rotazione (7 giorni), percorsi anonimizzati (`~`) | `crash.log` sincrono (release = `panic=abort`) |
| Crash | esclusione da Windows Error Reporting (per utente, solo installata) | nessun dump inviato a Microsoft |
| i18n | Fluent, it/en incorporati, override in `locales/<lingua>/main.ftl` | |

## Pipeline di cattura (Fase 2)

```
video : Desktop Duplication (monitor del gioco) → copia BGRA → D3D11 Video Processor
        (scala + BGRA→NV12 BT.709, hardware fisso) → MFT encoder HW (zero copie)
        → Annex-B → NAL con prefisso di lunghezza → sink
audio : WASAPI loopback (sistema) + capture (mic) a 48 kHz → mixer allineato
        al clock QPC (riempie i silenzi, assorbe jitter) → AAC ×N (mix + stem) → sink
sink  : ring buffer (GOP interi, keyframe ogni 1 s) + registrazione MP4 dal vivo;
        salvataggio clip in un thread separato (snapshot = copia di puntatori)
```

Tracce audio nelle clip: con sistema + microfono → traccia 1 = mix (attiva di default,
è quella che riproducono Discord e i player), traccia 2 = sistema, traccia 3 = microfono
(disattivate ma presenti per l'editor). Con una sola sorgente → una traccia.

File: `<cartella clip>\<Gioco>\<Gioco> AAAA-MM-GG HH-MM-SS.mp4`, registrazioni con
suffisso `REC`, screenshot in `Screenshots\`, marker in `<file>.markers.json`
(importati nella libreria in Fase 3). Nessun metadato nel file (né titolo, né software,
né date).

## Limiti noti (aggiornati)

- **Fase 2 completata**, con queste verifiche ancora mancanti su hardware reale: HDR (shader di tone-mapping testato in unit test, ma non su un monitor HDR), webcam (nessuna camera testata), overlay in-game (non verificato visivamente sopra un gioco), push-to-talk, noise gate e RNNoise (testati in unit test, non con un microfono reale). Backend NVENC/AMF nativi rimandati alla Fase 7 come ottimizzazione di RAM: Media Foundation copre già NVIDIA, AMD e Intel.
- **RAM del core in cattura:** 83–117 MB di working set al netto del buffer, sopra l'obiettivo dei 60 MB; le tracce audio separate pesano circa 29 MB (dettagli in BENCHMARKS).
- **Overlay in-game:** escluso da ogni cattura (`WDA_EXCLUDEFROMCAPTURE`); nei giochi in fullscreen esclusivo "vero" non è visibile e si ricade sul toast di Windows.
- **Audio per processo:** richiede Windows 10 2004+; altrimenti una sola traccia di sistema. Se un gioco riproduce l'audio da un processo esterno al suo albero, quel suono finisce in "Other".
- **Cattura finestra (WGC) su Windows 10:** bordo giallo non rimovibile (su Windows 11 viene nascosto).
- **HDR:** con un desktop HDR la cattura avviene in FP16 e viene convertita in SDR (bianco di riferimento configurabile). Non c'è ancora uscita HDR10.
- **Win10 22H2:** WGC mostra un bordo giallo non rimovibile → Desktop Duplication per monitor e gioco fullscreen.
- **Fullscreen esclusivo "vero":** possibili frame neri; niente hook (rischio anti-cheat) → suggerire il borderless.
- **Hotkey solo tasto** (es. `F8`): `RegisterHotKey` le consuma globalmente, come Medal e ShadowPlay.
- **Tasti mouse come hotkey:** Raw Input riceve anche i movimenti del mouse (costo CPU minimo, attivo solo se una hotkey usa il mouse).
- **CLI:** `hyperclips.exe` è un'app GUI che si aggancia alla console del padre. In `cmd` il prompt può tornare prima dell'output; in PowerShell usa `hyperclips.exe --status | Out-String`.
