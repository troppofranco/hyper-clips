# STATO

## Phase 0 - Plan: done (approved by running on the recommended choices)

## Phase 1 - Core: DONE, verified by CI on windows-latest (branch `dev`)
- cargo fmt, clippy -D warnings, 23 unit tests, release build of the whole workspace: all green.
- cargo deny: green (two documented "unmaintained" advisories from the egui 0.31 stack are ignored in deny.toml).
- Workflow: I push to `dev`, read the Actions result with `gh`, and fix errors myself.

Still NOT verified (needs a real PC, by hand): tray icon/menu, hotkeys, second-launch behaviour,
UI window, RAM numbers (run check-all.bat and send check-report.txt).

## Phase 2 - Capture + replay: DONE (compiles, clippy/test/deny green in CI) - NOT tested on a real PC
- Capture: borderless WGC, Desktop Duplication fallback (no yellow border). Primary monitor only.
- Audio: WASAPI loopback + mic, AAC MFT, multi-track MP4. Video: software H.264 MFT, CPU NV12 conversion.
- Replay buffer, clip saving (Alt+F10), manual recording (Alt+F9, streaming MP4 writer), PNG screenshots (Alt+F8).
- Game detection by fullscreen foreground program (tags clips).

## Phase 3 - Library: DONE (CI green, not tested on a PC)
hc-library (SQLite: tags, favorites, filters, auto-delete rules, folder scan). UI tabs: Library, Settings.

## Phase 5/6 (partial): updater + installer DONE (see docs/RELEASING.md); auto-update stays disabled until the minisign key is set.

## Still TO DO
- Hardware encoders (NVENC/AMF/QSV, async MFTs), GPU colour conversion, HEVC/AV1, HDR, fullscreen-exclusive edge cases.
- Window / region / multi-monitor capture, per-process audio, webcam overlay, in-game overlay, bookmarks.
- Auto-delete execution (rules exist in hc-library, not scheduled), editor/export, Discord webhook, LAN share, vault, plugins, CLI/API, backup/restore, Privacy Audit panel, metadata stripping.
- LICENSE (GPL-3.0 text), Cargo.lock, tray icon, pipe DACL, start_with_windows, per-game profiles.

## Known limitations
- Everything above is verified only by compilation and unit tests on CI; runtime behaviour needs your testing.
- Software H.264 encoding and CPU conversion are heavy at high resolutions/fps: lower fps/bitrate if the game stutters.
- Settings are applied after restarting the service.
- Pinned to eframe 0.31 / windows 0.61 (newer exist, unverified); two "unmaintained" advisories ignored in deny.toml.
