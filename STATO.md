# STATO

## Phase 0 - Plan: done (approved by running on the recommended choices)

## Phase 1 - Core: written, NOT yet compiled on Windows
Done in the workspace:
- hc-core: config (TOML, validated, atomic save), paths + portable mode, hotkey parsing/conflicts,
  i18n (embedded en/it + external override), local logging, IPC types + framing, platform traits.
- hc-os-win: tray + menu, global hotkeys (RegisterHotKey), single instance, named-pipe server,
  memory probe, shell open, locale, error dialog.
- apps/hyperclips (service) and apps/hyperclips-ui (egui spike).
- CI (fmt, clippy -D warnings, test, release build on windows-latest), deny.toml, memory script.

Verified in the authoring sandbox (Linux, no crates.io access):
- 8 unit tests of the std-only modules (hotkey, ipc framing) pass against a stub error type.

NOT verified (needs a Windows machine with network):
- Anything that depends on serde/toml/tracing/windows/eframe: no compile, no clippy, no other tests.
- All Win32 calls were written from memory of windows 0.61. Expect a few signature fixes.
- RAM/CPU numbers: none measured yet.

## Known limitations (Phase 1)
- Capture, replay buffer, save-clip and recording are stubs (Phase 2).
- Tray uses the stock application icon; no .ico resource, no version resource.
- Pipe uses the default security descriptor (remote clients rejected); explicit user-only DACL to do.
- Pipe client is blocking without timeout.
- `general.start_with_windows` is parsed but not applied yet.
- If the UI is already open, "Open" does not focus it.
- LICENSE file (GPL-3.0 text) not added yet.
- Dependencies pinned to eframe 0.31 / windows 0.61; newer releases exist (0.37.0 / 0.62.2).

## Next
Compile on Windows (`cargo build`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test`),
send me the errors, measure RAM of service and UI, then decide the UI toolkit and start Phase 2.
