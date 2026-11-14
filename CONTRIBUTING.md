# Contributing

- Keep it light and private: no telemetry, no Electron/WebView, few and audited dependencies.
- Rust stable. Before a PR: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`.
- No `unwrap()` in non-test code (the workspace lint denies it).
- OS-specific code goes in `*-win` crates behind traits defined in `hc-core`.
- Code comments in English. New UI strings go in both `crates/hc-core/lang/en.toml` and `it.toml`.
- Do not invent APIs: when unsure about a Win32/crate call, say so in the PR.
