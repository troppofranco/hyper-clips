# Benchmarks

Measured with `tools/hc-bench` (`hc-bench sample <exe> <seconds>`), release
profile, portable mode. Working set = RAM physically used by the process
(includes shared DLL pages); private bytes = memory committed only for it.

Machine: Windows 11 Pro 26300, NVIDIA GeForce RTX 5070 + AMD Radeon iGPU.

## Phase 1 — 2026-11-20

| Scenario | Working set | Private | CPU avg |
|---|---|---|---|
| Core, tray only (`--background`), 20 s | **16.3 MB** | 5.8 MB | 0.00 % |
| Studio open, idle, glow renderer, 10 s | **128.6 MB** | 124.0 MB | 0.01 % |
| Core while studio open | 15.2 MB | 5.7 MB | 0.00 % |
| **Core + studio** | **143.8 MB** | 129.7 MB | — |
| Core after studio closed | 15.2 MB | 5.7 MB | 0.00 % |

Startup to tray (core log `tray ready`): **23 ms** after process creation.

Binary sizes: `hyperclips.exe` 1.3 MB, `hyperclips-studio.exe` ~5.9 MB (glow).

### Studio renderer comparison (idle window)

| Renderer | Working set | Private |
|---|---|---|
| eframe **glow** (OpenGL) — default | 132 MB | 130 MB |
| wgpu, auto backend | 267 MB | 450 MB |
| wgpu, DX12, NVIDIA | 156 MB | 367 MB |
| wgpu, DX12, AMD iGPU | 238 MB | 437 MB |
| wgpu, Vulkan | 179 MB | 357 MB |
| wgpu, GL | 139 MB | 145 MB |

Decision: glow by default. A custom Direct3D 11 egui painter is planned for
Phase 3 (it will share the D3D11 device used for hardware video decode),
expected to land well below glow based on the spike below.

### Spike: cost of a D3D11 device (Phase 2 risk)

`hc-bench spike-d3d11`:

| Step | Working set | Private |
|---|---|---|
| Baseline | 6.1 MB | 1.1 MB |
| + DXGI factory | 8.7 MB | 1.7 MB |
| + D3D11 device on RTX 5070 | 37.7 MB | 52.2 MB |
| + D3D11 device on AMD iGPU | 65.1 MB | 77.5 MB |

One D3D11 device on the NVIDIA driver costs ≈ **+29 MB working set /
+50 MB private**. Consequences for Phase 2: the core must create exactly one
device (on the adapter driving the captured monitor) and only while capture is
active; the projected core budget is ~16 + 29 + encoder session, which keeps
the < 60 MB working-set goal reachable but tight. Private bytes will exceed
60 MB and will be reported honestly alongside the working set.

## Phase 2 — 2026-11-20 (capture)

Settings: 1920×1080 @ 60 fps H.264 VBR 20 Mbps (NVIDIA H.264 Encoder MFT on
RTX 5070), source monitor 2560×1440, system audio AAC 160 kbps, Instant
Replay 60 s in RAM, a game running fullscreen on the captured monitor.

| Scenario | Working set | Private | CPU avg |
|---|---|---|---|
| Core, replay active, 70 s (`hc-bench sample`) | 80–103 MB | 130–152 MB | **0.10 %** |
| — of which replay buffer (`--status`) | 21.4 MB | | |
| **Core excluding buffer** | **81 MB** | ~131 MB | |

Pipeline cost by stage (`hc-bench spike-pipeline`, bare process):

| Stage | Working set | Private |
|---|---|---|
| baseline + COM/Media Foundation | 7.9 MB | 1.5 MB |
| + D3D11 device (NVIDIA driver) | +27.8 | +51.5 |
| + Desktop Duplication, first frame | +0.5 | +15.4 |
| + NV12 converter (8 surfaces; now 4) | +1.0 | +29.7 |
| + NVIDIA H.264 Encoder MFT | +16.7 | +32.1 |
| + 2 s of encoding | +7.7 | +0.9 |
| + AAC encoder | +0.7 | +0.2 |

**Target miss:** the < 60 MB working-set goal (excluding buffer) is not met
(81 MB). ~45 MB is GPU driver + NVENC MFT user-mode memory that any capture
app on this driver pays. Planned mitigations (Phase 7): native NVENC
backend without the MFT layer, smaller surface pools, encoder session
created lazily. Startup to tray stays at 21 ms; the capture pipeline is
ready 250 ms later.

Encode submit time: 0.15 ms/frame on the capture thread (`hc-bench video-test`).
Game FPS impact not yet measured (needs PresentMon on a reproducible
benchmark: run by the user, see CONTRIBUTING).

## Phase 2 completion — 2026-11-20

Defaults now include cursor compositing and separate per-process audio
tracks (Mix, Game, Chat, Other). Core RAM excluding the replay buffer,
25 s after start, one setting changed at a time:

| Configuration | Working set (excl. buffer) |
|---|---|
| No cursor, single system track | 83.1 MB |
| + cursor compositing (D3D11 shaders, extra BGRA target) | 86.9 MB (+3.8) |
| + separate audio tracks (3 process-loopback clients, 4 AAC encoders) | 112.3 MB (+29.2) |
| All defaults | 116.9 MB |

70 s with all defaults, game running: CPU **0.57 %** avg (max 1.17 %),
working set 124–276 MB of which up to 149 MB is the 60 s replay buffer
(20 Mbps VBR during action).

Per-process audio verified with `hc-bench service-test` + pre-encode level
log, a game and Discord running:
`Mix=-31.6 dB Game=-31.6 dB Chat=-96.7 dB System=-96.7 dB` (game only) and
`Mix=-23.2 Game=-34.4 Chat=-23.5 System=-23.5` (game + voice): the game is
isolated, and "System" excludes it.

Disk replay buffer (`--disk`): 30 s window → 223 MB ring file, deleted on
close; RAM for the buffer drops to metadata only (~40 bytes/packet).

Studio idle (settings page reachable): 73.6 MB working set.

**Target status:** core < 60 MB is still missed (83 MB minimum). Turning
off separate tracks saves ~29 MB. Phase 7 work: native NVENC/AMF without
the MFT layer, sharing one WASAPI activation for the process-loopback
sources, lazy compositor.

## Phase 3 — 2026-11-20 (library)

`hc-bench fake-library` with 1,000 items (thumbnails 256×144):

| Studio state | Working set | Private | CPU |
|---|---|---|---|
| Library open, idle | 89.2 MB | 91.5 MB | 0.02 % |
| After scrolling the whole gallery | 90.4 MB | 136.8 MB | 0.04 % |
| Player open (1080p clip) | ~165 MB | — | — |

Thumbnails are decoded only for visible cards (max 12 per frame) and kept
in an LRU of 300 textures. The player's D3D11 device and Media Engine exist
only while a clip is open (+~70 MB), and are released when it is closed.

## Phase 4 — 2026-11-20 (export)

`hc-bench export-test` on a 7 s 1080p60 clip, two segments (second at
0.5×) with crossfade and a text overlay, RTX 5070:

| Preset / format | Output | Time | Size |
|---|---|---|---|
| Discord (720p, target 10 MB) | 1280×720 H.264 + AAC | 1.1 s | 0.46 MB |
| Shorts (9:16) | 1080×1920 H.264 + AAC | 1.8 s | 14.3 MB |
| Target 1.5 MB at 12 Mbps start | 1920×1080 HEVC | 3.6 s (2 passes) | 1.18 MB |
| WebM | 1280×720 AV1 (no audio) | 1.3 s | — |
| GIF | 640×360, 150 frames | 3.8 s | 12.7 MB |
| WebP (lossless) | 640×360 | 0.7 s | 30.1 MB |

Animated WebP is lossless (no lossy encoder without libwebp), so files are
large; GIF is usually the smaller choice.
