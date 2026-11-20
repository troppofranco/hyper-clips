# Rilasci e aggiornamenti automatici

## Come funziona l'aggiornamento
1. L'app (core) controlla `https://api.github.com/repos/troppofranco/hyper-clips/releases/latest`
   (canale stabile) o `/releases` (canale beta), secondo le impostazioni:
   all'avvio, ogni N ore, oppure mai. Owner e repository sono costanti in
   `crates/hc-updater/src/lib.rs` (`OWNER`, `REPO`).
2. Le richieste usano lo User-Agent generico `HyperClips-Updater`, senza identificativi,
   e ognuna viene registrata in `logs\network.log`, consultabile dal Privacy Audit.
3. Prima di installare, lo studio scarica `checksums.txt` dalla release e confronta lo
   **SHA-256** del pacchetto scaricato. Se non corrisponde, l'aggiornamento viene annullato.
   Il controllo protegge da download corrotti o incompleti; l'autenticità dei pacchetti
   dipende dalla sicurezza dell'account GitHub che pubblica le release (usa la 2FA).
4. `hc-updater-helper.exe`, copiato in una cartella temporanea:
   - aspetta la chiusura dell'app e fa un backup dei file di programma;
   - installa il pacchetto: `Setup.exe /VERYSILENT` per la versione installata, estrazione
     dello zip per quella portable (la cartella `data\` non viene mai toccata);
   - avvia la nuova versione con un controllo di salute: se entro 30 s non risponde,
     **ripristina il backup** e riavvia la versione precedente. Il log è in `update.log`
     nella cartella dati.

Non servono chiavi né secret su GitHub: il workflow usa solo il token automatico
`GITHUB_TOKEN` per creare la release.
## Pubblicare una release
1. Aggiorna `version` in `Cargo.toml` (`[workspace.package]`) e aggiungi a `CHANGELOG.md`
   una sezione `## [x.y.z]`.
2. Esegui commit, crea il tag e fai push:
```powershell
git tag v0.2.0
```
```powershell
git push origin v0.2.0
```
3. Il workflow `release.yml`:
   - esegue i test e compila;
   - crea `HyperClips-<v>-portable.zip` e `HyperClips-Setup-<v>.exe` (Inno Setup);
   - genera `checksums.txt` (SHA-256 di ogni file);
   - pubblica la release con le note prese dal CHANGELOG.

   I tag con un trattino (es. `v0.2.0-beta.1`) diventano pre-release, visibili solo
   a chi usa il canale beta.

## SmartScreen e firma del codice
Senza un certificato di firma del codice, Windows SmartScreen può mostrare
"Windows ha protetto il PC" ai primi download, finché il file non accumula reputazione.

Opzioni per ridurre l'avviso:
- **SignPath Foundation**: firma gratuita per progetti open source, integrata con GitHub Actions;
- **Certum Open Source Code Signing**: certificato a basso costo per sviluppatori open source;
- **Azure Trusted Signing**: servizio a pagamento con canone mensile basso, con requisiti
  di verifica dell'identità.

