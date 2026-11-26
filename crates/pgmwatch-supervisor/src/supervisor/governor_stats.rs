use super::{SupervisorState, WsBroadcast};
use crate::stats::{collect_platform_stats, ChannelStatsUpdate, PlatformStatsCaches};
use crate::worker_bridge::send_worker_cmd;
use chrono::Utc;
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::grid::{
    apply_overrides_to_samples, floor_to_minute_utc, prepare_accounting_samples, summarize_grid,
};
use pgmwatch_core::ipc::SupervisorToWorker;
use pgmwatch_core::models::{ChannelConfig, FeedTier, GridSample, MetricStatus, Platform, TriStateMetric};
use std::collections::HashMap;
use std::sync::Arc;

pub(super) fn build_stats_http_client(bind_ip: &str) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(std::time::Duration::from_secs(6));
    if let Some(ip) = (!bind_ip.trim().is_empty())
        .then(|| bind_ip.trim().parse::<std::net::IpAddr>().ok())
        .flatten()
    {
        builder = builder.local_address(ip);
    }
    builder.build().unwrap_or_default()
}

impl SupervisorState {
    pub async fn start_background_tasks(self: &Arc<Self>) {
        self.spawn_enabled_workers().await;

        let gov_state = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            let mut shutdown_rx = gov_state.subscribe_shutdown();
            let mut ticks = 0u64;
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shutdown_rx.changed() => break,
                }
                if *shutdown_rx.borrow() {
                    break;
                }
                ticks += 1;
                if ticks % 2 == 1 {
                    gov_state.run_governor_tick().await;
                }
                let snap = gov_state.snapshot_for_periodic_ws().await;
                let _ = gov_state.ws_tx.send(WsBroadcast::Snapshot {
                    snapshot: Box::new(snap),
                });
            }
        });

        let stats_state = Arc::clone(self);
        tokio::spawn(async move {
            let mut active_bind_ip = stats_state.cfg.read().await.server.outbound_bind_ip.clone();
            let mut client = build_stats_http_client(&active_bind_ip);
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10));
            let mut shutdown_rx = stats_state.subscribe_shutdown();
            let mut tick = 0u64;
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = stats_state.stats_notify.notified() => {}
                    _ = shutdown_rx.changed() => break,
                }
                if *shutdown_rx.borrow() {
                    break;
                }
                let desired_bind = stats_state.cfg.read().await.server.outbound_bind_ip.clone();
                if desired_bind != active_bind_ip {
                    client = build_stats_http_client(&desired_bind);
                    active_bind_ip = desired_bind;
                }
                tick += 1;
                stats_state.run_stats_tick(&client, tick).await;
            }
        });

        self.spawn_tiktok_bridge().await;
        self.spawn_chat_bridge().await;
    }

    async fn run_governor_tick(self: &Arc<Self>) {
        let (active_workers, shed_channels, wan_kbps, max_lat_pct) = {
            let map = self.channels.read().await;
            let mut wan = 0.0f32;
            let mut max_lat = 0.0f32;
            let mut shed = 0usize;
            for c in map.values() {
                wan += c.rx_kbps;
                let frame_ms = if c.video.fps > 1.0 { 1000.0 / c.video.fps } else { 33.3 };
                let lat = (c.video.decode_ms / frame_ms) * 100.0;
                max_lat = max_lat.max(lat);
                if c.forced_light || c.effective_tier != c.config.tier {
                    shed += 1;
                }
            }
            let worker_count = self.workers.lock().await.len();
            (worker_count, shed, wan, max_lat)
        };

        let (thumb_fps, preferred_luid) = {
            let c = self.cfg.read().await;
            let luid = if !c.media.master_gpu_luid.trim().is_empty() {
                c.media.master_gpu_luid.clone()
            } else {
                c.media.simulcast_gpu_luid.clone()
            };
            (c.media.thumbnail_fps, luid)
        };
        let decision = {
            let mut gov = self.governor.lock().await;
            gov.set_preferred_gpu_luid(preferred_luid);
            gov.evaluate(
                active_workers,
                shed_channels,
                wan_kbps / 1000.0,
                max_lat_pct,
                thumb_fps,
            )
        };

        *self.governor_tel.write().await = decision.telemetry.clone();

        {
            let mut workers = self.workers.lock().await;
            for w in workers.values_mut() {
                let _ = send_worker_cmd(
                    &mut w.stdin,
                    &SupervisorToWorker::SetThumbnailFps {
                        fps: decision.target_thumbnail_fps,
                    },
                )
                .await;
            }
        }

        if decision.telemetry.degradation_stage > 0 {
            *self.recovery_healthy_ticks.lock().await = 0;
        }

        if decision.shed_to_light {
            let victim: Option<ChannelConfig> = {
                let mut map = self.channels.write().await;
                let candidate_id = map
                    .values()
                    .filter(|c| c.effective_tier == FeedTier::Simulcast)
                    .max_by_key(|c| c.config.priority)
                    .map(|c| c.config.id.clone());
                if let Some(id) = candidate_id {
                    map.get_mut(&id).map(|m| {
                        m.effective_tier = FeedTier::Light;
                        m.config.clone()
                    })
                } else {
                    None
                }
            };
            if let Some(ch) = victim {
                let st = Arc::clone(self);
                tokio::spawn(async move {
                    let resolved = match st
                        .resolver
                        .resolve(&ch.url, ch.platform, FeedTier::Light, ch.live_streams_only)
                        .await
                    {
                        Ok(r) => Some(r),
                        Err(_) => st.resolved_media.read().await.get(&ch.id).cloned(),
                    };
                    let still_light = st
                        .channels
                        .read()
                        .await
                        .get(&ch.id)
                        .map(|m| m.effective_tier == FeedTier::Light)
                        .unwrap_or(false);
                    if !still_light {
                        return;
                    }
                    st.mse_hubs.invalidate(&ch.id).await;
                    if let Some(res) = resolved {
                        st.resolved_media.write().await.insert(ch.id.clone(), res.clone());
                        let mut workers = st.workers.lock().await;
                        if let Some(w) = workers.get_mut(&ch.id) {
                            let _ = send_worker_cmd(
                                &mut w.stdin,
                                &SupervisorToWorker::UpdateStream {
                                    resolved_url: res.stream_url,
                                    audio_url: res.audio_url,
                                    headers: res.headers,
                                    tier: FeedTier::Light,
                                },
                            )
                            .await;
                        }
                    }
                });
            }
        } else if decision.telemetry.degradation_stage == 0 {
            let ticks = {
                let mut g = self.recovery_healthy_ticks.lock().await;
                *g += 1;
                *g
            };
            if ticks >= 8 {
                *self.recovery_healthy_ticks.lock().await = 0;
                let recover_cand: Option<ChannelConfig> = {
                    let map = self.channels.read().await;
                    map.values()
                        .filter(|c| {
                            c.effective_tier == FeedTier::Light
                                && c.config.tier == FeedTier::Simulcast
                                && !c.forced_light
                        })
                        .min_by_key(|c| c.config.priority)
                        .map(|c| c.config.clone())
                };
                if let Some(ch) = recover_cand {
                    let admit_ok = self.governor.lock().await.admit_tier_upgrade().is_ok();
                    if admit_ok {
                        let st = Arc::clone(self);
                        tokio::spawn(async move {
                            if let Ok(resolved) = st
                                .resolver
                                .resolve(
                                    &ch.url,
                                    ch.platform,
                                    FeedTier::Simulcast,
                                    ch.live_streams_only,
                                )
                                .await
                            {
                                st.mse_hubs.invalidate(&ch.id).await;
                                {
                                    let mut map = st.channels.write().await;
                                    if let Some(m) = map.get_mut(&ch.id)
                                        && m.effective_tier == FeedTier::Light
                                        && !m.forced_light
                                    {
                                        m.effective_tier = FeedTier::Simulcast;
                                    }
                                }
                                st.resolved_media
                                    .write()
                                    .await
                                    .insert(ch.id.clone(), resolved.clone());
                                let mut workers = st.workers.lock().await;
                                if let Some(w) = workers.get_mut(&ch.id) {
                                    let _ = send_worker_cmd(
                                        &mut w.stdin,
                                        &SupervisorToWorker::UpdateStream {
                                            resolved_url: resolved.stream_url,
                                            audio_url: resolved.audio_url,
                                            headers: resolved.headers,
                                            tier: FeedTier::Simulcast,
                                        },
                                    )
                                    .await;
                                }
                            }
                        });
                    }
                }
            }
        }
    }

    async fn run_stats_tick(&self, client: &reqwest::Client, tick: u64) {
        let configs: Vec<ChannelConfig> = self
            .channels
            .read()
            .await
            .values()
            .filter(|m| m.config.enabled)
            .map(|m| m.config.clone())
            .collect();
        let tt_cache = self.tiktok_cache.read().await.clone();
        let chat_rates = self.chat_rates.read().await.clone();
        let bucket = floor_to_minute_utc(Utc::now());

        let msedge_path = self.cfg.read().await.extraction.msedge_path.clone();
        if let Some(msedge) = crate::ghost_browser::detect_msedge_path(&msedge_path) {
            let now = std::time::Instant::now();
            let current_ccv_ok: HashMap<String, bool> = self
                .channels
                .read()
                .await
                .iter()
                .map(|(id, m)| (id.clone(), m.ccv.status == MetricStatus::Value))
                .collect();

            for ch in &configs {
                if ch.url.starts_with("sim://") {
                    continue;
                }
                let needs_mode_b = match ch.platform {
                    Platform::Bigo | Platform::Custom => true,
                    Platform::Facebook => {
                        let has_fb_auth = self
                            .profiles_dir
                            .join(crate::resolver::auth_profile_slug(Platform::Facebook))
                            .join("Default")
                            .exists();
                        has_fb_auth && !current_ccv_ok.get(&ch.id).copied().unwrap_or(false)
                    }
                    _ => false,
                };
                if !needs_mode_b {
                    continue;
                }

                let stale = {
                    let cache = self.mode_b_cache.read().await;
                    cache
                        .get(&ch.id)
                        .map(|(_, ts)| now.duration_since(*ts) >= std::time::Duration::from_secs(45))
                        .unwrap_or(true)
                };
                if !stale {
                    continue;
                }

                let profile_dir = if ch.platform == Platform::Facebook {
                    self.profiles_dir
                        .join(crate::resolver::auth_profile_slug(Platform::Facebook))
                } else {
                    self.profiles_dir.join("mode_b_ephemeral")
                };

                if let Ok(dom) = crate::ghost_browser::scrape_dom_stats_via_edge_cdp(
                    &msedge,
                    &profile_dir,
                    &ch.url,
                    ch.platform,
                    Arc::clone(&self.job_guard),
                )
                .await
                {
                    let mut extras = HashMap::new();
                    extras.insert("mode".into(), serde_json::json!("ghost_mode_b"));
                    let upd = ChannelStatsUpdate {
                        channel_id: ch.id.clone(),
                        ccv: dom
                            .ccv
                            .map(|v| TriStateMetric::value(v, 90.0))
                            .unwrap_or_else(|| TriStateMetric::unavailable("Mode B DOM viewer count unavailable")),
                        total_views: dom
                            .total_views
                            .map(|v| TriStateMetric::value(v, 90.0))
                            .unwrap_or_else(TriStateMetric::na),
                        likes: dom
                            .likes
                            .map(|v| TriStateMetric::value(v, 90.0))
                            .unwrap_or_else(TriStateMetric::na),
                        chat_rate_per_min: TriStateMetric::na(),
                        shares: TriStateMetric::na(),
                        gifts_count: TriStateMetric::na(),
                        extras,
                    };
                    self.mode_b_cache
                        .write()
                        .await
                        .insert(ch.id.clone(), (upd, std::time::Instant::now()));
                }
            }
        }

        let mode_b_map: HashMap<String, ChannelStatsUpdate> = self
            .mode_b_cache
            .read()
            .await
            .iter()
            .map(|(k, (v, _))| (k.clone(), v.clone()))
            .collect();

        let stat_caches = PlatformStatsCaches {
            tiktok: &tt_cache,
            chat_rates: &chat_rates,
            mode_b: &mode_b_map,
        };
        let mut new_samples = Vec::with_capacity(configs.len());

        for ch in configs {
            let update = collect_platform_stats(
                client,
                &ch.id,
                &ch.url,
                ch.platform,
                tick,
                &stat_caches,
            )
            .await;

            {
                let mut map = self.channels.write().await;
                if let Some(m) = map.get_mut(&ch.id) {
                    m.ccv = update.ccv.clone();
                    m.total_views = update.total_views.clone();
                    m.likes = update.likes.clone();
                    m.chat_rate_per_min = update.chat_rate_per_min.clone();
                    m.shares = update.shares.clone();
                    m.gifts_count = update.gifts_count.clone();
                }
            }

            let sample = GridSample {
                bucket_utc: bucket,
                channel_id: ch.id,
                group_id: ch.group_id,
                platform: ch.platform,
                ccv: update.ccv.value.map(|v| v.round() as i64),
                ccv_status: update.ccv.status,
                source: update.ccv.source,
                views: update.total_views.value.map(|v| v.round() as i64),
                likes: update.likes.value.map(|v| v.round() as i64),
                chat_msgs: update.chat_rate_per_min.value.map(|v| v.round() as i64),
                shares: update.shares.value.map(|v| v.round() as i64),
                gifts: update.gifts_count.value.map(|v| v.round() as i64),
                coverage_pct: update.ccv.coverage_pct,
                extras_json: update.extras,
            };
            new_samples.push(sample.clone());
            self.db.enqueue(DbCommand::UpsertGridSample(sample)).await;
        }

        let samples: Vec<GridSample> = {
            let mut cache = self.grid_samples.write().await;
            for sample in new_samples {
                cache.insert((sample.bucket_utc, sample.channel_id.clone()), sample);
            }
            cache.values().cloned().collect()
        };
        let overrides = self.overrides.read().await.clone();
        let show_channels = self.show.read().await.channels.clone();
        let merged = prepare_accounting_samples(
            &apply_overrides_to_samples(&samples, &overrides),
            &show_channels,
        );
        let sum = summarize_grid(&merged);
        *self.combined_pcv.write().await = sum.combined_pcv;
        *self.hours_watched.write().await = sum.hours_watched;
    }
}
