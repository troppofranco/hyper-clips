# Privacy

Hyper Clips collects nothing. There is no telemetry, analytics, tracker, account, automatic crash report or third-party service.

- **Data location:** settings, logs and the clip database live in `%APPDATA%\HyperClips` (or the `data` folder next to the program in portable mode). Clips go to `Videos\HyperClips` unless you change it.
- **Network:** the only connection is the update check to `api.github.com` / `github.com` (public Releases). It sends nothing but the HTTP request itself. Turn it off with `update_check = "never"` in `[privacy]` of `config.toml`.
- **Update integrity:** downloads are verified with SHA-256 and a minisign (ed25519) signature before anything is installed. Without the embedded public key, updates are refused.
- **Microphone:** off by default (`audio.microphone = false`).
- **Logs:** local only, never uploaded.

Verify it yourself: the code is open, `cargo deny` runs in CI, and you can watch the process with any firewall or `netstat`.
