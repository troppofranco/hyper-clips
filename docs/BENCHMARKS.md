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
