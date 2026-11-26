# PGMWatch — Refined V1 Architecture & Direction

## Problem Statement
How might we give a live broadcast engineer or show producer instant, unmistakable confidence that 20+ multi-language public streams are healthy (clean audio, live motion, in-sync A/V) and defensibly counted (normalized PCV/Hours Watched with clip evidence)—on any Windows 10/11 PC with zero external setup?

## Recommended Direction
Build **PGMWatch** as a portable Windows-first multi-process system centered around:
1. **FFmpeg 9.x Dual-Ready Media Pipeline (`vendor/ffmpeg-9.0` LGPL shared + system `ffmpeg 9.0.2` intact):**
   - `pgmwatch-worker.exe` abstracts decoding behind a `MediaBackend` trait supporting both **in-process FFmpeg 9.x FFI (`ffmpeg-sys-next 9.x`)** and an **FFmpeg 9 CLI (`d3d11va` + `scale_d3d11`) fallback**, leaving the host's `ffmpeg 9.0.2` on `PATH` completely untouched while bundling `avcodec-63.dll`..`swscale-10.dll` in the portable folder.
2. **Reliable Windows Build Choices:**
   - Avoid fragile MSVC `opus` C-crate builds by streaming **16-bit PCM / FFmpeg-encoded PFL audio over WebSocket** to Web Audio API.
   - Treat rotating platform URLs (Facebook/TikTok/YouTube) as a normal `REACQUIRING` hot-swap state inside the supervisor, preserving alert and 60s-grid continuity.
3. **Calm, Night-Shift MCR Interface (`/frontend-design` + `/unslop`):**
   - **Character:** `calm, exact, night-shift`
   - **World:** Broadcast master-control rack instruments under low-glare studio lighting.
   - **Layout:** Persistent Master Tally, Resource Governor & Match Marker bar across 4 dedicated views: **Live Wall (MCR)**, **Accounting & Overrides**, **Clips & Incidents**, and **Show Setup**.
   - **Visual System:** Graphite dark surfaces (`#0f0f10`–`#232325`) with 8% hairlines, `Geist`/`IBM Plex Sans` + `IBM Plex Mono` (`tabular-nums`), `lucide-react` 1.5px stroke icons, zero glows/gradients/emojis.

## Key Assumptions to Validate
- [ ] `ffmpeg-sys-next` (`9.x`) compiles cleanly against `vendor/ffmpeg-9.0` (`avcodec 63`) on `x86_64-pc-windows-msvc` once `LLVM` (`libclang.dll`) is available.
- [ ] `streamlink` (`runtime/bin/streamlink/`) + `yt-dlp.exe` (`--js-runtimes node:bin\node.exe`) resolve public YouTube/Twitch streams reliably under a bounded `Semaphore(3)`.
- [ ] `d3d11va` + `scale_d3d11` (`160x90` NV12/Y-plane + `320x180` JPEG) runs comfortably within DXGI VRAM budgets across multi-stream loads and degrades cleanly via the 3-Layer Governor.
- [ ] Browser-based Web Audio PFL playback + 5s `.mp4` clip replay feel instant (<250ms) inside the embedded React SPA.

## MVP / V1 Workspace Scope
- **In V1:**
  - `pgmwatch-core`: Shared domain types, tri-state metrics (`VALUE`/`N/A`/`UNAVAILABLE`), alert state machine (hysteresis, intermission, cooldowns), SQLite WAL schema & DB Writer Actor, 60s UTC grid aggregator, 4-tab `.xlsx` exporter (`rust_xlsxwriter`), `pgmwatch.toml` config & show preset loader.
  - `pgmwatch-worker`: Media worker with `MediaBackend` (FFmpeg 9 FFI + FFmpeg 9 CLI + synthetic test pattern source), canonical 48kHz audio DSP (`ebur128`, `rustfft`, clipping/silence/phase/hum), 6s GOP ring buffer -> 5s `.mp4` clip saver with 10 GB FIFO cap, video SIMD checks (black/blink/frozen/macroblock), thumbnail generator, PFL audio stream.
  - `pgmwatch-supervisor` (`PGMWatch.exe`): Axum HTTP/WS server + `rust-embed` UI, 3-layer Windows Governor (DXGI VRAM, RAM/CPU, WAN, Windows Job Objects, `SetThreadExecutionState`), 2-tier URL resolver (`streamlink`/`yt-dlp` + Edge CDP `msedge.exe`), YouTube/Twitch/TikTok collectors + Chat-as-a-Sensor, Intermission & Match Markers.
  - `ui/`: Embedded React + TypeScript + Vite + Tailwind MCR console (`Live Wall`, `Accounting`, `Clips & Incidents`, `Show Setup`).
  - `sidecar-stats/`: Bundled `tiktok-live-connector` runner via `bin\node.exe`.
  - `companion-module/`: TypeScript Bitfocus Companion module (`@companion-module/base` compatible) talking to `ws://127.0.0.1:8787/ws`.
  - `scripts/`: `build.ps1` and `package-portable.ps1` producing `dist/PGMWatch/` & `PGMWatch-Portable.zip`.

## Not Doing (and Why)
- **No AI/ONNX models in V1 (`sentinel-ai`)** — deterministic DSP/SIMD keeps false alarms near zero and avoids GPU contention; post-V1 extension point only.
- **No standalone `opus` C-library FFI** — Web Audio PCM / FFmpeg-backed encoding eliminates Windows MSVC CMake/autotools build failures.
- **No shipped Chromium binary** — leverages pre-installed Windows `msedge.exe` with isolated `--user-data-dir` profiles.
- **No platform API keys or personal browser cookie scraping** — strictly public viewer endpoints plus optional isolated burner-login Edge fallback.
