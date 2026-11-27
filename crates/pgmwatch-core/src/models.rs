use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Youtube,
    Twitch,
    Facebook,
    Tiktok,
    Bigo,
    Custom,
}

impl Platform {
    pub fn display_name(&self) -> &'static str {
        match self {
            Platform::Youtube => "YouTube Live",
            Platform::Twitch => "Twitch",
            Platform::Facebook => "Facebook Live",
            Platform::Tiktok => "TikTok LIVE",
            Platform::Bigo => "Bigo Live",
            Platform::Custom => "Custom Stream",
        }
    }

    pub fn target_lufs(&self) -> f32 {
        match self {
            Platform::Youtube | Platform::Twitch | Platform::Custom => -14.0,
            Platform::Facebook => -13.0,
            Platform::Tiktok | Platform::Bigo => -11.0,
        }
    }

    pub fn lufs_tolerance(&self) -> f32 {
        2.5
    }

    pub fn from_url(url: &str) -> Self {
        let lower = url.to_ascii_lowercase();
        if lower.contains("youtube.com") || lower.contains("youtu.be") {
            Platform::Youtube
        } else if lower.contains("twitch.tv") {
            Platform::Twitch
        } else if lower.contains("facebook.com") || lower.contains("fb.watch") {
            Platform::Facebook
        } else if lower.contains("tiktok.com") {
            Platform::Tiktok
        } else if lower.contains("bigo.tv") {
            Platform::Bigo
        } else {
            Platform::Custom
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FeedTier {
    Master,
    Simulcast,
    Light,
}

impl FeedTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedTier::Master => "MASTER",
            FeedTier::Simulcast => "SIMULCAST",
            FeedTier::Light => "LIGHT",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AlertState {
    Offline,
    Reacquiring,
    #[serde(alias = "ADBREAK")]
    AdBreak,
    #[serde(alias = "EXTRACTORERR")]
    ExtractorErr,
    #[serde(alias = "LOGINGATED")]
    LoginGated,
    Healthy,
    Warning,
    Critical,
    Acknowledged,
}

impl AlertState {
    pub fn severity_rank(&self) -> u8 {
        match self {
            AlertState::Critical => 8,
            AlertState::Warning => 7,
            AlertState::Acknowledged => 6,
            AlertState::LoginGated => 5,
            AlertState::ExtractorErr => 4,
            AlertState::Reacquiring => 3,
            AlertState::AdBreak => 2,
            AlertState::Offline => 1,
            AlertState::Healthy => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FaultKind {
    StreamStall,
    CdnStarvation,
    BitrateCollapse,
    PtsDrift,
    AvSyncDrift,
    CorruptDecode,
    LufsOutOfRange,
    AudioClipping,
    AudioMicroSilence,
    AudioDeadAir,
    AudioMissingChannel,
    AudioPhaseInversion,
    AudioMainsHum,
    VideoBlack,
    VideoBlink,
    VideoFrozen,
    VideoMacroblocking,
    WrongBusAudio,
    ChatComplaintSpike,
    WorkerGpuFault,
    OperatorBookmark,
}

impl FaultKind {
    pub fn suppressed_in_intermission(&self) -> bool {
        matches!(
            self,
            FaultKind::VideoFrozen
                | FaultKind::AudioDeadAir
                | FaultKind::AudioMicroSilence
                | FaultKind::BitrateCollapse
        )
    }

    pub fn label(&self) -> &'static str {
        match self {
            FaultKind::StreamStall => "Stream Stall",
            FaultKind::CdnStarvation => "CDN Starvation",
            FaultKind::BitrateCollapse => "Bitrate Collapse",
            FaultKind::PtsDrift => "PTS Discontinuity",
            FaultKind::AvSyncDrift => "A/V Sync Drift",
            FaultKind::CorruptDecode => "Corrupt Decode",
            FaultKind::LufsOutOfRange => "LUFS Out of Spec",
            FaultKind::AudioClipping => "True Peak Clipping",
            FaultKind::AudioMicroSilence => "Audio Micro-Silence",
            FaultKind::AudioDeadAir => "Audio Dead Air",
            FaultKind::AudioMissingChannel => "Missing L/R Channel",
            FaultKind::AudioPhaseInversion => "Phase Inversion",
            FaultKind::AudioMainsHum => "Mains Hum / Feedback",
            FaultKind::VideoBlack => "Black Frame",
            FaultKind::VideoBlink => "Video Blink",
            FaultKind::VideoFrozen => "Frozen Frame",
            FaultKind::VideoMacroblocking => "Macroblocking",
            FaultKind::WrongBusAudio => "Wrong-Bus Audio",
            FaultKind::ChatComplaintSpike => "Chat Complaint Spike",
            FaultKind::WorkerGpuFault => "GPU Decode Fault",
            FaultKind::OperatorBookmark => "Operator Bookmark",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MetricStatus {
    Value,
    Na,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MetricSource {
    AutoViewer,
    ManualStudio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriStateMetric {
    pub status: MetricStatus,
    pub value: Option<f64>,
    pub coverage_pct: f32,
    pub source: MetricSource,
    pub note: Option<String>,
}

impl TriStateMetric {
    pub fn value(val: f64, coverage_pct: f32) -> Self {
        Self {
            status: MetricStatus::Value,
            value: Some(val),
            coverage_pct,
            source: MetricSource::AutoViewer,
            note: None,
        }
    }

    pub fn na() -> Self {
        Self {
            status: MetricStatus::Na,
            value: None,
            coverage_pct: 100.0,
            source: MetricSource::AutoViewer,
            note: None,
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            status: MetricStatus::Unavailable,
            value: None,
            coverage_pct: 0.0,
            source: MetricSource::AutoViewer,
            note: Some(reason.into()),
        }
    }
}

fn default_live_streams_only() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelConfig {
    pub id: String,
    pub group_id: String,
    pub name: String,
    pub url: String,
    pub platform: Platform,
    pub tier: FeedTier,
    pub priority: u8,
    pub orientation_vertical: bool,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_lufs: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lufs_tolerance: Option<f32>,
    #[serde(default)]
    pub broadcast_delay_secs: u32,
    #[serde(default)]
    pub exclude_from_accounting: bool,
    #[serde(default = "default_live_streams_only")]
    pub live_streams_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageGroupConfig {
    pub id: String,
    pub name: String,
    pub language_code: String,
    #[serde(default)]
    pub intermission_active: bool,
    #[serde(default)]
    pub complaint_keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShowPreset {
    pub show_id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_start_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduled_end_at: Option<DateTime<Utc>>,
    pub groups: Vec<LanguageGroupConfig>,
    pub channels: Vec<ChannelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTelemetry {
    pub integrated_lufs: f32,
    pub short_term_lufs: f32,
    pub true_peak_dbtp: f32,
    pub left_rms_dbfs: f32,
    pub right_rms_dbfs: f32,
    pub phase_correlation: f32,
    pub hum_peak_db: f32,
    pub hum_freq_hz: Option<f32>,
}

impl Default for AudioTelemetry {
    fn default() -> Self {
        Self {
            integrated_lufs: -23.0,
            short_term_lufs: -23.0,
            true_peak_dbtp: -12.0,
            left_rms_dbfs: -24.0,
            right_rms_dbfs: -24.0,
            phase_correlation: 0.92,
            hum_peak_db: 0.0,
            hum_freq_hz: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoTelemetry {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub bitrate_kbps: f32,
    pub avg_qp: f32,
    pub luma_mean: f32,
    pub frame_mad: f32,
    #[serde(default)]
    pub luma_variance: f32,
    #[serde(default)]
    pub changed_pct: f32,
    #[serde(default)]
    pub blockiness_score: f32,
    pub decode_ms: f32,
    pub av_sync_ms: f32,
    pub cdn_ratio: f32,
    pub hw_accelerated: bool,
}

impl Default for VideoTelemetry {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 60.0,
            bitrate_kbps: 6000.0,
            avg_qp: 22.0,
            luma_mean: 96.0,
            frame_mad: 12.0,
            luma_variance: 0.0,
            changed_pct: 0.0,
            blockiness_score: 0.0,
            decode_ms: 2.1,
            av_sync_ms: 0.0,
            cdn_ratio: 0.25,
            hw_accelerated: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelRuntimeSnapshot {
    pub config: ChannelConfig,
    pub effective_tier: FeedTier,
    pub video_shed_resource: bool,
    pub alert_state: AlertState,
    pub active_faults: Vec<FaultKind>,
    pub audio: AudioTelemetry,
    pub video: VideoTelemetry,
    pub thumbnail_b64: Option<String>,
    pub thumbnail_updated_at: Option<DateTime<Utc>>,
    pub ccv: TriStateMetric,
    pub total_views: TriStateMetric,
    pub likes: TriStateMetric,
    pub chat_rate_per_min: TriStateMetric,
    pub shares: TriStateMetric,
    pub gifts_count: TriStateMetric,
    pub last_status_reason: Option<String>,
    pub pfl_active: bool,
    pub uptime_pct: f32,
    pub resolved_via: Option<String>,
    pub effective_target_lufs: f32,
    pub effective_lufs_tolerance: f32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorTelemetry {
    pub gpu_adapter_name: String,
    pub vram_used_mb: u64,
    pub vram_budget_mb: u64,
    pub vram_pct: f32,
    pub ram_used_mb: u64,
    pub ram_total_mb: u64,
    pub ram_pct: f32,
    pub cpu_pct: f32,
    pub wan_rx_mbps: f32,
    pub wan_cap_mbps: f32,
    pub max_decode_latency_pct: f32,
    pub on_battery: bool,
    pub degradation_stage: u8,
    pub active_workers: usize,
    pub shed_channels: usize,
    pub updated_at: DateTime<Utc>,
}

impl Default for GovernorTelemetry {
    fn default() -> Self {
        Self {
            gpu_adapter_name: "Detecting GPU...".to_string(),
            vram_used_mb: 0,
            vram_budget_mb: 0,
            vram_pct: 0.0,
            ram_used_mb: 0,
            ram_total_mb: 0,
            ram_pct: 0.0,
            cpu_pct: 0.0,
            wan_rx_mbps: 0.0,
            wan_cap_mbps: 250.0,
            max_decode_latency_pct: 0.0,
            on_battery: false,
            degradation_stage: 0,
            active_workers: 0,
            shed_channels: 0,
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncidentRecord {
    pub id: String,
    pub channel_id: String,
    pub channel_name: String,
    pub group_id: String,
    pub fault: FaultKind,
    pub severity: AlertState,
    pub started_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub duration_secs: f64,
    pub clip_filename: Option<String>,
    pub acknowledged: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchMarker {
    pub id: String,
    pub show_id: String,
    pub label: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridSample {
    pub bucket_utc: DateTime<Utc>,
    pub channel_id: String,
    pub group_id: String,
    pub platform: Platform,
    pub ccv: Option<i64>,
    pub ccv_status: MetricStatus,
    pub source: MetricSource,
    pub views: Option<i64>,
    pub likes: Option<i64>,
    pub chat_msgs: Option<i64>,
    pub shares: Option<i64>,
    pub gifts: Option<i64>,
    pub coverage_pct: f32,
    pub extras_json: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualOverride {
    pub id: String,
    pub channel_id: String,
    pub metric_name: String,
    pub bucket_utc: Option<DateTime<Utc>>,
    pub value: f64,
    pub operator_note: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardSnapshot {
    pub show: ShowPreset,
    pub governor: GovernorTelemetry,
    pub global_intermission: bool,
    pub active_match: Option<MatchMarker>,
    pub matches: Vec<MatchMarker>,
    pub channels: Vec<ChannelRuntimeSnapshot>,
    pub incidents: Vec<IncidentRecord>,
    pub combined_ccv: i64,
    pub combined_pcv: i64,
    pub hours_watched: f64,
    pub active_pfl_channel_id: Option<String>,
    pub server_time: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alert_state_serde_matches_ui_and_legacy() {
        assert_eq!(serde_json::to_string(&AlertState::LoginGated).unwrap(), "\"LOGIN_GATED\"");
        assert_eq!(serde_json::to_string(&AlertState::AdBreak).unwrap(), "\"AD_BREAK\"");
        assert_eq!(serde_json::to_string(&AlertState::ExtractorErr).unwrap(), "\"EXTRACTOR_ERR\"");
        assert_eq!(serde_json::from_str::<AlertState>("\"LOGIN_GATED\"").unwrap(), AlertState::LoginGated);
        assert_eq!(serde_json::from_str::<AlertState>("\"LOGINGATED\"").unwrap(), AlertState::LoginGated);
        assert_eq!(serde_json::from_str::<AlertState>("\"ADBREAK\"").unwrap(), AlertState::AdBreak);
        assert_eq!(serde_json::from_str::<AlertState>("\"EXTRACTORERR\"").unwrap(), AlertState::ExtractorErr);
    }
}
