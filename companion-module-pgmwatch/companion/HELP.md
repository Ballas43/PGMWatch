# PGMWatch — Bitfocus Companion Module

Connects Bitfocus Companion to **PGMWatch** over its local WebSocket API (`ws://<host>:<port>/ws`, default `127.0.0.1:8787`) for hardware tally, live 48 kHz VU bars, Hold Mode, fault acknowledgment, and 5-second evidence clip triggers.

## Setup

1. Start `PGMWatch.exe`.
2. In Bitfocus Companion, open **Settings -> Developer modules path** and point it to the parent folder containing `companion-module-pgmwatch` (or copy the `companion-module-pgmwatch` folder directly into your Companion Developer Modules folder).
3. Add a **PGMWatch** connection:
   - **PGMWatch Host**: `127.0.0.1` (or the LAN IP of the PGMWatch host PC if `[server] bind_host` allows LAN)
   - **Port**: `8787`

## Auto-Generated Presets

Once connected, presets and dropdown options automatically populate from your active PGMWatch show:

- **Overview — Language Groups**: Shows group code + live viewers (`$(pgmwatch:group_<id>_ccv)`), rolls up worst tally color, and toggles **Group Hold Mode** on press.
- **Overview — Controls**: **Global Hold Mode** toggle, **Reset All Avg Loudness**, **Start Segment**, and **End Segment**.
- **Group Detail — `<group>`**: Per-channel **VU / Listen (Text)** (simple text meter bar + tally background, works on all Companion versions), **VU / Listen (Image)** (optional `png64` vertical bar graphic without duplicate text bars), **Ack**, **5s Clip**, and **Reset Loudness** buttons.

## Live Audio Meters

- **Simple Text (Default, Universal):** The **VU/Listen (Text)** preset renders `$(pgmwatch:ch_<id>_vu_bar)` + LUFS with `channel_tally` background colors and works cleanly in Companion v3, v4, and v5.
- **Image Sequence (`png64`):** The separate **VU/Listen (Image)** preset uses the `channel_vu_png` bitmap bar without overlaying text bars on top.
- **Companion v5 Native Gauges:** Bind a native **Gauge / Progress Bar** layer (`0–100`) to `$(pgmwatch:ch_<id>_left_pct)` and `$(pgmwatch:ch_<id>_right_pct)` (or `-60–0` dBFS via `ch_<id>_left_db` / `ch_<id>_right_db`).
