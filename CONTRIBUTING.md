# Contribuire / Contributing

Grazie dell'interesse! *Thanks for your interest!*

## Requisiti
- Windows 10 22H2+ o Windows 11, Rust stable (toolchain MSVC).
- `cargo install --locked cargo-deny cargo-audit`

## Prima di aprire una PR
```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
```

## Regole
- **Privacy prima di tutto:** nessuna nuova chiamata di rete, telemetria o dipendenza con tracking. Ogni dipendenza nuova va motivata nella PR.
- **Il core resta leggero:** niente toolkit UI, FFmpeg o stack di rete in `apps/hyperclips` e nei crate che usa (solo API multimediali del sistema operativo).
- Niente `unwrap()` nel codice di produzione (`clippy::unwrap_used` è vietato). Ogni blocco `unsafe` ha un commento `// SAFETY:`.
- Commenti nel codice in inglese. Testi dell'interfaccia in `locales/<lingua>/main.ftl`, sempre in italiano **e** in inglese (un test verifica che abbiano le stesse chiavi).
- Per le modifiche che toccano le prestazioni, allega le misure di `hc-bench`.

## Diagnostica
- `hc-bench encoders | probe <mp4> | frame <mp4> <png> | levels <mp4> | audio-sessions | service-test <s> <mp4> [--mic --disk --av1 --region --window --webcam]`.
- `HC_NATIVE_CRASH_LOG=1` scrive in `crash.log` il backtrace degli access violation (file locale, nessun invio).

## Traduzioni
Copia `locales/en/main.ftl` in `locales/<codice>/main.ftl` e traduci i valori.
Un file in `locales/` accanto all'eseguibile ha la precedenza su quelli incorporati.

## Licenza
Contribuendo accetti che il tuo codice sia distribuito con licenza GPL-3.0-or-later.
