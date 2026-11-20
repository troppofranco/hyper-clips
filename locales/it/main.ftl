## Hyper Clips — Italiano
app-name = Hyper Clips

## Tray
tray-tooltip = Hyper Clips — replay { $replay }
tray-open = Apri Hyper Clips
tray-save-clip = Salva clip
tray-toggle-recording = Avvia/ferma registrazione
tray-screenshot = Cattura screenshot
tray-quit = Esci
state-on = attivo
state-off = disattivo

## Azioni (usate anche per le hotkey)
action-save-clip = Salva clip
action-toggle-recording = Avvia/ferma registrazione
action-screenshot = Screenshot
action-add-marker = Aggiungi marker
action-open-studio = Apri Hyper Clips

## Notifiche
notify-unavailable = { $action }: non disponibile ({ $reason }).
notify-clip-saved = Clip salvata: { $seconds } s — { $game }
notify-recording-started = Registrazione avviata
notify-recording-stopped = Registrazione salvata ({ $duration }, { $markers } marker)
notify-marker = Marker aggiunto
notify-screenshot = Screenshot salvato
notify-error = Errore: { $error }
notify-update = È disponibile Hyper Clips { $version }. Apri l'app per installarlo.
notify-hotkey-conflicts = Alcune hotkey non sono state registrate. Apri le impostazioni per correggerle.
notify-already-running = Hyper Clips è già in esecuzione.

## Navigazione studio
nav-library = Libreria
nav-editor = Editor
nav-settings = Impostazioni
nav-privacy = Privacy
placeholder-library = La libreria delle clip arriva nella Fase 3.
placeholder-editor = L'editor arriva nella Fase 4.

## Impostazioni
settings-general = Generale
settings-language = Lingua
settings-language-auto = Automatica (Windows)
settings-theme = Tema
theme-light = Chiaro
theme-dark = Scuro
theme-oled = Nero OLED
settings-autostart = Avvia con Windows
settings-autostart-portable = Non disponibile in modalità portable.
settings-capture = Cattura
settings-replay-enabled = Instant Replay attivo
settings-replay-seconds = Durata replay (secondi)
settings-fps = Frame rate
settings-bitrate = Bitrate (kbps)
settings-hotkeys = Hotkey
settings-hotkeys-hint = Esempi: F8, Ctrl+Shift+S, Alt+Mouse5. Lascia vuoto per disattivare.
settings-updates = Aggiornamenti
settings-update-check = Controlla aggiornamenti
update-on-startup = All'avvio
update-every-hours = Ogni N ore
update-never = Mai
settings-update-channel = Canale
channel-stable = Stabile
channel-beta = Beta
settings-save = Salva
settings-revert = Annulla modifiche
settings-saved = Impostazioni salvate.
settings-save-failed = Impossibile salvare: { $error }
hotkey-invalid = Non valida: { $error }
hotkey-duplicate = Già usata da "{ $other }"
hotkey-conflict = Non registrata: { $reason }

## Privacy
privacy-title = Audit privacy
privacy-network = Rete: Hyper Clips contatta solo GitHub per controllare gli aggiornamenti, e solo se attivato.
privacy-folders = Cartelle usate
privacy-mode = Modalità: { $mode }
mode-installed = installata
mode-portable = portable

## Barra di stato
status-core-connected = Core v{ $version } · { $core_mem } · Studio { $ui_mem }
status-core-missing = Servizio core non in esecuzione

## Impostazioni — cattura e audio
settings-codec = Codec video
settings-rate-control = Controllo bitrate
settings-monitor = Monitor
monitor-game = Monitor del gioco
monitor-primary = Monitor principale
settings-screenshot-format = Formato screenshot
settings-audio = Audio
settings-audio-system = Registra l'audio di sistema (gioco, musica, chat)
settings-audio-mic = Registra il microfono
settings-volume = Volume (%)
settings-audio-hint = Con entrambe le sorgenti attive le clip contengono una traccia mixata più tracce separate per l'editing.

## Impostazioni — completamento fase 2
settings-capture-mode = Cattura
mode-monitor = Monitor intero
mode-window = Solo la finestra del gioco
mode-region = Regione
settings-region = Regione (x, y, larghezza, altezza)
settings-show-cursor = Mostra il cursore del mouse
settings-sdr-white = Bianco di riferimento HDR → SDR (nit)
settings-buffer-storage = Buffer del replay
storage-auto = Automatico (disco oltre 2 min)
storage-memory = RAM
storage-disk = Disco
settings-webcam = Webcam
settings-webcam-enabled = Mostra la webcam nelle registrazioni
settings-webcam-device = Videocamera
settings-webcam-size = Dimensione (% dell'altezza)
settings-position = Posizione
pos-top-left = In alto a sinistra
pos-top-right = In alto a destra
pos-bottom-left = In basso a sinistra
pos-bottom-right = In basso a destra
settings-separate-tracks = Tracce separate per gioco, chat vocale e altri suoni
settings-ptt = Tasto push-to-talk (vuoto = microfono sempre attivo)
settings-noise-gate = Noise gate
settings-noise-gate-db = Soglia (dBFS)
settings-noise-suppression = Soppressione del rumore (RNNoise, locale)
settings-notifications = Notifiche
settings-notify-enabled = Mostra le notifiche
settings-notify-in-game = Mostra sopra il gioco (mai registrate)
settings-notify-sound = Riproduci un suono

## Libreria
lib-search = Cerca titolo, gioco, note, tag…
lib-all-games = Tutti i giochi
lib-all-tags = Tutti i tag
lib-all-kinds = Tutto
kind-clip = Clip
kind-recording = Registrazioni
kind-screenshot = Screenshot
lib-favorites = Solo preferiti
lib-sort = Ordina
sort-newest = Più recenti
sort-oldest = Meno recenti
sort-longest = Più lunghe
sort-shortest = Più corte
sort-largest = Più pesanti
sort-title = Titolo
lib-group = Raggruppa
group-none = Nessuno
group-game = Per gioco
group-session = Per sessione
lib-empty = Ancora nessuna clip. Premi { $key } in gioco per salvarne una.
lib-summary = { $count } elementi · { $size }
lib-scanning = Scansione delle cartelle…
lib-title = Titolo
lib-rename = Rinomina
lib-notes = Note
lib-tags = Tag (separati da virgola)
lib-markers = Marker
lib-open-folder = Mostra in Esplora file
lib-copy = Copia negli appunti
lib-copied = Copiato: incollalo su Discord o in qualsiasi app.
lib-open = Apri
lib-delete = Elimina
lib-delete-confirm = Spostare nel Cestino?
lib-yes = Sì
lib-no = No
lib-close = Chiudi
lib-player-error = Impossibile riprodurre il file (codec mancante?).
lib-favorite = Preferito
settings-library = Libreria e spazio
settings-quota = Limite di spazio (GB, 0 = illimitato)
settings-auto-delete = Sposta nel Cestino le clip non preferite più vecchie quando si supera il limite
settings-extra-folders = Cartelle aggiuntive (una per riga)

## Editor
lib-edit = Modifica
ed-empty = Apri una clip dalla Libreria e premi "Modifica" per iniziare un progetto. Aggiungi altre clip per creare un montaggio.
ed-timeline = Timeline
ed-duration = Durata finale: { $d }
ed-in = Inizio
ed-out = Fine
ed-set-playhead = Usa il cursore
ed-move-left = Sposta prima
ed-move-right = Sposta dopo
ed-speed = Velocità
ed-split = Taglia al cursore
ed-remove = Rimuovi
ed-presets = Preset
preset-discord = Discord (10 MB)
preset-youtube = YouTube
preset-shorts = Shorts / TikTok
preset-twitter = Twitter / X
preset-max = Qualità massima
preset-gif = GIF
preset-webp = WebP
ed-framing = Inquadratura
ed-aspect-source = Originale
ed-zoom = Zoom
ed-pan-x = Spostamento X
ed-pan-y = Spostamento Y
ed-transition = Transizione
tr-cut = Stacco
tr-crossfade = Dissolvenza incrociata
tr-fade = Dissolvenza al nero
ed-audio = Tracce audio
ed-track-mix = Traccia 1 (mix registrato)
ed-track-n = Traccia { $n }
ed-separate-audio = Esporta anche le tracce selezionate separatamente
ed-overlays = Testo, sottotitoli e sticker
ed-size = Dimensione
ed-outline = Contorno
ed-from = da
ed-to = a
ed-add-text = Aggiungi testo
ed-image-path = Percorso immagine (PNG, JPG…)
ed-add-image = Aggiungi immagine
ed-srt-path = Percorso sottotitoli .srt
ed-import-srt = Importa .srt
ed-output = Uscita
ed-format = Formato
ed-height = Risoluzione
ed-target = Dimensione massima
ed-webm-note = Il WebM viene esportato in AV1 senza audio (manca ancora un encoder Opus locale). Per l'audio usa MP4.
ed-name = Nome file
ed-export = Esporta
ed-clear = Nuovo progetto
ed-queue = Coda di esportazione
ed-queue-empty = Nessuna esportazione. Le esportazioni continuano anche se chiudi questa finestra.
job-queued = In coda
job-cancel = Annulla

## Aggiornamenti
upd-title = Aggiornamento software
upd-check-now = Controlla ora
upd-checking = Controllo su GitHub…
upd-up-to-date = Hai già l'ultima versione.
upd-error = Controllo aggiornamenti non riuscito: { $error }
upd-available = È disponibile la versione { $version }
upd-install = Installa ora
upd-later = Più tardi
upd-ignore = Salta questa versione
upd-downloading = Download e verifica di { $version }…
upd-privacy = Vengono contattati solo api.github.com e github.com, con uno User-Agent generico e senza identificativi. I pacchetti vengono verificati con firma e SHA-256 prima dell'installazione.
