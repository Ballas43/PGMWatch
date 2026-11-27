use super::{SupervisorState, WsBroadcast};
use crate::stats::{collect_platform_stats, ChannelStatsUpdate, PlatformStatsCaches};
use crate::worker_bridge::send_worker_cmd;
use chrono::Utc;
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::grid::{
    apply_overrides_to_samples, filter_samples_by_window, floor_to_minute_utc,
    prepare_accounting_samples, summarize_grid,
};
use pgmwatch_core::ipc::SupervisorToWorker;
use pgmwatch_core::models::{
    AlertState, ChannelConfig, FeedTier, GridSample, MetricStatus, Platform, TriStateMetric,
};
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

        // Auto-poll enabled Offline / ExtractorErr channels so pre-live links seamlessly go live
        // without requiring manual Retry clicks. Polls every 20s near/during broadcast (60s far outside).
        let offline_state = Arc::clone(self);
        tokio::spawn(async move {
            struct PollGuard {
                set: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
                id: String,
            }
            impl Drop for PollGuard {
                fn drop(&mut self) {
                    if let Ok(mut g) = self.set.lock() {
                        g.remove(&self.id);
                    }
                }
            }

            let in_flight_polls: Arc<std::sync::Mutex<std::collections::HashSet<String>>> =
                Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()));
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(20));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // Skip the immediate t=0 tick so initial startup workers finish resolving first.
            interval.tick().await;
            let mut shutdown_rx = offline_state.subscribe_shutdown();
            let mut poll_tick = 0u64;
            loop {
                tokio::select! {
                    _ = interval.tick() => {}
                    _ = shutdown_rx.changed() => break,
                }
                if *shutdown_rx.borrow() {
                    break;
                }
                poll_tick += 1;

                let (sched_start, sched_end) = {
                    let s = offline_state.show.read().await;
                    (s.scheduled_start_at, s.scheduled_end_at)
                };
                let now = Utc::now();
                let far_outside_window = sched_start
                    .is_some_and(|st| now < st - chrono::Duration::minutes(15))
                    || sched_end.is_some_and(|en| now > en + chrono::Duration::minutes(15));
                if far_outside_window && poll_tick % 3 != 1 {
                    continue;
                }
                let cadence_secs = if far_outside_window { 60 } else { 20 };

                let active_worker_ids: std::collections::HashSet<String> = {
                    let workers = offline_state.workers.lock().await;
                    workers.keys().cloned().collect()
                };
                let candidates: Vec<(ChannelConfig, FeedTier)> = {
                    let map = offline_state.channels.read().await;
                    map.values()
                        .filter(|m| {
                            m.config.enabled
                                && !active_worker_ids.contains(&m.config.id)
                                && matches!(
                                    m.tracker.state,
                                    AlertState::Offline | AlertState::ExtractorErr
                                )
                        })
                        .map(|m| (m.config.clone(), m.effective_tier))
                        .collect()
                };

                for (ch, eff_tier) in candidates {
                    {
                        let Ok(mut set) = in_flight_polls.lock() else {
                            continue;
                        };
                        if !set.insert(ch.id.clone()) {
                            continue;
                        }
                    }
                    let guard = PollGuard {
                        set: Arc::clone(&in_flight_polls),
                        id: ch.id.clone(),
                    };
                    let st = Arc::clone(&offline_state);
                    tokio::spawn(async move {
                        let _guard = guard;
                        if *st.shutdown_tx.borrow() {
                            return;
                        }
                        st.resolver.invalidate_url(&ch.url).await;
                        match st
                            .resolver
                            .resolve(&ch.url, ch.platform, eff_tier, ch.live_streams_only)
                            .await
                        {
                            Ok(_) => {
                                if *st.shutdown_tx.borrow() {
                                    return;
                                }
                                // Cache now holds the live resolved media URL; spawn worker immediately.
                                let still_offline = {
                                    let map = st.channels.read().await;
                                    map.get(&ch.id).is_some_and(|m| {
                                        m.config.enabled
                                            && m.config.url == ch.url
                                            && matches!(
                                                m.tracker.state,
                                                AlertState::Offline | AlertState::ExtractorErr
                                            )
                                    })
                                };
                                if still_offline {
                                    tracing::info!(
                                        channel_id = %ch.id,
                                        "Pre-live/offline stream is now live; auto-starting worker"
                                    );
                                    st.reload_channel_worker(&ch.id).await;
                                }
                            }
                            Err(crate::resolver::ResolveError::LoginGated(msg)) => {
                                if let Some(m) = st.channels.write().await.get_mut(&ch.id)
                                    && m.config.enabled
                                    && m.config.url == ch.url
                                {
                                    m.tracker.set_external_state(AlertState::LoginGated, msg);
                                }
                            }
                            Err(crate::resolver::ResolveError::Offline(_)) => {
                                if let Some(m) = st.channels.write().await.get_mut(&ch.id)
                                    && m.config.enabled
                                    && m.config.url == ch.url
                                    && m.tracker.state == AlertState::Offline
                                {
                                    m.tracker.set_external_state(
                                        AlertState::Offline,
                                        format!(
                                            "Waiting for live stream (auto-checking every {cadence_secs}s)"
                                        ),
                                    );
                                }
                            }
                            Err(crate::resolver::ResolveError::ExtractorFailed(_)) => {
                                // Keep existing status reason until stream comes online.
                            }
                        }
                    });
                }
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
            // Shed highest-bitrate SIMULCAST channel to LIGHT first; if all SIMULCAST feeds are
            // already LIGHT and WAN/GPU is still overloaded, step MASTER down 1080p60 -> 480p (Simulcast).
            let victim: Option<(ChannelConfig, FeedTier)> = {
                let mut map = self.channels.write().await;
                let sim_id = map
                    .values()
                    .filter(|c| c.effective_tier == FeedTier::Simulcast)
                    .max_by_key(|c| (c.rx_kbps.max(0.0) as u64, c.config.priority))
                    .map(|c| c.config.id.clone());
                if let Some(id) = sim_id {
                    map.get_mut(&id).map(|m| {
                        m.effective_tier = FeedTier::Light;
                        (m.config.clone(), FeedTier::Light)
                    })
                } else {
                    let master_id = map
                        .values()
                        .filter(|c| c.effective_tier == FeedTier::Master)
                        .max_by_key(|c| (c.rx_kbps.max(0.0) as u64, c.config.priority))
                        .map(|c| c.config.id.clone());
                    master_id.and_then(|id| {
                        map.get_mut(&id).map(|m| {
                            m.effective_tier = FeedTier::Simulcast;
                            (m.config.clone(), FeedTier::Simulcast)
                        })
                    })
                }
            };
            if let Some((ch, target_tier)) = victim {
                let st = Arc::clone(self);
                tokio::spawn(async move {
                    let resolved = match st
                        .resolver
                        .resolve(&ch.url, ch.platform, target_tier, ch.live_streams_only)
                        .await
                    {
                        Ok(r) => Some(r),
                        Err(_) => st.resolved_media.read().await.get(&ch.id).cloned(),
                    };
                    let still_target = st
                        .channels
                        .read()
                        .await
                        .get(&ch.id)
                        .map(|m| m.effective_tier == target_tier)
                        .unwrap_or(false);
                    if !still_target {
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
                                    tier: target_tier,
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
                // Recover stepped-down MASTER channels first, then shed SIMULCAST channels.
                let recover_cand: Option<(ChannelConfig, FeedTier)> = {
                    let map = self.channels.read().await;
                    map.values()
                        .filter(|c| {
                            c.effective_tier == FeedTier::Simulcast
                                && c.config.tier == FeedTier::Master
                                && !c.forced_light
                        })
                        .min_by_key(|c| c.config.priority)
                        .map(|c| (c.config.clone(), FeedTier::Master))
                        .or_else(|| {
                            map.values()
                                .filter(|c| {
                                    c.effective_tier == FeedTier::Light
                                        && c.config.tier == FeedTier::Simulcast
                                        && !c.forced_light
                                })
                                .min_by_key(|c| c.config.priority)
                                .map(|c| (c.config.clone(), FeedTier::Simulcast))
                        })
                };
                if let Some((ch, target_tier)) = recover_cand {
                    let admit_ok = self.governor.lock().await.admit_tier_upgrade().is_ok();
                    if admit_ok {
                        let st = Arc::clone(self);
                        tokio::spawn(async move {
                            if let Ok(resolved) = st
                                .resolver
                                .resolve(
                                    &ch.url,
                                    ch.platform,
                                    target_tier,
                                    ch.live_streams_only,
                                )
                                .await
                            {
                                st.mse_hubs.invalidate(&ch.id).await;
                                let expected_prev = if target_tier == FeedTier::Master {
                                    FeedTier::Simulcast
                                } else {
                                    FeedTier::Light
                                };
                                {
                                    let mut map = st.channels.write().await;
                                    if let Some(m) = map.get_mut(&ch.id)
                                        && m.effective_tier == expected_prev
                                        && !m.forced_light
                                    {
                                        m.effective_tier = target_tier;
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
                                            tier: target_tier,
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
        self.recompute_live_accounting_from_samples(&samples).await;
    }

    pub async fn recompute_live_accounting_totals(&self) {
        let samples: Vec<GridSample> = {
            let cache = self.grid_samples.read().await;
            cache.values().cloned().collect()
        };
        self.recompute_live_accounting_from_samples(&samples).await;
    }

    async fn recompute_live_accounting_from_samples(&self, samples: &[GridSample]) {
        let overrides = self.overrides.read().await.clone();
        let (show_channels, sched_start, sched_end) = {
            let s = self.show.read().await;
            (s.channels.clone(), s.scheduled_start_at, s.scheduled_end_at)
        };
        let merged = filter_samples_by_window(
            prepare_accounting_samples(
                &apply_overrides_to_samples(samples, &overrides),
                &show_channels,
            ),
            sched_start,
            sched_end,
        );
        let sum = summarize_grid(&merged);
        *self.combined_pcv.write().await = sum.combined_pcv;
        *self.hours_watched.write().await = sum.hours_watched;
    }
}
