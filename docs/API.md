# API locale, CLI e plugin / Local API, CLI and plugins

## CLI
`hyperclips.exe` accetta comandi (uno per volta). Codici di uscita: 0 ok, 1 errore,
2 funzione non disponibile, 3 core non in esecuzione.

| Comando | Effetto |
|---|---|
| `--save-clip` | salva il buffer Instant Replay |
| `--toggle-recording` | avvia/ferma la registrazione |
| `--screenshot` | screenshot |
| `--marker` | marker nella registrazione |
| `--open` | apre lo studio |
| `--status` | stato del core (memoria, buffer, conflitti hotkey) |
| `--list` | elenca la libreria (funziona anche a core spento) |
| `--export <id> --preset <discord\|youtube\|shorts\|twitter\|max\|gif\|webp>` | accoda un export in background |
| `--highlights <id>` | rileva i momenti salienti e li aggiunge come marker |
| `--backup-settings <file>` / `--restore-settings <file>` | backup e ripristino delle impostazioni |
| `--quit` | chiude il core |

L'output può essere reindirizzato (`hyperclips.exe --list > clip.txt`).

## API HTTP locale
Disattivata di default (Impostazioni → API locale). Ascolta **solo su 127.0.0.1**
(porta predefinita 47821) e richiede `Authorization: Bearer <token>`; il token è in
`<cartella dati>\api-token.txt`. Le richieste con intestazione `Origin` o con un `Host`
diverso da `127.0.0.1:<porta>`/`localhost:<porta>` vengono rifiutate (403), così una
pagina web non può pilotare l'API.

| Metodo | Percorso | Risposta |
|---|---|---|
| GET | `/v1/status` | stato del core (JSON) |
| GET | `/v1/clips?limit=N` | ultimi elementi della libreria (max 500) |
| POST | `/v1/clip` | salva una clip |
| POST | `/v1/recording/toggle` | avvia/ferma la registrazione |
| POST | `/v1/screenshot` | screenshot |
| POST | `/v1/marker` | marker |

Esempio (PowerShell):
```powershell
$t = Get-Content "$env:LOCALAPPDATA\HyperClips\library\api-token.txt"; Invoke-RestMethod -Method Post http://127.0.0.1:47821/v1/clip -Headers @{ Authorization = "Bearer $t" }
```

Errori: 401 token mancante o errato, 403 origine non ammessa, 404/405 percorso o metodo,
409 azione non disponibile (es. replay spento), 500 errore interno.

## Plugin (WebAssembly)
Disattivati di default. Ogni plugin è una cartella in `<cartella dati>\plugins\<id>\`
con `plugin.toml` e `plugin.wasm`. Vengono eseguiti con l'interprete **wasmi** in una
sandbox: nessun accesso a file, rete, orologio o sistema; limiti di carburante (CPU),
memoria (32 MB) e dimensione. Un plugin può solo, se autorizzato nel manifest,
aggiungere tag (`tags`) e mostrare notifiche (`notify`) quando una clip viene salvata.
ABI ed esempio in Rust: [`crates/hc-plugins/README.md`](../crates/hc-plugins/README.md).

---

## English
- **CLI**: same commands as above; output can be redirected.
- **Local HTTP API**: off by default; loopback only; bearer token from `api-token.txt`;
  requests with an `Origin` header or a foreign `Host` are refused. Endpoints as in the table.
- **Plugins**: off by default; sandboxed WebAssembly (wasmi interpreter) with no file,
  network or clock access, fuel and memory limits; capabilities limited to adding tags
  and showing notifications when the manifest asks for them. See the crate README.
