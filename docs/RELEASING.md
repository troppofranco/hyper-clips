# Rilasci e aggiornamenti automatici

## Come funziona l'aggiornamento
1. L'app (core) controlla `https://api.github.com/repos/troppofranco/hyper-clips/releases/latest`
   (canale stabile) o `/releases` (canale beta), secondo le impostazioni:
   all'avvio, ogni N ore, oppure mai. Owner e repository sono costanti in
   `crates/hc-updater/src/lib.rs` (`OWNER`, `REPO`).
2. Le richieste usano lo User-Agent generico `HyperClips-Updater`, senza identificativi,
   e ognuna viene registrata in `logs\network.log`, consultabile dal Privacy Audit.
3. Prima di installare, lo studio scarica `checksums.txt` e `checksums.txt.minisig`:
   - verifica la firma **minisign (Ed25519)** con la chiave pubblica incorporata nell'app
     (`keys/hyperclips.pub.b64`);
   - pretende che il *trusted comment* sia `hyperclips <versione>`, per impedire che venga
     riproposta una vecchia release firmata;
   - scarica il pacchetto e ne confronta lo **SHA-256**.

   Se un qualsiasi controllo fallisce, l'aggiornamento viene annullato.
4. `hc-updater-helper.exe`, copiato in una cartella temporanea:
   - aspetta la chiusura dell'app e fa un backup dei file di programma;
   - installa il pacchetto: `Setup.exe /VERYSILENT` per la versione installata, estrazione
     dello zip per quella portable (la cartella `data\` non viene mai toccata);
   - avvia la nuova versione con un controllo di salute: se entro 30 s non risponde,
     **ripristina il backup** e riavvia la versione precedente. Il log è in `update.log`
     nella cartella dati.

## Chiavi di firma
La coppia di chiavi si genera **una sola volta**, fuori dal repository:

```powershell
cargo run -p hc-sign -- keygen "$env:USERPROFILE\.hyperclips-release-keys"
```

Vengono creati quattro file:
- `hyperclips.pub` e `hyperclips.pub.b64`: chiave pubblica, da copiare in `keys/` e committare;
- `hyperclips.key`: chiave segreta, cifrata con la password;
- `hyperclips.password`: la password.

**La chiave segreta e la password non vanno mai nel repository** (`.gitignore` le esclude).
Conservane un backup offline: senza la chiave non potrai più pubblicare aggiornamenti
che le installazioni esistenti accettino.

### Configurare i secret di GitHub Actions
Una volta sola, dalla cartella del progetto:

```powershell
Get-Content "$env:USERPROFILE\.hyperclips-release-keys\hyperclips.key" -Raw | gh secret set MINISIGN_SECRET_KEY --repo troppofranco/hyper-clips
```
```powershell
Get-Content "$env:USERPROFILE\.hyperclips-release-keys\hyperclips.password" -Raw | gh secret set MINISIGN_PASSWORD --repo troppofranco/hyper-clips
```

### Rotazione della chiave
Pubblica prima una release firmata con la chiave **vecchia** che contenga la nuova chiave
pubblica. Da lì in poi firma con la chiave nuova.

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
   - genera `checksums.txt` e lo firma;
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

La firma minisign degli aggiornamenti è indipendente da queste opzioni: protegge
l'auto-update anche senza certificato.
