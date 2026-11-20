# Architettura di Hyper Clips

Questo documento riassume la Fase 0 (piano approvato) e le decisioni prese
durante lo sviluppo. Le misure stanno in [BENCHMARKS.md](BENCHMARKS.md).

## Processi

```
hyperclips.exe (core, sempre attivo)
  tray Win32 · hotkey (RegisterHotKey + Raw Input) · server IPC (named pipe)
  [Fase 2] cattura WGC/Desktop Duplication → NV12 su GPU → encoder HW
           audio WASAPI multitraccia → AAC · ring buffer replay · fMP4
  Nessun toolkit UI, nessun FFmpeg, nessuno stack di rete.

hyperclips-studio.exe (avviato su richiesta, termina alla chiusura)
  egui · libreria · player · editor · impostazioni · privacy audit
  [Fase 3+] FFmpeg (DLL) caricato solo qui; ruolo --worker per gli export

hc-updater-helper.exe [Fase 5]
  sostituzione file / Setup silenzioso, health check, rollback
```

Comunicazione: named pipe `\\.\pipe\HyperClips.<sessione>`, DACL che ammette
solo l'utente corrente, `PIPE_REJECT_REMOTE_CLIENTS`, prima istanza esclusiva
(anti-squatting), client con `SECURITY_IDENTIFICATION` (niente impersonation).
Framing: `u32` LE + `postcard`. Configurazione condivisa via `config.toml`
(il core la ricarica su `Request::ReloadConfig`).

## Workspace

| Crate | Ruolo | Stato |
|---|---|---|
| `crates/hc-core` | config, percorsi, logging, IPC, i18n, utility Win32 | Fase 1 ✅ |
| `crates/hc-hotkeys` | parsing e registrazione hotkey | Fase 1 ✅ |
| `apps/hyperclips` | core + tray + CLI | Fase 1 ✅ |
| `apps/hyperclips-studio` | UI egui | Fase 1 (scheletro) ✅ |
| `tools/hc-bench` | misure RAM/CPU, spike | Fase 1 ✅ |
| `hc-capture`, `hc-encode`, `hc-buffer` | cattura, encoder, replay | Fase 2 |
| `hc-library`, `hc-media` | SQLite, FFmpeg, player | Fase 3 |
| `hc-editor` | editor, export | Fase 4 |
| `hc-updater`, `apps/hc-updater-helper` | auto-update | Fase 5 |
| `hc-detect`, `hc-privacy`, `hc-share`, `hc-plugins` | extra | Fase 6 |

I crate vengono creati nella fase in cui servono, per non avere codice vuoto.

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

## Limiti noti (aggiornati)

- **Win10 22H2:** WGC mostra un bordo giallo non rimovibile → Desktop Duplication per monitor e gioco fullscreen.
- **Fullscreen esclusivo "vero":** possibili frame neri; niente hook (rischio anti-cheat) → suggerire il borderless.
- **Hotkey solo tasto** (es. `F8`): `RegisterHotKey` le consuma globalmente, come Medal e ShadowPlay.
- **Tasti mouse come hotkey:** Raw Input riceve anche i movimenti del mouse (costo CPU minimo, attivo solo se una hotkey usa il mouse).
- **CLI:** `hyperclips.exe` è un'app GUI che si aggancia alla console del padre. In `cmd` il prompt può tornare prima dell'output; in PowerShell usa `hyperclips.exe --status | Out-String`.
