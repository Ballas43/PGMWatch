use crate::models::{ChannelConfig, FeedTier, LanguageGroupConfig, Platform, ShowPreset};
use anyhow::Context;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgmWatchConfig {
    pub server: ServerConfig,
    pub media: MediaConfig,
    pub governor: GovernorConfig,
    pub extraction: ExtractionConfig,
    pub alerts: AlertsConfig,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub bind_host: String,
    pub port: u16,
    pub open_browser_on_start: bool,
    pub outbound_bind_ip: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    pub gpu_mode: String,
    pub master_gpu_luid: String,
    pub simulcast_gpu_luid: String,
    pub extra_hw_frames: u32,
    pub thumbnail_fps: f32,
    pub clip_quota_gb: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernorConfig {
    pub vram_admit_max_pct: f32,
    pub ram_admit_max_pct: f32,
    pub wan_cap_mbps: f32,
    pub wan_admit_max_pct: f32,
    pub vram_stage1_pct: f32,
    pub vram_stage2_pct: f32,
    pub decode_latency_stage1_pct: f32,
    pub decode_latency_stage2_pct: f32,
    pub worker_memory_limit_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    pub auth_mode: String,
    pub max_concurrent_extractors: usize,
    pub enable_bgutil_pot: bool,
    pub bgutil_pot_port: u16,
    pub msedge_path: String,
}

fn default_lufs_tolerance() -> f32 {
    2.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertsConfig {
    pub recovery_hysteresis_secs: u64,
    pub critical_escalation_secs: u64,
    pub clip_cooldown_secs: u64,
    pub chat_spike_unique_viewers: usize,
    pub chat_spike_window_secs: u64,
    #[serde(default)]
    pub custom_target_lufs: Option<f32>,
    #[serde(default = "default_lufs_tolerance")]
    pub lufs_tolerance: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_dir: String,
    pub db_batch_interval_ms: u64,
}

impl Default for PgmWatchConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                bind_host: "127.0.0.1".to_string(),
                port: 8787,
                open_browser_on_start: true,
                outbound_bind_ip: String::new(),
            },
            media: MediaConfig {
                gpu_mode: "auto".to_string(),
                master_gpu_luid: String::new(),
                simulcast_gpu_luid: String::new(),
                extra_hw_frames: 6,
                thumbnail_fps: 1.5,
                clip_quota_gb: 10.0,
            },
            governor: GovernorConfig {
                vram_admit_max_pct: 75.0,
                ram_admit_max_pct: 92.0,
                wan_cap_mbps: 250.0,
                wan_admit_max_pct: 85.0,
                vram_stage1_pct: 80.0,
                vram_stage2_pct: 88.0,
                decode_latency_stage1_pct: 85.0,
                decode_latency_stage2_pct: 100.0,
                worker_memory_limit_mb: 512,
            },
            extraction: ExtractionConfig {
                auth_mode: "anonymous".to_string(),
                max_concurrent_extractors: 6,
                enable_bgutil_pot: false,
                bgutil_pot_port: 4416,
                msedge_path: String::new(),
            },
            alerts: AlertsConfig {
                recovery_hysteresis_secs: 3,
                critical_escalation_secs: 5,
                clip_cooldown_secs: 30,
                chat_spike_unique_viewers: 10,
                chat_spike_window_secs: 15,
                custom_target_lufs: None,
                lufs_tolerance: default_lufs_tolerance(),
            },
            storage: StorageConfig {
                data_dir: "data".to_string(),
                db_batch_interval_ms: 1000,
            },
        }
    }
}

impl PgmWatchConfig {
    pub fn load_or_create(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("failed to read config {}", path.display()))?;
            let parsed: Self = toml::from_str(&raw)
                .with_context(|| format!("failed to parse TOML {}", path.display()))?;
            Ok(parsed)
        } else {
            let cfg = Self::default();
            let serialized = toml::to_string_pretty(&cfg)?;
            std::fs::write(path, serialized)?;
            Ok(cfg)
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let serialized = toml::to_string_pretty(self)?;
        std::fs::write(path, serialized)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        let gpu_mode = self.media.gpu_mode.to_ascii_lowercase();
        if !matches!(gpu_mode.as_str(), "auto" | "d3d11va" | "cli_ffmpeg" | "cpu_light") {
            return Err("media.gpu_mode must be auto, d3d11va, cli_ffmpeg, or cpu_light".into());
        }
        if self.media.extra_hw_frames == 0 || self.media.extra_hw_frames > 64 {
            return Err("media.extra_hw_frames must be 1..64".into());
        }
        if !self.media.thumbnail_fps.is_finite()
            || self.media.thumbnail_fps < 0.2
            || self.media.thumbnail_fps > 15.0
        {
            return Err("media.thumbnail_fps must be 0.2..15.0".into());
        }
        if !self.media.clip_quota_gb.is_finite()
            || self.media.clip_quota_gb < 0.5
            || self.media.clip_quota_gb > 500.0
        {
            return Err("media.clip_quota_gb must be 0.5..500.0".into());
        }
        let auth_mode = self.extraction.auth_mode.to_ascii_lowercase();
        if !matches!(auth_mode.as_str(), "anonymous" | "fallback_auth") {
            return Err("extraction.auth_mode must be anonymous or fallback_auth".into());
        }
        if self.extraction.max_concurrent_extractors == 0
            || self.extraction.max_concurrent_extractors > 32
        {
            return Err("extraction.max_concurrent_extractors must be 1..32".into());
        }
        if self.extraction.bgutil_pot_port == 0 {
            return Err("extraction.bgutil_pot_port must be 1..65535".into());
        }
        if !self.governor.wan_cap_mbps.is_finite() || self.governor.wan_cap_mbps < 1.0 {
            return Err("governor.wan_cap_mbps must be >= 1.0".into());
        }
        for (label, pct) in [
            ("vram_admit_max_pct", self.governor.vram_admit_max_pct),
            ("ram_admit_max_pct", self.governor.ram_admit_max_pct),
            ("wan_admit_max_pct", self.governor.wan_admit_max_pct),
            ("vram_stage1_pct", self.governor.vram_stage1_pct),
            ("vram_stage2_pct", self.governor.vram_stage2_pct),
        ] {
            if !pct.is_finite() || !(10.0..=100.0).contains(&pct) {
                return Err(format!("governor.{label} must be 10..100"));
            }
        }
        if self.governor.worker_memory_limit_mb < 128 || self.governor.worker_memory_limit_mb > 8192 {
            return Err("governor.worker_memory_limit_mb must be 128..8192".into());
        }
        if self.alerts.chat_spike_unique_viewers == 0 || self.alerts.chat_spike_window_secs == 0 {
            return Err("alerts chat spike thresholds must be >= 1".into());
        }
        if let Some(target) = self.alerts.custom_target_lufs
            && (!target.is_finite() || !(-36.0..=-5.0).contains(&target))
        {
            return Err("alerts.custom_target_lufs must be between -36.0 and -5.0 LUFS".into());
        }
        if !self.alerts.lufs_tolerance.is_finite() || !(0.5..=10.0).contains(&self.alerts.lufs_tolerance) {
            return Err("alerts.lufs_tolerance must be between 0.5 and 10.0 LU".into());
        }
        if !self.server.outbound_bind_ip.trim().is_empty()
            && self.server.outbound_bind_ip.trim().parse::<std::net::IpAddr>().is_err()
        {
            return Err("server.outbound_bind_ip must be blank or a valid IP address".into());
        }
        Ok(())
    }

    pub fn resolve_data_dir(&self, base_dir: &Path) -> PathBuf {
        if let Some(override_dir) = std::env::var_os("PGMWATCH_DATA_DIR") {
            let p = PathBuf::from(override_dir);
            if !p.as_os_str().is_empty() {
                return if p.is_absolute() { p } else { base_dir.join(p) };
            }
        }
        let p = Path::new(&self.storage.data_dir);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base_dir.join(p)
        }
    }
}

/// Resolve the active `(target_lufs, tolerance_lu)` for a channel, applying per-channel
/// override first, then global `alerts.custom_target_lufs`, then platform default.
pub fn resolve_channel_lufs_spec(ch: &ChannelConfig, alerts: &AlertsConfig) -> (f32, f32) {
    let target = ch
        .target_lufs
        .or(alerts.custom_target_lufs)
        .unwrap_or_else(|| ch.platform.target_lufs())
        .clamp(-36.0, -5.0);
    let tolerance = ch
        .lufs_tolerance
        .unwrap_or(alerts.lufs_tolerance)
        .clamp(0.5, 10.0);
    (target, tolerance)
}

pub fn default_show_preset() -> ShowPreset {
    let default_keywords = vec![
        "no sound".into(),
        "muted".into(),
        "bisu".into(),
        "no audio".into(),
        "lag".into(),
        "freeze".into(),
        "stutter".into(),
        "suara hilang".into(),
    ];

    ShowPreset {
        show_id: "default-show".to_string(),
        title: "PGMWatch Live Broadcast".to_string(),
        created_at: Utc::now(),
        scheduled_start_at: None,
        scheduled_end_at: None,
        groups: vec![
            LanguageGroupConfig {
                id: "id-pgm".to_string(),
                name: "ID_PGM (Bahasa Indonesia)".to_string(),
                language_code: "ID".to_string(),
                intermission_active: false,
                complaint_keywords: default_keywords.clone(),
            },
            LanguageGroupConfig {
                id: "en-pgm".to_string(),
                name: "EN_PGM (English Global)".to_string(),
                language_code: "EN".to_string(),
                intermission_active: false,
                complaint_keywords: default_keywords,
            },
        ],
        channels: vec![
            ChannelConfig {
                id: "id-yt-master".to_string(),
                group_id: "id-pgm".to_string(),
                name: "ID YouTube Main".to_string(),
                url: "sim://healthy/id-yt-master".to_string(),
                platform: Platform::Youtube,
                tier: FeedTier::Master,
                priority: 1,
                orientation_vertical: false,
                enabled: true,
                target_lufs: None,
                lufs_tolerance: None,
                broadcast_delay_secs: 0,
                exclude_from_accounting: false,
                live_streams_only: true,
            },
            ChannelConfig {
                id: "id-tt-sim".to_string(),
                group_id: "id-pgm".to_string(),
                name: "ID TikTok Vertical".to_string(),
                url: "sim://healthy/id-tt-sim".to_string(),
                platform: Platform::Tiktok,
                tier: FeedTier::Simulcast,
                priority: 2,
                orientation_vertical: true,
                enabled: true,
                target_lufs: None,
                lufs_tolerance: None,
                broadcast_delay_secs: 0,
                exclude_from_accounting: false,
                live_streams_only: true,
            },
            ChannelConfig {
                id: "en-tw-master".to_string(),
                group_id: "en-pgm".to_string(),
                name: "EN Twitch Main".to_string(),
                url: "sim://healthy/en-tw-master".to_string(),
                platform: Platform::Twitch,
                tier: FeedTier::Master,
                priority: 1,
                orientation_vertical: false,
                enabled: true,
                target_lufs: None,
                lufs_tolerance: None,
                broadcast_delay_secs: 0,
                exclude_from_accounting: false,
                live_streams_only: true,
            },
            ChannelConfig {
                id: "en-fb-light".to_string(),
                group_id: "en-pgm".to_string(),
                name: "EN Facebook Audio/Stats".to_string(),
                url: "sim://healthy/en-fb-light".to_string(),
                platform: Platform::Facebook,
                tier: FeedTier::Light,
                priority: 3,
                orientation_vertical: false,
                enabled: true,
                target_lufs: None,
                lufs_tolerance: None,
                broadcast_delay_secs: 0,
                exclude_from_accounting: false,
                live_streams_only: true,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validate_and_save_roundtrip() {
        let mut cfg = PgmWatchConfig::default();
        assert!(cfg.validate().is_ok());

        cfg.media.gpu_mode = "invalid_mode".into();
        assert!(cfg.validate().is_err());
        cfg.media.gpu_mode = "d3d11va".into();
        cfg.extraction.bgutil_pot_port = 0;
        assert!(cfg.validate().is_err());
        cfg.extraction.bgutil_pot_port = 4416;
        cfg.extraction.enable_bgutil_pot = true;

        let path = std::env::temp_dir().join(format!("pgmwatch_cfg_test_{}.toml", std::process::id()));
        cfg.save(&path).expect("config save");
        let reloaded = PgmWatchConfig::load_or_create(&path).expect("config reload");
        assert!(reloaded.extraction.enable_bgutil_pot);
        assert_eq!(reloaded.media.gpu_mode, "d3d11va");
        let _ = std::fs::remove_file(path);
    }
}
