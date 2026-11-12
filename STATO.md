# STATO

## Phase 0 - Plan: done (approved by running on the recommended choices)

## Phase 1 - Core: DONE, verified by CI on windows-latest (branch `dev`)
- cargo fmt, clippy -D warnings, 23 unit tests, release build of the whole workspace: all green.
- cargo deny: green (two documented "unmaintained" advisories from the egui 0.31 stack are ignored in deny.toml).
- Workflow: I push to `dev`, read the Actions result with `gh`, and fix errors myself.

Still NOT verified (needs a real PC, by hand): tray icon/menu, hotkeys, second-launch behaviour,
UI window, RAM numbers (run check-all.bat and send check-report.txt).

## Known limitations (Phase 1)
- Capture, replay buffer, save-clip and recording are stubs (Phase 2).
- Tray uses the stock application icon; no .ico resource, no version resource.
- Pipe uses the default security descriptor (remote clients rejected); explicit user-only DACL to do.
- Pipe client is blocking without timeout.
- `general.start_with_windows` is parsed but not applied yet.
- If the UI is already open, "Open" does not focus it.
- LICENSE file (GPL-3.0 text) not added yet.
- Dependencies pinned to eframe 0.31 / windows 0.61; newer releases exist (0.37.0 / 0.62.2).
- The pinned egui stack pulls in unmaintained crates (paste, ttf-parser): re-check on upgrade.

## Next
Phase 2: capture (Windows Graphics Capture) + encoders + replay ring buffer + audio.
