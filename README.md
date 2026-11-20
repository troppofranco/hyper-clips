# Hyper Clips

**🇮🇹 Italiano · [🇬🇧 English](#english)**

Registra e gestisci le clip dei tuoi giochi su Windows, con un consumo di
memoria minimo e **privacy totale**: nessun account, nessuna telemetria,
nessun tracker. Scritto in Rust.

> ⚠️ **Stato: sviluppo iniziale (Fase 2 di 7).** Funzionano Instant Replay, salvataggio clip,
> registrazione, marker e screenshot con encoder hardware e audio di sistema e microfono.
> Libreria ed editor arrivano nelle Fasi 3 e 4.

## Perché

| | Hyper Clips | App basate su Electron |
|---|---|---|
| RAM in background | ~16 MB solo tray · ~81 MB con replay attivo (buffer escluso) | centinaia di MB |
| Telemetria / account | nessuna / nessuno | spesso obbligatori |
| Rete | solo il controllo aggiornamenti verso GitHub (disattivabile) | continua |

Misure dettagliate: [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Compilare

Requisiti: Windows 10 22H2 o Windows 11, [Rust](https://rustup.rs) stable (MSVC).

```powershell
cargo build --release
```

Gli eseguibili si trovano in `target\release`. Per una prova senza toccare
`%APPDATA%` né il registro, crea un file vuoto `portable.flag` accanto a `hyperclips.exe`.

## Uso

- Avvia `hyperclips.exe`: compare l'icona nella tray e si apre la finestra.
- Hotkey predefinite: `F8` salva clip · `Ctrl+F9` registrazione · `Alt+F8` screenshot · `Shift+F8` marker · `Ctrl+Alt+H` apri finestra.
- CLI: `hyperclips.exe --save-clip | --toggle-recording | --screenshot | --marker | --open | --status | --quit`

## Documenti

[Architettura](docs/ARCHITECTURE.md) · [Privacy](PRIVACY.md) · [Contribuire](CONTRIBUTING.md) · [Changelog](CHANGELOG.md)

Licenza: [GPL-3.0-or-later](LICENSE).

---

<a id="english"></a>
## English

Record and manage game clips on Windows with a tiny memory footprint and
**complete privacy**: no account, no telemetry, no trackers. Written in Rust.

> ⚠️ **Status: early development (phase 2 of 7).** Instant Replay, clip saving,
> recording, markers and screenshots work with hardware encoding and system/mic audio.

**Build:** `cargo build --release` (Windows 10 22H2+/11, Rust stable MSVC).
Put an empty `portable.flag` next to `hyperclips.exe` for a self-contained test run.

**Default hotkeys:** `F8` save clip · `Ctrl+F9` record · `Alt+F8` screenshot ·
`Shift+F8` marker · `Ctrl+Alt+H` open window.

**Privacy:** the only network request the app will ever make is the optional
update check against GitHub. See [PRIVACY.md](PRIVACY.md).

License: [GPL-3.0-or-later](LICENSE).
