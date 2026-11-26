import type { FaultKind } from "../types";

export interface MetricGlossaryEntry {
  title: string;
  summary: string;
  guide?: string;
}

export const ANALYTICS_GLOSSARY = {
  ccv: {
    title: "Live Viewers (CCV)",
    summary:
      "Concurrent Viewers — how many people are watching the livestream right now.",
    guide: "Updates live every ~10s and rolls into 1-minute accounting charts.",
  },
  pcv: {
    title: "Peak Concurrent Viewers (PCV)",
    summary:
      "The highest number of people watching at the exact same time during this show.",
    guide: "Combined PCV adds simultaneous channels in the same 1-minute interval.",
  },
  acv: {
    title: "Average Concurrent Viewers (ACV)",
    summary:
      "Average live audience size per active broadcast minute (offline/N/A minutes are excluded).",
  },
  hw: {
    title: "Hours Watched (HW)",
    summary:
      "Total viewing hours accumulated across your audience (viewers × minutes ÷ 60).",
  },
  gridBucket: {
    title: "60s Accounting Grid",
    summary:
      "Normalized 1-minute time slots displayed in your device's local timezone so YouTube, Twitch, Facebook, and TikTok viewer totals align on one shared timeline.",
  },
  coverage: {
    title: "Data Coverage %",
    summary:
      "Percentage of broadcast time where viewer stats were successfully retrieved from the platform.",
    guide: "100% means continuous stats; lower % means outages or hidden metrics.",
  },
  metricStatus: {
    title: "Metric Availability Status",
    summary:
      "VALUE = live number collected. NA = platform doesn't expose this stat. UNAVAILABLE = stream offline or blocked.",
  },
  metricSource: {
    title: "Data Provenance (AUTO vs STUDIO)",
    summary:
      "AUTO means fetched automatically from platform APIs/scrapers. MANUAL_STUDIO means adjusted by an operator.",
  },
  chatRate: {
    title: "Live Chat Activity (Chat/m)",
    summary:
      "Incoming chat messages per minute. Sudden drops or keyword bursts help spot stream outages early.",
  },
  views: {
    title: "Total Views / Plays",
    summary:
      "Cumulative stream views/starts reported by the platform since the broadcast began.",
  },
  likes: {
    title: "Stream Likes",
    summary: "Total viewer likes or thumbs-up reactions reported on the broadcast.",
  },
  shares: {
    title: "Shares / Restreams",
    summary: "How many times viewers shared the live broadcast (where supported).",
  },
  gifts: {
    title: "Viewer Gifts",
    summary: "Total virtual gifts sent by viewers during the live session (TikTok).",
  },
  uptime: {
    title: "Healthy Uptime %",
    summary:
      "Percentage of monitored time this feed stayed healthy without sustained freezes, dead air, or drops.",
  },
  tier: {
    title: "Monitoring Quality Tier",
    summary:
      "MASTER pulls full source video/audio, anchors the group's Wrong-Bus LUFS reference, and is never auto-shed. SIMULCAST decodes <=480p video + audio and may auto-shed to LIGHT under heavy GPU load. LIGHT monitors audio DSP, stalls, chat, and viewers with 0% GPU video decode.",
    guide:
      "Use 1 MASTER feed per Stream Group (your main broadcast output), SIMULCAST for platform backups, and LIGHT for extra mirrors or battery/low-GPU machines.",
  },
  avgLufs: {
    title: "Average Loudness (Avg LUFS)",
    summary:
      "Perceived overall audio volume measured like human hearing (EBU R128) since stream start or last reset. Triggers the Loudness Out of Spec alert if it stays outside your Target ± tolerance for 10s.",
    guide: "Streams are typically mastered around -14 LUFS (YouTube/Twitch) or -23 LUFS (TV broadcast).",
  },
  shortLufs: {
    title: "Short-Term Loudness (3s LUFS)",
    summary:
      "Rolling 3-second instantaneous loudness window. Used to spot brief dynamics and compare Wrong-Bus correlation across feeds.",
  },
  lufsTarget: {
    title: "Target Loudness Rule",
    summary:
      "Your chosen loudness goal in LUFS plus allowed wiggle room (± LU). Click to customize per channel or use Global/Platform defaults.",
    guide: "Example: Target -14.0 ±2.5 warns if loudness stays outside -16.5 to -11.5 LUFS.",
  },
  truePeak: {
    title: "True Peak (dBTP)",
    summary:
      "The loudest brief audio spike between digital samples. Above -1.0 dBTP risks harsh crackling/clipping.",
    guide: "Safe range: -12 dBTP to -1.0 dBTP. Red at 0.0 dBTP means clipping.",
  },
  vuBars: {
    title: "Left / Right Audio Levels (L / R)",
    summary:
      "Instant volume bars for Left and Right stereo speakers. Shows if audio is quiet, balanced stereo, or peaking.",
  },
  phaseCorr: {
    title: "Stereo Phase Alignment (Φ)",
    summary:
      "Shows if Left and Right speakers reinforce or cancel each other out (from +1.0 mono/clean to -1.0 inverted).",
    guide: "Good stereo: +0.20 to +1.00. Negative (< -0.50) means audio can vanish on phones/mono speakers.",
  },
  mainsHum: {
    title: "Electrical Mains Hum (50/60 Hz)",
    summary:
      "Continuous 50/60 Hz AC power buzz or ground-loop tone lasting 2+ seconds — typically from a loose cable or power hum.",
  },
  hwDecode: {
    title: "Decoder Mode (HW vs SW)",
    summary:
      "HW = decoded on your graphics card (GPU) to keep CPU fast. SW = decoded on main CPU processor.",
  },
  resolutionFps: {
    title: "Video Resolution & Frame Rate",
    summary:
      "Source picture height and smoothness (e.g. 1080p60 = HD at 60 frames per second) plus stream bitrate.",
  },
  avSync: {
    title: "Source Audio/Video Sync (A/V ms)",
    summary:
      "Timing gap between sound and picture inside the incoming stream itself (before preview playback).",
    guide: "Within ±45 ms is imperceptible; beyond ±150 ms causes visible lip-sync delay.",
  },
  decodeMs: {
    title: "Frame Decode Time (Dec)",
    summary:
      "How many milliseconds your computer needs to decode one video frame.",
    guide: "Keep under 16.6 ms for 60fps feeds (or under 33 ms for 30fps).",
  },
  cdnRatio: {
    title: "Download Speed Health (CDN %)",
    summary:
      "How long fetching each video segment takes compared to its duration. Lower % means plenty of download headroom.",
    guide: "Healthy < 65%. Above 85–100% warns buffering/stuttering is likely.",
  },
  avgQp: {
    title: "Compression Level (Avg QP)",
    summary:
      "Estimated video compression factor. Lower numbers mean crisp detail; high numbers indicate heavy compression blur.",
    guide: "Crisp: 12–26. Heavy compression: 35–51.",
  },
  motionPct: {
    title: "Frame Motion %",
    summary:
      "How much of the picture visibly changed between analyzed frames. Sticking at 0% indicates a frozen screen.",
  },
  blockiness: {
    title: "Compression Blockiness",
    summary:
      "Measures blocky square artifacts along 8×8 pixel grids caused by dropped data or starved bitrate.",
  },
  lumaMean: {
    title: "Average Brightness (Luma Mean)",
    summary:
      "Average screen brightness on a 16 (pure black) to 235 (pure white) broadcast scale.",
  },
  lumaVar: {
    title: "Contrast Spread (Luma Var)",
    summary:
      "How much light-to-dark detail is on screen. Very low (< 5) indicates a solid black, solid color, or blank screen.",
  },
  frameMad: {
    title: "Pixel Change Score (Frame MAD)",
    summary:
      "Average brightness difference per pixel between consecutive frames. Detects frozen feeds.",
  },
  resolver: {
    title: "Stream Link Resolver",
    summary:
      "Which engine extracted the live video URL (e.g., yt-dlp, Streamlink, headless Edge CDP, direct link, or simulator).",
  },
  vram: {
    title: "Graphics Memory (VRAM)",
    summary:
      "GPU memory used for hardware video decoding. Above 80–88%, PGMWatch sheds backup feeds to Audio-Only to stay smooth.",
  },
  ram: {
    title: "System Memory (RAM)",
    summary:
      "Total PC memory in use across OS, stream workers, and browser video buffers.",
  },
  cpu: {
    title: "Processor Load (CPU)",
    summary:
      "Host CPU utilization across video/audio decoders and FFmpeg preview relays.",
  },
  wan: {
    title: "Internet Download Usage (WAN)",
    summary:
      "Total live stream download speed compared to your configured bandwidth cap.",
  },
  maxDecodeLatency: {
    title: "Decode Budget Used (Latency %)",
    summary:
      "How close the slowest decoder is to missing its real-time frame deadline (>100% means frames cannot keep up live).",
  },
  governorStage: {
    title: "Auto-Protection State",
    summary:
      "Nominal = normal operation. Stage 1 Throttle = slows thumbnail previews to save PC load. Stage 2 Shed = drops backup feeds to Audio-Only.",
  },
  activeWorkers: {
    title: "Active Channel Workers",
    summary:
      "Number of background channel monitor processes currently decoding and analyzing streams.",
  },
  shedChannels: {
    title: "Shed Channels",
    summary:
      "Backup feeds temporarily downgraded to Light (Audio-Only) mode to protect Master feeds under high PC load.",
  },
  forceClip: {
    title: "Instant Pre-Trigger Clip",
    summary:
      "Saves right away using video already received before you clicked — it does not wait to record 2.5s after the click.",
    guide:
      "Pulls ~2–6s back from the newest keyframe in memory (6s / 8 MB cap) into Clips & Incidents. CLI/fallback mode records 5s forward instead.",
  },
  clipStorage: {
    title: "Replay Clip Storage Limit (Auto-FIFO)",
    summary:
      "MP4 replay clips do NOT fill your disk indefinitely: once stored clips exceed your quota (10 GB default), the oldest files are automatically deleted before new clips save.",
    guide:
      "Use Clear Clips to free space anytime, or adjust the GB limit in Show Setup -> Runtime Settings.",
  },
  segmentMarker: {
    title: "Show Segment Bookmark",
    summary:
      "Marks a named window (such as Keynote, Set 1, Flash Sale, or Game 1) so Broadcast Accounting & Excel break down viewers per segment.",
    guide:
      "Click Start Segment in live studio time; delayed feeds are auto-aligned by their configured delay.",
  },
  holdMode: {
    title: "Hold / Standby Alert Mute",
    summary:
      "Temporarily mutes expected static-screen alerts (Frozen Frame, Silence/Dead Air, and VBR Bitrate Collapse) during breaks, worship slides, or 'Starting Soon' hold screens.",
    guide:
      "Keeps a ~30s flush grace tail after turning Hold OFF so normal encoder/CDN lag doesn't false-trip, while delayed feeds auto-shift by their Delay (s).",
  },
  channelDelay: {
    title: "Intentional Client Delay (Seconds)",
    summary:
      "Leave at 0 for normal ~5–25s YouTube/HLS encoding latency (handled automatically by +/-30s audio lag matching and Hold Mode's 30s grace tail).",
    guide:
      "Only enter 300 / 600 / 900s for intentional 5–15 min tournament or client broadcast delays so Segments, Hold Mode, and group audio stay aligned.",
  },
  excludeStats: {
    title: "Exclude from Viewer Accounting",
    summary:
      "Monitors A/V health, PFL, clips, and AI normally without counting internal/unlisted QC viewers in Combined CCV, PCV, or Excel totals.",
  },
  liveStreamsOnly: {
    title: "Live Streams Only (Reject VOD / Replays)",
    summary:
      "Marks ended broadcast archives, standard VOD videos, and upcoming premieres Offline instead of monitoring past recordings.",
    guide:
      "Checked by default for live broadcast feeds. Uncheck on a channel only if you want to test with a regular uploaded/VOD video link. Simulator (sim://) and RTMP/SRT always stay exempt.",
  },
  chatComplaintKeywords: {
    title: "Chat Comment Problem Keywords",
    summary:
      "Comma-separated words or phrases PGMWatch watches for in live viewer chat comments (for example: no sound, muted, bisu, lag, freeze, stutter).",
    guide:
      "If enough unique viewers type these problem words inside the configured Chat Spike window (see Runtime Settings), PGMWatch raises a Chat Complaint Spike alert. Leave blank to turn off chat keyword detection for this group.",
  },
} as const satisfies Record<string, MetricGlossaryEntry>;

export const RUNTIME_SETTINGS_GLOSSARY = {
  sectionMedia: {
    title: "Media Decode & GPU Pinning",
    summary:
      "Controls which graphics card decodes video frames, how often wall thumbnails refresh, and how much disk space is reserved for instant replay clips.",
    guide:
      "Keep defaults on single-GPU PCs. Only pin GPU LUIDs when balancing load across an integrated GPU (iGPU) and a dedicated NVIDIA/AMD card.",
  },
  gpuMode: {
    title: "GPU Decode Mode",
    summary:
      "Chooses whether video frames are decoded on your graphics hardware or CPU.",
    guide:
      "auto (recommended) prefers iGPU/NPU; d3d11va forces Windows Direct3D11 hardware decode; cpu uses software decode (high CPU load, helpful if GPU drivers misbehave).",
  },
  masterGpuLuid: {
    title: "Master Tier GPU LUID",
    summary:
      "Pins high-priority Master / Main streams to a specific Windows graphics adapter.",
    guide:
      "Leave on Auto unless you want Master feeds dedicated to your stronger GPU while backup feeds run on another adapter.",
  },
  simulcastGpuLuid: {
    title: "Simulcast Tier GPU LUID",
    summary:
      "Pins backup / simulcast streams to a specific Windows graphics adapter.",
    guide:
      "On dual-GPU systems, pinning backups to your iGPU keeps your dedicated GPU free for Master feeds.",
  },
  extraHwFrames: {
    title: "Extra Hardware Decode Frames (1–64)",
    summary:
      "Number of extra GPU frame buffers reserved per video decoder to smooth out frame timing jitter.",
    guide:
      "Default ~8–16 works well for most cards. Increase slightly if hardware decode logs surface-starvation drops; lower it if VRAM fills up with many channels.",
  },
  thumbnailFps: {
    title: "Live Wall Thumbnail FPS (0.2–15)",
    summary:
      "How many video frames per second each monitored stream decodes for wall preview tiles and visual black/freeze checks.",
    guide:
      "1–2 FPS is usually ideal for monitoring. Higher FPS (e.g. 5–15) makes wall tiles look smoother but uses noticeably more CPU/GPU per stream.",
  },
  clipQuotaGb: {
    title: "Replay Clip Storage Limit (0.5–500 GB)",
    summary:
      "Maximum total disk space allowed for saved MP4 incident and manual pre-trigger clips.",
    guide:
      "Uses an automatic FIFO ring: once total clips pass this limit, the oldest MP4 files are deleted before saving new ones so your disk never fills up.",
  },
  sectionExtraction: {
    title: "Extraction, PO-Token & Outbound Network",
    summary:
      "Controls how PGMWatch resolves livestream URLs from YouTube, Twitch, Facebook, and TikTok, plus browser fallback and network interface binding.",
  },
  authMode: {
    title: "Stream Auth Mode",
    summary:
      "Controls whether URL extraction runs anonymously or uses signed-in browser cookies.",
    guide:
      "Use anonymous (default) for public and unlisted links without account risk. Switch to fallback_auth only if you need to monitor member-only or age-restricted streams.",
  },
  maxConcurrentExtractors: {
    title: "Max Concurrent Extractors (1–32)",
    summary:
      "Maximum number of feeds resolving stream manifests/URLs at the exact same moment.",
    guide:
      "Default 4–8 prevents CPU/network spikes when launching many channels at once. Lower it if opening shows triggers rate limits.",
  },
  bgutilPotPort: {
    title: "bgutil PO-Token Local Port",
    summary:
      "Local loopback port where PGMWatch communicates with the built-in PO-Token helper for YouTube anti-bot checks.",
    guide:
      "Default is 4416. Only change this if another program on your PC already uses port 4416.",
  },
  outboundBindIp: {
    title: "Outbound Bind IP (Multi-NIC)",
    summary:
      "Optional local network adapter IPv4 address used for outgoing stream traffic instead of Windows' default gateway.",
    guide:
      "Useful in broadcast trucks or studios with separate studio vs public WAN NICs. Leave blank on normal PCs. Restart required after changing.",
  },
  msedgePath: {
    title: "Browser Path Override (Fallback)",
    summary:
      "Optional path to a Chromium browser executable for browser-assisted stream extraction when standard extraction fails.",
    guide:
      "Leave blank for automatic detection (checks portable bin/browser/ first, then Edge, Chrome, and Brave installed on Windows).",
  },
  enableBgutilPot: {
    title: "Enable Built-in PO-Token Provider",
    summary:
      "Runs PGMWatch's local PO-Token helper automatically to reduce YouTube 'Sign in to confirm you are not a bot' blocks.",
    guide:
      "Keep checked unless you intentionally run a separate external PO-Token service on the configured port.",
  },
  sectionGovernor: {
    title: "Resource Governor & Alert Tuning",
    summary:
      "Protects Master streams when bandwidth, RAM, or VRAM gets tight, and sets how strict audio loudness, alert recovery, and live-chat spike alarms are.",
  },
  wanCapMbps: {
    title: "WAN Bandwidth Cap (Mbps)",
    summary:
      "Total internet download speed (in Mbps) that PGMWatch budgets across all active feeds.",
    guide:
      "Used alongside WAN Admit Max % so backups step down to audio-only before saturating your internet connection.",
  },
  wanAdmitMaxPct: {
    title: "WAN Admit Max %",
    summary:
      "Maximum percentage of WAN Cap allowed before new video decodes pause or backup feeds shed to Light (Audio-Only) mode.",
    guide:
      "For example, 80% on a 100 Mbps cap begins protecting Master streams around 80 Mbps.",
  },
  vramAdmitMaxPct: {
    title: "VRAM Admit Max %",
    summary:
      "Maximum GPU video-memory percentage before PGMWatch stops adding hardware video decoders or degrades backup feeds.",
    guide:
      "Keeps Windows desktop and Master decoders stable instead of crashing on out-of-VRAM.",
  },
  ramAdmitMaxPct: {
    title: "System RAM Admit Max %",
    summary:
      "Maximum PC memory percentage before the Resource Governor sheds lower-priority video decode.",
    guide:
      "Default ~80–85% leaves headroom for the OS and audio monitoring.",
  },
  workerMemoryLimitMb: {
    title: "Per-Worker Memory Limit (MB)",
    summary:
      "RAM ceiling for each single channel's worker process.",
    guide:
      "If one channel's FFmpeg/decoder process exceeds this MB limit (e.g. driver memory leak), only that channel restarts cleanly without interrupting the wall.",
  },
  recoveryHysteresisSecs: {
    title: "Fault Recovery Hysteresis (Seconds)",
    summary:
      "How many clean, fault-free seconds a stream must maintain before a red/amber alert automatically clears back to OK.",
    guide:
      "Prevents rapid alert flickering on borderline signals (e.g. brief audio dips or static slides).",
  },
  criticalEscalationSecs: {
    title: "Critical Escalation Delay (Seconds)",
    summary:
      "How long an active issue must persist continuously before escalating from a short warning into a critical incident.",
    guide:
      "Short encoder hiccups shorter than this stay minor; sustained outages escalate automatically.",
  },
  chatSpikeWindow: {
    title: "Chat Complaint Spike (Viewers / Window Sec)",
    summary:
      "Raises a Chat Complaint Spike alert when at least N unique viewers (left box) type a group's Chat Problem Keyword within M seconds (right box).",
    guide:
      "Example: 5 viewers in 30s ignores single trolls, while catching sudden audience-wide 'no sound' or 'lag' reports.",
  },
  customTargetLufs: {
    title: "Default Loudness Target (LUFS)",
    summary:
      "Expected broadcast audio loudness level when left in automatic platform mode (blank) or forced to a custom studio target.",
    guide:
      "Leave blank for automatic platform standards (-14 LUFS YouTube/Twitch, -13 Facebook, -11 TikTok), or enter a fixed value like -16 or -23 if required by client delivery specs.",
  },
  lufsTolerance: {
    title: "Loudness Tolerance Window (+/- LU)",
    summary:
      "Allowed margin above or below the LUFS Target before triggering an Over-Loudness or Quiet Audio warning.",
    guide:
      "Example: target -14 LUFS with +/- 2.5 LU alerts outside -16.5 to -11.5 LUFS. Raise slightly for wide dynamic music/talk shows.",
  },
} as const satisfies Record<string, MetricGlossaryEntry>;

export const FAULT_GLOSSARY: Record<FaultKind, MetricGlossaryEntry> = {
  STREAM_STALL: {
    title: "Stream Stall",
    summary: "No video or audio packets have arrived from the broadcast server recently.",
    guide: "Check encoder internet connection or click Retry.",
  },
  CDN_STARVATION: {
    title: "CDN Starvation",
    summary: "Video chunks are downloading slower than real-time playback speed, risking buffering.",
  },
  BITRATE_COLLAPSE: {
    title: "Bitrate Collapse",
    summary:
      "Stream bitrate stayed below 35% of its rolling baseline for 6+ seconds during active motion or quality degradation (clean static VBR/CRF slides are ignored).",
  },
  PTS_DRIFT: {
    title: "PTS Discontinuity",
    summary: "Media timestamps jumped backwards or skipped ahead abnormally (often an encoder reset or dropped segment).",
  },
  AV_SYNC_DRIFT: {
    title: "A/V Sync Drift",
    summary: "Source audio and video timestamps are separated by more than ±150 ms for 3+ seconds.",
  },
  CORRUPT_DECODE: {
    title: "Corrupt Decode",
    summary: "Corrupted compressed frames arrived and caused decoder errors or glitches.",
  },
  LUFS_OUT_OF_RANGE: {
    title: "Loudness Out of Spec",
    summary: "Average LUFS (Avg LUFS) stayed noticeably quieter or louder than your chosen LUFS Target ± tolerance for 10+ seconds.",
  },
  AUDIO_CLIPPING: {
    title: "True Peak Clipping",
    summary: "Audio volume hit maximum 0 dBFS continuously, causing harsh distortion.",
  },
  AUDIO_MICRO_SILENCE: {
    title: "Audio Micro-Silence",
    summary: "Brief unwanted audio dropout/mute lasting ~0.4–2 seconds.",
  },
  AUDIO_DEAD_AIR: {
    title: "Audio Dead Air",
    summary: "Complete silence on the stream for 5+ continuous seconds.",
  },
  AUDIO_MISSING_CHANNEL: {
    title: "Missing L or R Channel",
    summary: "One stereo side (Left or Right) dropped silent while the other side kept playing.",
  },
  AUDIO_PHASE_INVERSION: {
    title: "Audio Phase Inversion",
    summary: "Left and Right channels are wired/encoded upside-down (`Φ < -0.5`), cancelling sound on mono speakers.",
  },
  AUDIO_MAINS_HUM: {
    title: "Mains Hum / Feedback",
    summary: "Persistent 50 Hz or 60 Hz electrical hum detected continuously for 2+ seconds.",
  },
  VIDEO_BLACK: {
    title: "Black Video Frame",
    summary: "Picture went completely dark/black for sustained duration.",
  },
  VIDEO_BLINK: {
    title: "Video Blink / Flash",
    summary: "Rapid black-frame flicker detected in incoming video.",
  },
  VIDEO_FROZEN: {
    title: "Frozen Picture",
    summary: "Video picture stopped updating/moving for 5+ seconds even if packets continue.",
  },
  VIDEO_MACROBLOCKING: {
    title: "Heavy Pixelation / Macroblocking",
    summary: "Severe blocky compression artifacts detected across frames.",
  },
  WRONG_BUS_AUDIO: {
    title: "Wrong-Bus Audio",
    summary: "Backup/simulcast audio loudness pattern doesn't match the group's Master feed (possible wrong audio routing).",
  },
  CHAT_COMPLAINT_SPIKE: {
    title: "Viewer Chat Complaint Spike",
    summary: "Multiple unique viewers in live chat simultaneously reported lag, mute, or freeze keywords.",
  },
  WORKER_GPU_FAULT: {
    title: "GPU Hardware Decode Fault",
    summary: "Hardware graphics decoder failed and automatically fell back to CPU software decode.",
  },
  OPERATOR_BOOKMARK: {
    title: "Operator Bookmark / Cue",
    summary: "Manual event note or instant replay clip saved by the operator (not a broadcast fault).",
  },
};
