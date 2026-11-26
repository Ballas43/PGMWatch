# PGMWatch — Bitfocus Companion Module

Connects Bitfocus Companion to **PGMWatch** over its local WebSocket API (`ws://<host>:<port>/ws`, default `127.0.0.1:8787`) for hardware tally, live 48 kHz VU bars, Hold Mode, fault acknowledgment, and 5-second evidence clip triggers.

## Setup

1. Start `PGMWatch.exe`.
2. In Bitfocus Companion, open **Settings -> Developer modules path** and point it to the parent folder containing `companion-module` (or copy the `companion-module` folder directly into your Companion Developer Modules folder).
3. Add a **PGMWatch** connection:
   - **PGMWatch Host**: `127.0.0.1` (or the LAN IP of the PGMWatch host PC if `[server] bind_host` allows LAN)
   - **Port**: `8787`

## Auto-Generated Presets

Once connected, presets and dropdown options automatically populate from your active PGMWatch show:

- **Overview — Language Groups**: Shows group code + live viewers (`$(pgmwatch:group_<id>_ccv)`), rolls up worst tally color, and toggles **Group Hold Mode** on press.
- **Overview — Controls**: **Global Hold Mode** toggle, **Reset All Avg Loudness**, **Start Segment**, and **End Segment**.
- **Group Detail — `<group>`**: Per-channel **VU / Listen (PFL)** (live meter + LUFS text; pressing toggles Listen), **Ack**, **5s Clip**, and **Reset Loudness** buttons.

## Live Audio Meters in Companion v5

Companion v5 replaced legacy bitmap/pixel-buffer (`png64` / `imageBuffer`) feedbacks with native **Gauge / Progress Bar** layers:

- Every **VU/Listen** preset includes a live 10 Hz text bar (`$(pgmwatch:ch_<id>_vu_bar)`) that animates immediately in Companion v5.
- For smooth vector meters in Companion v5, add a **Gauge** element to any button and bind its value (`0–100`) to `$(pgmwatch:ch_<id>_left_pct)` and `$(pgmwatch:ch_<id>_right_pct)` (or `-60–0` dBFS via `ch_<id>_left_db` / `ch_<id>_right_db`).
