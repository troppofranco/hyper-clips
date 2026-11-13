# Rilasci e aggiornamenti automatici

Ogni push su `main` pubblica una GitHub Release `v0.1.<numero>` con:

- `HyperClips-Setup-<versione>.exe` (installer per-utente, IT/EN, senza admin)
- `HyperClips-<versione>-portable.zip`
- `checksums.txt` (SHA-256)
- le firme `*.minisig` (solo se la chiave di firma è configurata)

L'app controlla `releases/latest` all'avvio (dopo 10 s) e dal menu della tray
("Cerca aggiornamenti"). Installa **solo** se SHA-256 e firma minisign sono validi.

## Configurare la firma (una volta sola)

Senza chiave l'app NON si aggiorna da sola (rifiuta pacchetti non verificabili).

1. Genera la coppia di chiavi (serve Rust). La chiave privata resta sul tuo PC e **non va mai nel repo**:
   ```powershell
   cargo install rsign2 --locked
   rsign generate -W -p minisign.pub -s minisign.key
   ```
   `-W` crea la chiave senza password (serve per firmare in automatico nella CI).
2. Metti la chiave **pubblica** nel repo, sostituendo il contenuto di
   `crates/hc-updater/minisign.pub` con quello di `minisign.pub`, poi fai commit.
3. Su GitHub: *Settings -> Secrets and variables -> Actions -> New repository secret*
   - Nome: `MINISIGN_SECRET_KEY`
   - Valore: l'intero contenuto del file `minisign.key`
4. Cancella `minisign.key` dal PC (o conservalo in un posto sicuro fuori dal repo).
5. Fai un push su `main`: la release successiva è firmata e contiene la chiave pubblica.
   **Installa quella** (o una più recente): le versioni costruite prima non hanno la chiave.

Se perdi o compromettI la chiave privata: genera una nuova coppia, sostituisci
`minisign.pub` e il secret, e reinstalla a mano una volta.

## Come funziona l'aggiornamento

1. L'app scarica `checksums.txt`, il pacchetto e la sua firma e verifica entrambe.
2. Mostra la versione e le note di rilascio: **Sì** installa, **No** rimanda,
   **Annulla** salta quella versione (`updates.skip_version` in `config.toml`).
3. Copia `hc-updater-helper.exe` in una cartella temporanea, lo avvia e si chiude.
4. L'helper aspetta che l'app sia chiusa, fa il backup dei file del programma
   (mai dati e clip), esegue il Setup in modalità silenziosa (o sostituisce i file
   se portable) e, se qualcosa fallisce, ripristina il backup e riavvia la versione precedente.
   Il suo log è in `%TEMP%\hyperclips-update.log`.

Canale beta: in `config.toml` imposta `[updates] channel = "beta"`.
Disattivare il controllo: `[privacy] update_check = "never"`.

## Limiti noti

- Senza certificato di code signing Windows SmartScreen può avvisare al primo avvio.
  Per un progetto open source esistono servizi di firma gratuiti (es. SignPath Foundation).
- Ogni push su `main` crea una release: è voluto, ma riempie la lista. Le release vecchie si possono cancellare.
- Il rollback ripristina i file se l'installazione fallisce; non verifica che la nuova versione parta correttamente.
