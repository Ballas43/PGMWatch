export type Platform = "youtube" | "twitch" | "facebook" | "tiktok" | "bigo" | "custom";
export type FeedTier = "MASTER" | "SIMULCAST" | "LIGHT";
export type AlertState =
  | "OFFLINE"
  | "REACQUIRING"
  | "AD_BREAK"
  | "EXTRACTOR_ERR"
  | "LOGIN_GATED"
  | "HEALTHY"
  | "WARNING"
  | "CRITICAL"
  | "ACKNOWLEDGED";

export type MetricStatus = "VALUE" | "NA" | "UNAVAILABLE";
export type MetricSource = "AUTO_VIEWER" | "MANUAL_STUDIO";

export type FaultKind =
  | "STREAM_STALL"
  | "CDN_STARVATION"
  | "BITRATE_COLLAPSE"
  | "PTS_DRIFT"
  | "AV_SYNC_DRIFT"
  | "CORRUPT_DECODE"
  | "LUFS_OUT_OF_RANGE"
  | "AUDIO_CLIPPING"
  | "AUDIO_MICRO_SILENCE"
  | "AUDIO_DEAD_AIR"
  | "AUDIO_MISSING_CHANNEL"
  | "AUDIO_PHASE_INVERSION"
  | "AUDIO_MAINS_HUM"
  | "VIDEO_BLACK"
  | "VIDEO_BLINK"
  | "VIDEO_FROZEN"
  | "VIDEO_MACROBLOCKING"
  | "WRONG_BUS_AUDIO"
  | "CHAT_COMPLAINT_SPIKE"
  | "WORKER_GPU_FAULT"
  | "OPERATOR_BOOKMARK";

export interface TriStateMetric {
  status: MetricStatus;
  value: number | null;
  coverage_pct: number;
  source: MetricSource;
  note: string | null;
}

export interface ChannelConfig {
  id: string;
  group_id: string;
  name: string;
  url: string;
  platform: Platform;
  tier: FeedTier;
  priority: number;
  orientation_vertical: boolean;
  enabled: boolean;
  target_lufs?: number | null;
  lufs_tolerance?: number | null;
  broadcast_delay_secs?: number;
  exclude_from_accounting?: boolean;
  live_streams_only?: boolean;
}

export interface LanguageGroupConfig {
  id: string;
  name: string;
  language_code: string;
  intermission_active: boolean;
  complaint_keywords: string[];
}

export interface ShowPreset {
  show_id: string;
  title: string;
  created_at: string;
  scheduled_start_at?: string | null;
  scheduled_end_at?: string | null;
  groups: LanguageGroupConfig[];
  channels: ChannelConfig[];
}

export interface ShowPresetSummary {
  show_id: string;
  title: string;
  created_at: string;
  updated_at: string;
  group_count: number;
  channel_count: number;
  is_active: boolean;
}

export interface ShowPresetListResponse {
  active_show_id: string;
  active_segment_label: string | null;
  shows: ShowPresetSummary[];
}

export interface AudioTelemetry {
  integrated_lufs: number;
  short_term_lufs: number;
  true_peak_dbtp: number;
  left_rms_dbfs: number;
  right_rms_dbfs: number;
  phase_correlation: number;
  hum_peak_db: number;
  hum_freq_hz: number | null;
}

export interface VideoTelemetry {
  width: number;
  height: number;
  fps: number;
  bitrate_kbps: number;
  avg_qp: number;
  luma_mean: number;
  frame_mad: number;
  luma_variance?: number;
  changed_pct?: number;
  blockiness_score?: number;
  decode_ms: number;
  av_sync_ms: number;
  cdn_ratio: number;
  hw_accelerated: boolean;
}

export interface ChannelRuntimeSnapshot {
  config: ChannelConfig;
  effective_tier: FeedTier;
  video_shed_resource: boolean;
  alert_state: AlertState;
  active_faults: FaultKind[];
  audio: AudioTelemetry;
  video: VideoTelemetry;
  thumbnail_b64: string | null;
  thumbnail_updated_at: string | null;
  ccv: TriStateMetric;
  total_views: TriStateMetric;
  likes: TriStateMetric;
  chat_rate_per_min: TriStateMetric;
  shares: TriStateMetric;
  gifts_count: TriStateMetric;
  last_status_reason: string | null;
  pfl_active: boolean;
  uptime_pct: number;
  resolved_via: string | null;
  effective_target_lufs?: number;
  effective_lufs_tolerance?: number;
  updated_at: string;
}

export interface GovernorTelemetry {
  gpu_adapter_name: string;
  vram_used_mb: number;
  vram_budget_mb: number;
  vram_pct: number;
  ram_used_mb: number;
  ram_total_mb: number;
  ram_pct: number;
  cpu_pct: number;
  wan_rx_mbps: number;
  wan_cap_mbps: number;
  max_decode_latency_pct: number;
  on_battery: boolean;
  degradation_stage: number;
  active_workers: number;
  shed_channels: number;
  updated_at: string;
}

export interface IncidentRecord {
  id: string;
  channel_id: string;
  channel_name: string;
  group_id: string;
  fault: FaultKind;
  severity: AlertState;
  started_at: string;
  resolved_at: string | null;
  duration_secs: number;
  clip_filename: string | null;
  acknowledged: boolean;
  detail: string;
}

export interface ClipStorageStats {
  clip_count: number;
  total_bytes: number;
  quota_gb: number;
}

export interface MatchMarker {
  id: string;
  show_id: string;
  label: string;
  started_at: string;
  ended_at: string | null;
}

export interface DashboardSnapshot {
  show: ShowPreset;
  governor: GovernorTelemetry;
  global_intermission: boolean;
  active_match: MatchMarker | null;
  matches: MatchMarker[];
  channels: ChannelRuntimeSnapshot[];
  incidents: IncidentRecord[];
  combined_ccv: number;
  combined_pcv: number;
  hours_watched: number;
  active_pfl_channel_id: string | null;
  server_time: string;
}

export interface GridPoint {
  bucket_utc: string;
  combined_ccv: number;
  per_group_ccv: Record<string, number>;
  per_channel_ccv: Record<string, number>;
}

export interface AccountingResponse {
  summary: {
    total_minutes: number;
    combined_pcv: number;
    combined_acv: number;
    hours_watched: number;
    group_pcv: Record<string, number>;
    group_acv: Record<string, number>;
    group_hours_watched: Record<string, number>;
    timeline: GridPoint[];
  };
  matches: {
    match_marker: MatchMarker;
    duration_minutes: number;
    pcv: number;
    acv: number;
    hours_watched: number;
  }[];
  overrides: {
    id: string;
    channel_id: string;
    metric_name: string;
    bucket_utc: string | null;
    value: number;
    operator_note: string;
    updated_at: string;
  }[];
}

export interface ServerConfig {
  bind_host: string;
  port: number;
  open_browser_on_start: boolean;
  outbound_bind_ip: string;
}

export interface MediaConfig {
  gpu_mode: string;
  master_gpu_luid: string;
  simulcast_gpu_luid: string;
  extra_hw_frames: number;
  thumbnail_fps: number;
  clip_quota_gb: number;
}

export interface GovernorConfig {
  vram_admit_max_pct: number;
  ram_admit_max_pct: number;
  wan_cap_mbps: number;
  wan_admit_max_pct: number;
  vram_stage1_pct: number;
  vram_stage2_pct: number;
  decode_latency_stage1_pct: number;
  decode_latency_stage2_pct: number;
  worker_memory_limit_mb: number;
}

export interface ExtractionConfig {
  auth_mode: string;
  max_concurrent_extractors: number;
  enable_bgutil_pot: boolean;
  bgutil_pot_port: number;
  msedge_path: string;
}

export interface AlertsConfig {
  recovery_hysteresis_secs: number;
  critical_escalation_secs: number;
  clip_cooldown_secs: number;
  chat_spike_unique_viewers: number;
  chat_spike_window_secs: number;
  custom_target_lufs: number | null;
  lufs_tolerance: number;
}

export interface StorageConfig {
  data_dir: string;
  db_batch_interval_ms: number;
}

export interface PgmWatchConfig {
  server: ServerConfig;
  media: MediaConfig;
  governor: GovernorConfig;
  extraction: ExtractionConfig;
  alerts: AlertsConfig;
  storage: StorageConfig;
}

export interface DxgiAdapterInfo {
  adapter_index: number;
  name: string;
  luid_hex: string;
  vram_used_mb: number;
  vram_budget_mb: number;
}

export interface RuntimeConfigResponse {
  config: PgmWatchConfig;
  adapters: DxgiAdapterInfo[];
}

export interface SaveConfigResponse {
  ok: boolean;
  config: PgmWatchConfig;
  restart_required: boolean;
}
