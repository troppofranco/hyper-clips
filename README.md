# Hyper Clips

**🇮🇹 Italiano · [🇬🇧 English](#english)**

Registra, modifica e gestisci le clip dei tuoi giochi su Windows con un consumo
di risorse contenuto e **privacy totale**: nessun account, nessuna telemetria,
nessun tracker, nessun cloud. Scritto in Rust, usa solo le API multimediali di
Windows (DXGI, Direct3D 11, Media Foundation, WASAPI).

> **Stato: 0.3.3.** Tutte le fasi del piano sono implementate.
> Alcune parti non sono ancora verificate su tutto l'hardware (vedi
> [Limiti noti](#limiti-noti)).

## Funzioni

**Cattura**
- Instant Replay da 15 s a 10 min, in RAM o su disco (file temporaneo autocancellante).
- Registrazione completa con pre-roll, marker durante la partita, screenshot PNG/JPG/WebP.
- Encoder hardware NVIDIA/AMD/Intel (H.264, HEVC, AV1) con fallback software.
- Monitor intero, finestra del gioco (Windows Graphics Capture) o regione; cursore; HDR con tone-mapping.
- Audio di sistema, microfono e **tracce separate** per gioco, chat vocale e altro; push-to-talk, noise gate, soppressione del rumore locale (RNNoise).
- Riconoscimento del gioco da un database locale modificabile; **profili per gioco**.
- Overlay di notifica che non finisce mai nelle clip; webcam in sovrimpressione (spenta di default).
- **Modalità a basso impatto** e politica a batteria per i portatili.

**Libreria ed editor**
- Galleria con ricerca, tag, preferiti, note, marker, raggruppamento per gioco o sessione, limite di spazio.
- Player integrato con decodifica hardware.
- Editor non distruttivo: tagli, velocità, transizioni, crop 9:16/1:1, testo, immagini, sottotitoli .srt, mix delle tracce.
- Export su GPU in MP4, WebM (AV1), GIF, WebP; preset Discord, YouTube, Shorts/TikTok, X; **ottimizzatore di dimensione** per Discord (10/25/50/500 MB); coda in background.
- **Momenti salienti** rilevati in locale dall'audio, come marker.

**Extra**
- **Condivisione in rete locale con QR code**: link temporaneo con token, scadenza e limite di download.
- **Vault cifrato** (XChaCha20-Poly1305 + Argon2id) per le clip private.
- **CLI** e **API HTTP locale** (solo 127.0.0.1, con token) per Stream Deck e automazioni: [docs/API.md](docs/API.md).
- **Plugin WebAssembly** in sandbox (interprete wasmi, nessun accesso a file o rete).
- Backup e ripristino delle impostazioni; modalità **portable**.
- Italiano e inglese; temi chiaro, scuro, OLED e **alto contrasto**; scala dell'interfaccia.
- **Aggiornamenti automatici** da GitHub Releases verificati con SHA-256, con rollback.

## Installazione

Scarica da [Releases](https://github.com/troppofranco/hyper-clips/releases):
- `HyperClips-Setup-<versione>.exe`: installazione per utente, senza permessi di amministratore;
- `HyperClips-<versione>-portable.zip`: estrai ovunque, tutti i dati restano nella cartella.

Senza certificato di firma del codice, Windows SmartScreen può mostrare un avviso ai primi download
("Ulteriori informazioni" → "Esegui comunque").

## Compilare

Requisiti: Windows 10 22H2 o Windows 11, [Rust](https://rustup.rs) stable (MSVC).

```powershell
cargo build --release
```

Gli eseguibili si trovano in `target\release`. Per una prova senza toccare `%APPDATA%`
né il registro, crea un file vuoto `portable.flag` accanto a `hyperclips.exe`.

## Uso

- Avvia `hyperclips.exe`: compare l'icona nella tray e si apre lo studio.
- Hotkey predefinite: `F8` salva clip · `Ctrl+F9` registrazione · `Alt+F8` screenshot · `Shift+F8` marker · `Ctrl+Alt+H` apri lo studio.
- CLI: `hyperclips.exe --help`.

## Limiti noti

- **RAM del core con la cattura attiva**: circa 115–120 MB esclusi il buffer, sopra l'obiettivo di 60 MB.
  La maggior parte è memoria dei driver grafici e dell'encoder hardware; solo tray: ~16 MB.
- I file **WebM** sono senza audio: Windows non include un encoder Opus/Vorbis e non usiamo librerie esterne.
- Il **WebP animato** è lossless e quindi pesante; per Discord conviene MP4 o GIF.
- HDR, webcam, AV1 su AMD/Intel e l'overlay sopra i giochi a schermo intero esclusivo sono implementati ma
  verificati solo su una configurazione (NVIDIA RTX, SDR).
- Il rilevamento dei momenti salienti si basa solo sul volume: trova esplosioni e urla, non le azioni silenziose.
- La condivisione LAN usa HTTP non cifrato all'interno della rete locale.

## Documenti

[Architettura](docs/ARCHITECTURE.md) · [Benchmark](docs/BENCHMARKS.md) · [API e plugin](docs/API.md) ·
[Rilasci](docs/RELEASING.md) · [Privacy](PRIVACY.md) · [Contribuire](CONTRIBUTING.md) · [Changelog](CHANGELOG.md)

Licenza: [GPL-3.0-or-later](LICENSE).

---

<a id="english"></a>
## English

Record, edit and manage game clips on Windows with a small footprint and
**complete privacy**: no account, no telemetry, no trackers, no cloud. Written in
Rust on top of Windows' own media APIs.

> **Status: 0.3.3.** Every planned phase is implemented; some parts
> are not yet verified on all hardware (see *Known limits*).

**Features**
- **Capture:** Instant Replay (15 s–10 min, RAM or disk), recording with pre-roll, markers,
  PNG/JPG/WebP screenshots; NVIDIA/AMD/Intel hardware H.264/HEVC/AV1; monitor, game window or region;
  cursor; HDR tone-mapping; separate audio tracks for game, voice chat and other; push-to-talk,
  noise gate, local noise suppression; per-game profiles; low-impact and battery modes; in-game
  notification overlay that never appears in clips; optional webcam.
- **Library & editor:** searchable gallery with tags, favourites, notes, markers, grouping and a storage
  quota; hardware player; non-destructive editor (cuts, speed, transitions, 9:16/1:1 framing, text,
  images, .srt subtitles, track mix); GPU export to MP4, WebM (AV1), GIF, WebP; Discord/YouTube/Shorts/X
  presets; Discord size optimizer; background export queue; local audio-based highlight detection.
- **Extras:** temporary LAN sharing with a QR code; encrypted vault (XChaCha20-Poly1305 + Argon2id);
  CLI and token-protected loopback HTTP API ([docs/API.md](docs/API.md)); sandboxed WebAssembly plugins;
  settings backup/restore; portable mode; Italian/English; light, dark, OLED and high-contrast themes
  with UI scaling;
  SHA-256-verified auto-updates with rollback.

**Install:** grab the per-user installer or the portable zip from
[Releases](https://github.com/troppofranco/hyper-clips/releases).
**Build:** `cargo build --release` (Windows 10 22H2+/11, Rust stable MSVC).

**Known limits:** core RAM with capture running is ~115–120 MB excluding the buffer (above the
60 MB goal; mostly GPU driver and hardware encoder memory; tray only ~16 MB); WebM exports have no
audio (no Opus/Vorbis encoder ships with Windows); animated WebP is lossless and large; HDR, webcam,
AMD/Intel AV1 and the overlay over exclusive-fullscreen games are verified on a single NVIDIA/SDR
setup; highlight detection only looks at loudness; LAN sharing is plain HTTP on the local network.

**Privacy:** the only automatic network request is the optional update check against GitHub.
See [PRIVACY.md](PRIVACY.md).

License: [GPL-3.0-or-later](LICENSE).
