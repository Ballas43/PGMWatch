use crate::governor::ResourceGovernor;
use crate::mse_stream::MseHubManager;
use crate::resolver::{ResolvedMedia, UrlResolver};
use crate::stats::{ChannelStatsUpdate, ChatComplaintSensor};
use crate::win32::WindowsJobGuard;
use crate::worker_bridge::WorkerHandle;
use chrono::Utc;
use pgmwatch_core::config::{default_show_preset, resolve_channel_lufs_spec, PgmWatchConfig};
use pgmwatch_core::db::DbHandle;
use pgmwatch_core::models::{
    AudioTelemetry, ChannelConfig, ChannelRuntimeSnapshot, DashboardSnapshot,
    FeedTier, GovernorTelemetry, GridSample, IncidentRecord, ManualOverride,
    MatchMarker, MetricStatus, Platform, ShowPreset, TriStateMetric, VideoTelemetry,
};
use pgmwatch_core::state_machine::ChannelAlertTracker;
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

mod config_show;
mod controls;
mod governor_stats;

use config_show::{load_newest_preset_file, write_preset_json};

pub(crate) type TikTokSubTx = Mutex<Option<tokio::sync::mpsc::Sender<Vec<(String, String)>>>>;
pub(crate) type ChatSubTx =
    Mutex<Option<tokio::sync::mpsc::Sender<Vec<(String, Platform, String)>>>>;
pub(crate) type HoldWindow = (chrono::DateTime<Utc>, Option<chrono::DateTime<Utc>>);
pub(crate) type GroupHoldWindows = RwLock<HashMap<String, Vec<HoldWindow>>>;
pub(crate) type MasterLufsTimeline =
    RwLock<HashMap<String, VecDeque<(chrono::DateTime<Utc>, f32)>>>;

pub(super) const HOLD_END_GRACE_SECS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum WsBroadcast {
    Snapshot {
        snapshot: Box<DashboardSnapshot>,
    },
    Thumbnail {
        channel_id: String,
        jpeg_b64: String,
    },
    Meter {
        channel_id: String,
        audio: AudioTelemetry,
    },
    PflAudio {
        channel_id: String,
        sample_rate: u32,
        channels: u8,
        pcm_s16le_b64: String,
    },
    Incident {
        incident: IncidentRecord,
    },
}

pub(crate) struct ManagedChannel {
    pub(crate) config: ChannelConfig,
    pub(crate) effective_tier: FeedTier,
    pub(crate) forced_light: bool,
    pub(crate) tracker: ChannelAlertTracker,
    pub(crate) audio: AudioTelemetry,
    pub(crate) video: VideoTelemetry,
    pub(crate) thumbnail_b64: Option<String>,
    pub(crate) thumbnail_updated_at: Option<chrono::DateTime<Utc>>,
    pub(crate) ccv: TriStateMetric,
    pub(crate) total_views: TriStateMetric,
    pub(crate) likes: TriStateMetric,
    pub(crate) chat_rate_per_min: TriStateMetric,
    pub(crate) shares: TriStateMetric,
    pub(crate) gifts_count: TriStateMetric,
    pub(crate) resolved_via: Option<String>,
    pub(crate) rx_kbps: f32,
    pub(crate) active_incident_id: Option<String>,
    pub(crate) updated_at: chrono::DateTime<Utc>,
}

pub struct SupervisorState {
    pub cfg: RwLock<PgmWatchConfig>,
    pub base_dir: PathBuf,
    pub data_dir: PathBuf,
    pub clips_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub bin_dir: PathBuf,
    pub db: DbHandle,
    pub job_guard: Arc<WindowsJobGuard>,
    pub dash_relay: Arc<crate::dash_relay::DashRelayState>,
    pub resolver: Arc<UrlResolver>,
    pub governor: Mutex<ResourceGovernor>,
    pub governor_tel: RwLock<GovernorTelemetry>,
    pub show: RwLock<ShowPreset>,
    pub global_intermission: RwLock<bool>,
    pub(crate) global_hold_windows: RwLock<Vec<HoldWindow>>,
    pub(crate) group_hold_windows: GroupHoldWindows,
    pub(crate) master_lufs_timeline: MasterLufsTimeline,
    pub active_match: RwLock<Option<MatchMarker>>,
    pub matches: RwLock<Vec<MatchMarker>>,
    pub overrides: RwLock<Vec<ManualOverride>>,
    pub incidents: RwLock<Vec<IncidentRecord>>,
    pub active_pfl_channel: RwLock<Option<String>>,
    pub(crate) channels: RwLock<HashMap<String, ManagedChannel>>,
    pub(crate) workers: Mutex<HashMap<String, WorkerHandle>>,
    pub(crate) resolved_media: RwLock<HashMap<String, ResolvedMedia>>,
    pub(crate) mse_hubs: MseHubManager,
    pub chat_sensor: Mutex<ChatComplaintSensor>,
    pub tiktok_cache: RwLock<HashMap<String, ChannelStatsUpdate>>,
    pub(crate) tiktok_cmd_tx: TikTokSubTx,
    pub(crate) chat_rates: RwLock<HashMap<String, f64>>,
    pub(crate) chat_cmd_tx: ChatSubTx,
    pub(crate) chat_spike_cooldown: Mutex<HashMap<String, std::time::Instant>>,
    pub(crate) mode_b_cache: RwLock<HashMap<String, (ChannelStatsUpdate, std::time::Instant)>>,
    pub(crate) grid_samples: RwLock<HashMap<(chrono::DateTime<Utc>, String), GridSample>>,
    pub(crate) recovery_healthy_ticks: Mutex<u32>,
    pub(crate) stats_notify: Arc<tokio::sync::Notify>,
    pub ws_tx: broadcast::Sender<WsBroadcast>,
    pub(crate) shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub combined_pcv: RwLock<i64>,
    pub hours_watched: RwLock<f64>,
}

impl SupervisorState {
    pub async fn init(cfg: PgmWatchConfig, base_dir: PathBuf) -> anyhow::Result<Arc<Self>> {
        let data_dir = cfg.resolve_data_dir(&base_dir);
        let presets_dir = data_dir.join("presets");
        let clips_dir = data_dir.join("clips");
        let profiles_dir = data_dir.join("profiles");
        std::fs::create_dir_all(&presets_dir)?;
        std::fs::create_dir_all(&clips_dir)?;
        std::fs::create_dir_all(&profiles_dir)?;

        let bin_dir = if base_dir.join("bin").exists() {
            base_dir.join("bin")
        } else {
            base_dir.join("runtime").join("bin")
        };

        let db = DbHandle::open(&data_dir.join("sentinel.db"), cfg.storage.db_batch_interval_ms).await?;
        let db_show = db.load_active_show_with_ts().await?;
        let file_show = load_newest_preset_file(&presets_dir);
        let show = match (db_show, file_show) {
            (Some((ds, db_ts)), Some((fs, fs_ts))) => {
                if fs_ts > db_ts + chrono::Duration::seconds(2) {
                    fs
                } else {
                    ds
                }
            }
            (Some((ds, _)), None) => ds,
            (None, Some((fs, _))) => fs,
            (None, None) => default_show_preset(),
        };
        if let Err(err) = db.upsert_show_now(&show).await {
            tracing::warn!("Failed initial synchronous show persist: {err}");
        }
        let _ = db.set_active_show_id_now(&show.show_id).await;
        write_preset_json(&presets_dir, &show);

        let matches = db.load_matches().await.unwrap_or_default();
        let active_match = matches.iter().find(|m| m.ended_at.is_none()).cloned();
        let overrides = db.load_overrides().await.unwrap_or_default();
        if let Err(err) = db.close_stale_open_incidents_now().await {
            tracing::warn!("Failed closing stale incidents on startup: {err}");
        }
        let incidents = db.load_incidents().await.unwrap_or_default();
        let initial_grid_samples = db.load_grid_samples(None).await.unwrap_or_default();
        let mut grid_cache = HashMap::with_capacity(initial_grid_samples.len());
        for s in initial_grid_samples {
            grid_cache.insert((s.bucket_utc, s.channel_id.clone()), s);
        }

        let job_guard = Arc::new(WindowsJobGuard::new(cfg.governor.worker_memory_limit_mb)?);
        let dash_relay = Arc::new(crate::dash_relay::DashRelayState::new(cfg.server.port));
        let resolver = Arc::new(UrlResolver::new(
            cfg.extraction.clone(),
            bin_dir.clone(),
            profiles_dir.clone(),
            Arc::clone(&job_guard),
            Arc::clone(&dash_relay),
        ));
        resolver.set_bind_ip(&cfg.server.outbound_bind_ip).await;
        let governor = ResourceGovernor::new(cfg.governor.clone());
        let (ws_tx, _) = broadcast::channel(512);
        let (shutdown_tx, _) = tokio::sync::watch::channel(false);

        let mut ch_map = HashMap::new();
        for ch in &show.channels {
            ch_map.insert(ch.id.clone(), new_managed_channel(ch.clone()));
        }

        Ok(Arc::new(Self {
            cfg: RwLock::new(cfg),
            base_dir,
            data_dir,
            clips_dir,
            profiles_dir,
            bin_dir,
            db,
            job_guard,
            dash_relay,
            resolver,
            governor: Mutex::new(governor),
            governor_tel: RwLock::new(GovernorTelemetry::default()),
            show: RwLock::new(show),
            global_intermission: RwLock::new(false),
            global_hold_windows: RwLock::new(Vec::new()),
            group_hold_windows: RwLock::new(HashMap::new()),
            master_lufs_timeline: RwLock::new(HashMap::new()),
            active_match: RwLock::new(active_match),
            matches: RwLock::new(matches),
            overrides: RwLock::new(overrides),
            incidents: RwLock::new(incidents),
            active_pfl_channel: RwLock::new(None),
            channels: RwLock::new(ch_map),
            workers: Mutex::new(HashMap::new()),
            resolved_media: RwLock::new(HashMap::new()),
            mse_hubs: MseHubManager::new(),
            chat_sensor: Mutex::new(ChatComplaintSensor::new()),
            tiktok_cache: RwLock::new(HashMap::new()),
            tiktok_cmd_tx: Mutex::new(None),
            chat_rates: RwLock::new(HashMap::new()),
            chat_cmd_tx: Mutex::new(None),
            chat_spike_cooldown: Mutex::new(HashMap::new()),
            mode_b_cache: RwLock::new(HashMap::new()),
            grid_samples: RwLock::new(grid_cache),
            recovery_healthy_ticks: Mutex::new(0),
            stats_notify: Arc::new(tokio::sync::Notify::new()),
            ws_tx,
            shutdown_tx,
            combined_pcv: RwLock::new(0),
            hours_watched: RwLock::new(0.0),
        }))
    }

    pub async fn snapshot(&self) -> DashboardSnapshot {
        self.snapshot_with_thumbnails(true).await
    }

    /// Build a periodic 1 Hz snapshot without duplicating base64 thumbnails already streamed via WsBroadcast::Thumbnail.
    pub async fn snapshot_for_periodic_ws(&self) -> DashboardSnapshot {
        self.snapshot_with_thumbnails(false).await
    }

    pub(crate) async fn broadcast_snapshot(&self) {
        let snap = self.snapshot().await;
        let _ = self.ws_tx.send(WsBroadcast::Snapshot {
            snapshot: Box::new(snap),
        });
    }

    async fn snapshot_with_thumbnails(&self, include_thumbnails: bool) -> DashboardSnapshot {
        let show = self.show.read().await.clone();
        let governor = self.governor_tel.read().await.clone();
        let global_intermission = *self.global_intermission.read().await;
        let active_match = self.active_match.read().await.clone();
        let matches = self.matches.read().await.clone();
        let active_pfl_channel_id = self.active_pfl_channel.read().await.clone();
        let incidents = self.incidents.read().await.clone();
        let alerts_cfg = self.cfg.read().await.alerts.clone();

        let mut channels: Vec<ChannelRuntimeSnapshot> = self
            .channels
            .read()
            .await
            .values()
            .map(|m| {
                let (effective_target_lufs, effective_lufs_tolerance) =
                    resolve_channel_lufs_spec(&m.config, &alerts_cfg);
                ChannelRuntimeSnapshot {
                    config: m.config.clone(),
                    effective_tier: m.effective_tier,
                    video_shed_resource: m.effective_tier != m.config.tier || m.forced_light,
                    alert_state: m.tracker.state,
                    active_faults: m.tracker.active_faults.clone(),
                    audio: m.audio.clone(),
                    video: m.video.clone(),
                    thumbnail_b64: if include_thumbnails {
                        m.thumbnail_b64.clone()
                    } else {
                        None
                    },
                    thumbnail_updated_at: m.thumbnail_updated_at,
                    ccv: m.ccv.clone(),
                    total_views: m.total_views.clone(),
                    likes: m.likes.clone(),
                    chat_rate_per_min: m.chat_rate_per_min.clone(),
                    shares: m.shares.clone(),
                    gifts_count: m.gifts_count.clone(),
                    last_status_reason: m.tracker.status_reason.clone(),
                    pfl_active: active_pfl_channel_id.as_deref() == Some(m.config.id.as_str()),
                    uptime_pct: m.tracker.uptime_pct(),
                    resolved_via: m.resolved_via.clone(),
                    effective_target_lufs,
                    effective_lufs_tolerance,
                    updated_at: m.updated_at,
                }
            })
            .collect();

        channels.sort_by(|a, b| {
            a.config
                .group_id
                .cmp(&b.config.group_id)
                .then(a.config.priority.cmp(&b.config.priority))
                .then(a.config.id.cmp(&b.config.id))
        });

        let combined_ccv: i64 = channels
            .iter()
            .filter_map(|c| {
                if !c.config.exclude_from_accounting && c.ccv.status == MetricStatus::Value {
                    c.ccv.value.map(|v| v.round() as i64)
                } else {
                    None
                }
            })
            .sum();
        let now_bucket = pgmwatch_core::grid::floor_to_minute_utc(Utc::now());
        let in_schedule_window = show
            .scheduled_start_at
            .map(pgmwatch_core::grid::floor_to_minute_utc)
            .is_none_or(|sb| now_bucket >= sb)
            && show
                .scheduled_end_at
                .map(pgmwatch_core::grid::floor_to_minute_utc)
                .is_none_or(|eb| now_bucket <= eb);
        let stored_pcv = *self.combined_pcv.read().await;
        let pcv = if in_schedule_window {
            stored_pcv.max(combined_ccv)
        } else {
            stored_pcv
        };
        let hours_watched = *self.hours_watched.read().await;

        DashboardSnapshot {
            show,
            governor,
            global_intermission,
            active_match,
            matches,
            channels,
            incidents,
            combined_ccv,
            combined_pcv: pcv,
            hours_watched,
            active_pfl_channel_id,
            server_time: Utc::now(),
        }
    }

    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub fn subscribe_shutdown(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub async fn stop_all_workers(&self) {
        self.mse_hubs.invalidate_all().await;
        let ids: Vec<String> = self.workers.lock().await.keys().cloned().collect();
        for id in ids {
            self.stop_worker(&id).await;
        }
    }
}

pub(super) fn new_managed_channel(config: ChannelConfig) -> ManagedChannel {
    let tracker = if config.enabled {
        ChannelAlertTracker::new_connecting()
    } else {
        ChannelAlertTracker::new()
    };
    ManagedChannel {
        effective_tier: config.tier,
        config,
        forced_light: false,
        tracker,
        audio: AudioTelemetry::default(),
        video: VideoTelemetry::default(),
        thumbnail_b64: None,
        thumbnail_updated_at: None,
        ccv: TriStateMetric::unavailable("Initializing"),
        total_views: TriStateMetric::na(),
        likes: TriStateMetric::na(),
        chat_rate_per_min: TriStateMetric::na(),
        shares: TriStateMetric::na(),
        gifts_count: TriStateMetric::na(),
        resolved_via: None,
        rx_kbps: 0.0,
        active_incident_id: None,
        updated_at: Utc::now(),
    }
}
