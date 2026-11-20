# Privacy

Hyper Clips è progettato perché i tuoi dati non lascino mai il tuo PC.
*Hyper Clips is designed so your data never leaves your PC.*

## Cosa NON fa
- Nessuna telemetria, analytics, crash report automatici o identificativi univoci.
- Nessun account, nessun cloud, nessun servizio di terze parti, nessun font o CDN remoto.
- Nessuna lettura della memoria di altri processi e nessuna iniezione nei giochi.

## Rete
L'unica connessione prevista è il **controllo aggiornamenti** verso
`api.github.com` / `github.com` (Fase 5), disattivabile nelle impostazioni.
La richiesta usa uno User-Agent generico e non contiene alcun dato personale.
Le funzioni di condivisione (webhook Discord, server LAN, upload) partono solo
su azione esplicita e sono disattivate di default.

Nella Fase 1 l'app **non apre alcuna connessione di rete**. L'unico canale di
comunicazione è una named pipe locale, accessibile solo all'utente corrente e
chiusa ai client remoti.

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
Dalla Fase 6 il pannello **Privacy Audit** mostra le connessioni aperte
dall'app e permette di cancellare tutti i dati.
