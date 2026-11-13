# STATO

## Phase 0 - Plan: done (approved by running on the recommended choices)

## Phase 1 - Core: DONE, verified by CI on windows-latest (branch `dev`)
- cargo fmt, clippy -D warnings, 23 unit tests, release build of the whole workspace: all green.
- cargo deny: green (two documented "unmaintained" advisories from the egui 0.31 stack are ignored in deny.toml).
- Workflow: I push to `dev`, read the Actions result with `gh`, and fix errors myself.

Still NOT verified (needs a real PC, by hand): tray icon/menu, hotkeys, second-launch behaviour,
UI window, RAM numbers (run check-all.bat and send check-report.txt).

## Phase 2 - Capture + replay: IN PROGRESS (branch `dev`, CI green)
Done and compiling on Windows (CI): hc-buffer (7 tests), hc-capture-win (WGC, primary monitor),
hc-encode-win (software H.264 MFT, CPU NV12 conversion with tests, MP4 muxer without re-encode),
service wiring: replay buffer runs at startup, hotkey Alt+F10 / tray "Save clip" / UI button write
`HyperClips_<date>.mp4` (default folder `Videos\HyperClips`, or `clips\` in portable mode).
Config: new [video] fps / bitrate_kbps and [storage] clips_dir.

NOT verified on a real PC yet: that capture, encoding and the saved MP4 actually work and play.
Expected weak points: CPU use (software encoder + CPU colour conversion), HDR/fullscreen games, size changes.

Still to do in Phase 2: audio (WASAPI loopback, mic, per-process, AAC, multi-track MP4),
hardware encoders (NVENC/AMF/QuickSync, async MFTs), GPU colour conversion, HEVC/AV1,
game detection + capture of a single game/window/region, manual recording, screenshots, HDR, overlay.

## Known limitations (Phase 1)
- Manual recording and screenshots are still stubs. No audio yet. Only the primary monitor is captured.
- Encoding is software H.264 on the CPU for now (hardware encoders and GPU conversion come next).
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
