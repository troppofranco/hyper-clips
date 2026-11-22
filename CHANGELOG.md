# Changelog

Formato: [Keep a Changelog](https://keepachangelog.com/it/1.1.0/) · Versioni: [SemVer](https://semver.org/lang/it/).

## [0.3.3]

### Interfaccia
- Barra laterale come scheda arrotondata separata, come il resto dell'interfaccia; barra superiore che si adatta allo schermo intero (la ricerca occupa tutto lo spazio libero) e testo della ricerca centrato, con bordo colorato quando scrivi.
- **Editor riorganizzato** in cinque passi: Taglia, Inquadra, Testo, Audio, Esporta. Ogni passo mostra solo le sue opzioni; anteprima e timeline al centro; il pulsante Esporta e il riepilogo del risultato sono sempre visibili; le opzioni tecniche sono in "Opzioni avanzate".
- **Preset con la forma del risultato**: ogni preset mostra il formato (16:9, 9:16, 1:1), la risoluzione, il tipo di file e il limite di dimensione.
- Rimossa la sezione Privacy (il registro delle richieste resta in `logs\network.log`).

### Correzioni
- **Anteprime distorte**: la decodifica del fotogramma calcolava male la larghezza delle righe quando il video ha righe aggiuntive (1080p codificato a 1088) o larghezze dispari, e l'immagine usciva a strisce oblique. Ora usa il passo dichiarato dal decoder.
- Miniature più nitide (480×270) e mai stirate: ritagliate per riempire la scheda, le clip verticali mostrate intere. Vengono rigenerate una volta da sole.
## [0.3.2]

### Aggiornamento dall'app
- Il popup degli aggiornamenti ha il pulsante **Aggiorna ora**: scarica il pacchetto giusto (installer o zip portable), ne verifica lo SHA-256, chiude servizio e studio, installa e riapre lo studio sulla nuova versione, con barra di avanzamento. Se la nuova versione non parte, viene ripristinata la precedente e lo studio si riapre comunque.
- Lo stesso pulsante è in Impostazioni → Aggiornamenti; non si apre più GitHub.
- Verificato davvero: una copia portable "0.3.0" si è aggiornata da sola alla 0.3.1 pubblicata su GitHub.
## [0.3.1]

### Animazioni
- Animazioni fluide in tutto lo studio, con curve morbide (cubic-out) e durate come in Hyper Launcher:
  - pagine e categorie delle impostazioni entrano in dissolvenza salendo;
  - le schede della libreria compaiono a cascata, si sollevano con un'ombra al passaggio del mouse, il bordo di selezione e il badge play cambiano colore gradualmente;
  - le miniature appaiono in dissolvenza; mentre si caricano c'è un riflesso "skeleton";
  - voci di menu, pulsanti, pillole della barra superiore (con freccia che si ribalta), interruttori (con pallino che si allunga) e controlli a segmenti (con evidenziazione che scorre);
  - pulsante principale con alone al passaggio e leggera compressione al clic;
  - pannello dei dettagli che scorre da destra, barra "modifiche non salvate" che sale dal basso, notifiche e popup degli aggiornamenti in dissolvenza;
  - indicatore di registrazione pulsante.
- Tutto rispetta Personalizzazione → Animazioni (Complete, Ridotte = metà durata e niente cascate/riflessi, Spente) e l'opzione "riduci animazioni".
- A riposo lo studio non ridisegna nulla (0% CPU misurato); le animazioni continue esistono solo durante una registrazione o un caricamento.
- Correzione: con un file non riproducibile aperto nei dettagli, il player ridisegnava di continuo (circa 4% di un core).
## [0.3.0]

### Nuova interfaccia (stile Hyper Launcher)
- Ridisegnata da capo sulla bozza: barra laterale con logo e tre voci (Libreria, Editor, Impostazioni); barra superiore arrotondata con ricerca e menu filtri, tasto "Salva clip", durata del replay e risoluzione modificabili al volo.
- Schede della libreria con miniatura incassata e badge play, titolo, gioco e durata; sollevamento al passaggio del mouse.
- Colori, forme e personalizzazione di Hyper Launcher: palette scura e chiara, accento viola predefinito con 5 colori più uno personalizzato, sfondo in tinta unita o sfumato con opacità dei pannelli, angoli netti/normali/arrotondati, pulsanti Standard o Liquid glass, animazioni complete/ridotte/spente. Restano i temi OLED e alto contrasto.
- Vault e Privacy spostati nelle Impostazioni; momenti salienti come filtro della libreria.
## [0.2.1]

### Aggiornamenti
- All'apertura dello studio compare un popup se su GitHub c'è una versione più recente: numero di versione, novità principali, link alla pagina della release e pulsanti "Apri su GitHub", "Più tardi" e "Salta questa versione". Rispetta le impostazioni (controllo disattivato, canale, versioni saltate).
- Prima l'avviso arrivava solo come notifica di Windows e l'installazione andava avviata a mano da Impostazioni → Aggiornamenti.

## [0.2.0]

### Interfaccia rinnovata
- Nuovo layout dello studio: barra laterale con icone (Libreria, Momenti salienti, Editor, Vault, Impostazioni, Privacy), spazio su disco e stato del servizio; barra superiore con stato della cattura (replay attivo / registrazione / spento) e menu di azioni rapide, ricerca e "Condividi l'ultima".
- Libreria: filtri a pillola (Tutto, Preferiti, Clip, Registrazioni, Screenshot, Per gioco), raggruppamento per data (Oggi, Ieri, Questa settimana…), schede che riempiono la finestra con etichetta del gioco, durata, tipo e "2 h fa · 00:11 · 6.5 MB"; stelle per i preferiti; stato vuoto con istruzioni.
- Nuova pagina **Momenti salienti**.
- Impostazioni divise in 13 categorie, ogni opzione con una spiegazione, interruttori al posto delle caselle, barra "Modifiche non salvate" con Salva/Annulla.
- Nuova sezione **Personalizzazione**: tema con anteprime (scuro, chiaro, OLED, alto contrasto), colore principale (10 colori o personalizzato), dimensione delle schede, arrotondamento, layout compatto, etichette e dettagli delle schede, pagina iniziale, scala dell'interfaccia; anteprima dal vivo.

## [0.1.0]
### Modifiche
- Rimossa la firma minisign degli aggiornamenti (e lo strumento `hc-sign`): i pacchetti vengono verificati con lo SHA-256 pubblicato in `checksums.txt`. Non servono più secret su GitHub per pubblicare una release.

### Fase 7 — Privacy Audit e rifinitura
- **Privacy Audit** nello studio: connessioni TCP aperte in tempo reale da ogni processo di Hyper Clips (tabella di Windows via `GetExtendedTcpTable`), registro delle richieste in uscita, cartelle con dimensioni, spiegazione di cosa contengono i file, **cancellazione dei dati** selettiva (log, cache, indice, impostazioni, vault, clip) con conferma scritta.
- Verificato che clip, registrazioni ed export non contengono metadati (date azzerate nei box MP4, nessun nome di software).
- Documentazione completa in italiano e inglese: README, PRIVACY, ARCHITECTURE, BENCHMARKS, API.
- Limiti noti dichiarati: RAM del core con cattura attiva ~117 MB (obiettivo 60 MB non raggiunto), WebM senza audio, WebP animato lossless.

### Fase 6 — Funzioni extra
- **Momenti salienti locali**: analisi dell'audio della clip (sonorità a breve termine contro il fondo mobile), marker automatici con tag `highlight` dopo ogni salvataggio, pulsante "Trova momenti salienti" e `--highlights <id>`. Nessun modello, nessuna rete.
- **Condivisione in rete locale con QR** (`hc-share`): link con token casuale a 128 bit, solo LAN, scadenza e limite di download, pagina con player e supporto Range per il seek dal telefono. Spenta finché non viene avviata da una clip; traffico HTTP non cifrato (dichiarato nell'interfaccia).
- **Vault cifrato** (`hc-vault`): XChaCha20-Poly1305 a blocchi con rilevamento di troncamenti e riordini, chiave principale protetta da Argon2id, indice cifrato (i nomi dei file non sono leggibili), cambio password senza ricifrare i file. "Sposta nel vault" dalla libreria, apertura ed estrazione.
- **Profili per gioco**: fps, risoluzione, bitrate, codec, durata del replay, modalità di cattura, microfono e tracce separate per singolo gioco; replay disattivabile per gioco.
- **Modalità a basso impatto** e **politica a batteria** (basso impatto, sospensione del replay o nessuna modifica); la cattura si riavvia solo fuori dalle registrazioni.
- **API HTTP locale** (spenta di default): solo 127.0.0.1, token bearer, rifiuto delle richieste dai browser (`Origin`/`Host`). Vedi `docs/API.md`.
- **Plugin WebAssembly** (`hc-plugins`, spenti di default): interprete wasmi in sandbox senza I/O, limiti di CPU e memoria, permessi `tags` e `notify`.
- **Backup e ripristino delle impostazioni** (studio e CLI).
- **Accessibilità**: tema ad alto contrasto, scala dell'interfaccia, riduzione delle animazioni.
- CLI: `--export <id> --preset <nome>` ora accoda davvero l'export; l'output della CLI si può reindirizzare su file.
- Correzione: al cambio lingua il core ricaricava ancora la lingua precedente.

### Fase 5 — Aggiornamenti, installer, rilasci
- `hc-updater`: controllo delle release su GitHub (`troppofranco/hyper-clips`, costanti in un solo punto), canali stabile e beta (SemVer), all'avvio, ogni N ore o mai; versioni ignorabili. User-Agent generico, nessun identificativo; ogni richiesta finisce in `logs\network.log`.
- Verifica prima dell'installazione: firma **minisign** di `checksums.txt` con chiave pubblica incorporata (il *trusted comment* deve contenere la versione, per bloccare i downgrade), poi **SHA-256** del pacchetto.
- `hc-updater-helper`: aspetta la chiusura dell'app, fa il backup dei file di programma, installa (Setup silenzioso o estrazione dello zip portable senza toccare `data\`), avvia la nuova versione con un controllo di salute e, se fallisce entro 30 s, **ripristina la versione precedente**. Rifiuta eseguibili non validi senza mostrare dialoghi di sistema.
- Notifica dell'aggiornamento dal core; pannello "Aggiornamenti" nello studio (note di rilascio, installa, ignora).
- Installer **Inno Setup** per utente (niente amministratore), italiano/inglese, modalità silenziosa, collegamenti opzionali, avvio automatico opzionale; il programma di disinstallazione chiede se eliminare i dati (predefinito: no). Zip **portable** con `portable.flag`.
- `hc-sign` (keygen/firma) e workflow GitHub Actions `release.yml`: test, build, zip, installer, checksum firmati, note dal CHANGELOG, pre-release per i tag con trattino. Guida in `docs/RELEASING.md`.

### Fase 4 — Editor ed export
- `hc-editor`: progetto non distruttivo (JSON) con segmenti (inizio, fine, velocità 0,25–4x), taglio al cursore, riordino, transizioni (stacco, dissolvenza incrociata, dissolvenza al nero), inquadratura (originale, 16:9, 9:16, 1:1, zoom, spostamento), overlay di testo con contorno (GDI, nessun font remoto) e di immagini, sottotitoli .srt importabili, tracce audio con volume e muto, esportazione mixata o separata.
- Motore di export su GPU: decodifica Media Foundation (DXVA), composizione D3D11, stessi encoder hardware della cattura. Formati: **MP4** (H.264/HEVC/AV1 + AAC), **WebM** (AV1, muxer Matroska proprio; senza audio perché manca un encoder Opus locale), **GIF**, **WebP animato** (lossless).
- Preset Discord (10 MB), YouTube, Shorts/TikTok (9:16), Twitter/X, qualità massima, GIF, WebP.
- **Ottimizzatore di dimensione**: nuova codifica con bitrate corretto finché il file rientra nel limite (fino a 3 passate; 10/25/50/500 MB a un clic).
- **Coda di esportazione in background** in un processo worker separato a priorità ridotta, che continua anche a finestra chiusa; il risultato viene aggiunto alla libreria.
- Studio: pagina Editor con anteprima (riquadro di crop e posizioni degli overlay), timeline dei segmenti e coda.
### Fase 3 — Libreria e player
- `hc-library`: indice SQLite locale (WAL) con tag, preferiti, note, marker, rinomina del file, eliminazione nel Cestino, limite di spazio con auto-eliminazione delle clip non preferite più vecchie, cartelle aggiuntive, scansione e riconciliazione, raggruppamento per gioco o per sessione.
- Miniature JPEG generate in locale (Media Foundation per i video, WIC per le immagini) nella cache dell'app.
- Il core indicizza in background ogni clip, registrazione e screenshot salvato; `hyperclips.exe --list` legge la libreria anche a core spento.
- Studio: galleria con ricerca, filtri (gioco, tag, tipo, preferiti), ordinamenti, raggruppamenti; pannello di dettaglio con player (MF Media Engine: decodifica hardware, audio, seek), marker sulla timeline, modifica di titolo, tag, note e preferito; "Mostra in Esplora file", "Copia negli appunti" (incolla su Discord), "Apri", "Elimina".
- Correzione: il server IPC tiene sempre pronta un'istanza della pipe (prima, due comandi CLI ravvicinati potevano fallire).
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
