# Changelog

Every push to `main` publishes a release `v0.1.<run number>` (see `docs/RELEASING.md`); entries below are grouped by feature, newest first.

## Unreleased
- Hardware H.264 encoders (NVENC/AMF/QSV) with automatic software fallback (`video.encoder`).
- `hyperclips-cli`: list, trim, merge, export (MP4/WebM/GIF/WebP, 9:16, 1:1, speed), size shrink, Discord share (needs ffmpeg).
- Auto-delete rules (`[storage]`), Privacy tab.
- Full-session manual recording (Alt+F9) and PNG screenshots (Alt+F8).
- Game detection (fullscreen foreground program) and tagging of clips.
- UI: Library tab (search, favorites, tags, open, delete) and Settings tab.
- Clip library in SQLite (`rusqlite` 0.41 with patched SQLite).
- Audio: system loopback + microphone, AAC, multi-track MP4.
- Capture without the yellow border (borderless WGC, Desktop Duplication fallback).
- Updater (SHA-256 + minisign, rollback) and per-user installer + portable zip.
- Replay buffer, software H.264 encoding, tray, hotkeys, IPC, IT/EN localization.
