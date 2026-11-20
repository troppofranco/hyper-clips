# Privacy

Hyper Clips è progettato perché i tuoi dati non lascino mai il tuo PC.
*Hyper Clips is designed so your data never leaves your PC.*

## Cosa NON fa
- Nessuna telemetria, analytics, crash report automatici o identificativi univoci.
- Nessun account, nessun cloud, nessun servizio di terze parti, nessun font o CDN remoto.
- Nessuna lettura della memoria di altri processi e nessuna iniezione nei giochi.

## Rete
L'unica connessione automatica è il **controllo aggiornamenti** verso
`api.github.com` / `github.com`: all'avvio, ogni N ore oppure mai (impostazioni).
La richiesta usa uno User-Agent generico (`HyperClips-Updater`) e non contiene dati
personali, identificativi o statistiche; ogni richiesta viene annotata in
`logs\network.log`. Gli aggiornamenti vengono installati solo se la firma minisign
e lo SHA-256 corrispondono.

Tutto il resto parte solo su tua azione esplicita ed è spento di default:
- **Condivisione in rete locale**: un piccolo server HTTP nello studio, raggiungibile
  solo nella tua rete, con un token casuale nel link, scadenza e limite di download.
  Il traffico non è cifrato; Windows Firewall può chiedere il permesso al primo uso.
- **API locale**: ascolta solo su `127.0.0.1`, richiede un token e rifiuta le richieste
  provenienti dai browser.
- **Plugin**: moduli WebAssembly senza alcun accesso a rete, file o sistema.

La comunicazione tra core, studio e CLI usa una named pipe locale, accessibile solo
all'utente corrente e chiusa ai client remoti.

Il pannello **Privacy Audit** mostra in tempo reale le connessioni TCP aperte dai
processi di Hyper Clips (dalla tabella di Windows), il registro delle richieste in
uscita, le cartelle usate e permette di cancellare tutti i dati.

## Vault
Il vault cifra le clip con XChaCha20-Poly1305; la chiave principale è protetta dalla
password tramite Argon2id. Nomi dei file e metadati sono cifrati. Non protegge da
malware in esecuzione mentre il vault è sbloccato; rivela il numero approssimativo
di file e le loro dimensioni. L'eliminazione dell'originale dopo la cifratura è una
normale cancellazione, non una sovrascrittura sicura (su SSD non sarebbe affidabile).
Senza password i dati non sono recuperabili.
## Cattura
- Il **microfono è disattivato di default**: va attivato esplicitamente nelle impostazioni. Anche la **webcam** è spenta di default e la camera viene aperta solo se la attivi.
- La soppressione del rumore (RNNoise) e il noise gate girano in locale; non viene inviato alcun audio.
- Le tracce audio per processo usano il "process loopback" di Windows: nessun handle verso il processo del gioco, nessuna iniezione.
- L'overlay delle notifiche è escluso dalle catture (`WDA_EXCLUDEFROMCAPTURE`) e non compare mai nelle clip.
- Il buffer di replay su disco è un file temporaneo nella cartella dell'app, aperto con cancellazione automatica alla chiusura (anche dopo un crash).
- Il riconoscimento del gioco legge solo il nome dell'eseguibile della finestra in primo piano, con un handle di sola interrogazione: non legge la memoria dei processi e non inietta nulla. Il database dei giochi è un file locale (`games.toml`).
- Le clip non contengono metadati: né titolo, né software, né date nel file. Il nome del gioco compare solo nel nome del file e della cartella.

## Dati salvati in locale

| Dato | Installata | Portable |
|---|---|---|
| Impostazioni (`config.toml`) | `%APPDATA%\HyperClips` | `.\data\config` |
| Log (7 giorni, percorsi anonimizzati) | `%LOCALAPPDATA%\HyperClips\logs` | `.\data\logs` |
| Libreria, cache, file temporanei | `%LOCALAPPDATA%\HyperClips` | `.\data` |
| Clip | `Video\Hyper Clips` | `.\data\clips` |

Il registro di Windows viene toccato solo nella versione installata, sotto
`HKCU`: la voce di avvio automatico (se attivata) e l'esclusione da Windows
Error Reporting (attiva di default, così nessun dump di crash viene inviato a
Microsoft).

## Cosa resta fuori dal nostro controllo
Il driver grafico salva la cache degli shader (es. `%LOCALAPPDATA%\NVIDIA\DXCache`)
e Windows registra Prefetch e Jump List. Sono comportamenti del sistema
operativo, non dell'app.

## Verifica
Il codice è aperto (GPL-3.0). Le dipendenze vengono controllate in CI con
`cargo deny`, che blocca anche i crate di telemetria noti, e con `cargo audit`.
Il pannello **Privacy Audit** permette di verificarlo direttamente.

---

## English summary
- No telemetry, analytics, automatic crash reports, accounts, cloud or remote fonts/CDNs.
- The only automatic network request is the optional update check against GitHub
  (generic User-Agent, logged in `logs\network.log`, packages verified with minisign + SHA-256).
- LAN sharing, the local API and plugins are off by default and start only when you enable them:
  LAN sharing is plain HTTP on your local network with a random token and expiry; the API listens on
  127.0.0.1 only and needs a token; plugins are sandboxed WebAssembly with no I/O at all.
- Microphone and webcam are off by default. Clips contain no metadata. Game detection only reads the
  foreground executable name; nothing is injected into games.
- The encrypted vault uses XChaCha20-Poly1305 with an Argon2id-protected key; it does not protect
  against malware while unlocked, and forgotten passwords cannot be recovered.
- The **Privacy Audit** panel shows live TCP connections of every Hyper Clips process, the outgoing
  request log and the folders in use, and can delete all data.
