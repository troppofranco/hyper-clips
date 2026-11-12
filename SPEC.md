# Hyper Clips - SPEC

Native Windows 10 22H2 / 11 clip recorder in Rust, feature-parity with Medal.tv (minus Medal's
own publishing/social platform), privacy-first and light. Repo: https://github.com/troppofranco/hyper-clips
License: GPL-3.0-or-later.

## Numeric targets
| Metric | Target | How it is measured |
|---|---|---|
| Service RAM (tray + replay active), excluding the configured replay buffer | < 60 MB private working set | `scripts/measure-memory.ps1` |
| UI open | < 150 MB private working set | same script |
| CPU while recording (HW encoder) | < 3 % | Task Manager / perf counters |
| Game FPS impact | < 2 % | PresentMon |
| Start to tray | < 1 s | manual timing |
| Release profile | lto="fat", codegen-units=1, panic="abort", strip=true, opt-level=3 | Cargo.toml |

Private working set, not total working set: shared DLL pages (graphics drivers) are excluded.

## Non-negotiable
- No telemetry, trackers, analytics, accounts, crash reports, third-party services, remote fonts.
- Network: only the update check to api.github.com / github.com (can be disabled).
- All data local (`%APPDATA%\HyperClips`, or `data\` next to the exe when `portable.flag` exists).
- Crate hygiene: `cargo audit`, `cargo deny`, `clippy -D warnings`, no `unwrap()` in critical paths.

## Architecture
- `hyperclips.exe`: service (tray, hotkeys, capture, encode, buffer). No UI toolkit linked.
- `hyperclips-ui.exe`: started on demand by the service, closed when the window closes.
- `hc-updater-helper.exe`: replaces files after the app exits (Phase 5).
- IPC: JSON lines over a per-user named pipe (`\\.\pipe\HyperClips-<user>`).
- Platform-independent crates: hc-core, hc-buffer, hc-library, hc-editor, hc-detect, hc-updater, hc-privacy, hc-ui.
- Windows-specific crates behind traits from `hc-core::platform`: hc-os-win, hc-capture-win, hc-audio-win, hc-encode-win.

## Decisions
- UI: egui (eframe, glow renderer). To be confirmed or replaced by the Phase 1 RAM measurement.
- Capture: Windows Graphics Capture, Desktop Duplication fallback. No injection / DXGI hook in the base plan.
- Export/editor: external `ffmpeg.exe` subprocess. Replay buffer: native encoders, muxing without re-encode.
- Updates: minisign signature + SHA-256, per-user Inno Setup installer.

## Roadmap
0 Plan - 1 Core - 2 Capture + replay - 3 Library + player - 4 Editor + export - 5 Updater + CI -
6 Extras (CLI/API, backup, local highlights, low-impact/battery, vault, LAN, WASM) - 7 Optimize, audit, installer, portable.
