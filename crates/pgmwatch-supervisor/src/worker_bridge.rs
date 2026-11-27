use crate::supervisor::{SupervisorState, WsBroadcast};
use chrono::Utc;
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::ipc::{SupervisorToWorker, WorkerToSupervisor};
use pgmwatch_core::models::{
    AlertState, ChannelConfig, FaultKind, FeedTier, IncidentRecord, MetricStatus, Platform,
    TriStateMetric,
};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use uuid::Uuid;

pub(crate) struct WorkerHandle {
    pub(crate) stdin: ChildStdin,
    pub(crate) child: Child,
    pub(crate) gpu_fault_timestamps: VecDeque<chrono::DateTime<Utc>>,
}

impl SupervisorState {
    pub async fn sync_tiktok_subscriptions(&self) {
        let desired: Vec<(String, String)> = self
            .show
            .read()
            .await
            .channels
            .iter()
            .filter(|ch| ch.enabled && ch.platform == Platform::Tiktok && !ch.url.starts_with("sim://"))
            .filter_map(|ch| {
                crate::stats::extract_tiktok_username(&ch.url).map(|user| (ch.id.clone(), user))
            })
            .collect();

        if let Some(tx) = self.tiktok_cmd_tx.lock().await.clone() {
            let _ = tx.send(desired).await;
        }
    }

    pub async fn spawn_tiktok_bridge(self: &Arc<Self>) {
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<(String, String)>>(32);
        *self.tiktok_cmd_tx.lock().await = Some(cmd_tx);
        self.sync_tiktok_subscriptions().await;

        let tt_state = Arc::clone(self);
        tokio::spawn(async move {
            let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel(256);
            let node_exe = tt_state.bin_dir.join("node.exe");
            let sidecar_js = tt_state.base_dir.join("sidecar-stats").join("index.js");
            let mut client = crate::stats::TikTokSidecarClient::spawn(
                &node_exe,
                &sidecar_js,
                ev_tx,
                Some(&tt_state.job_guard),
            );
            let mut active_rooms: HashMap<String, String> = HashMap::new();
            let mut shutdown_rx = tt_state.subscribe_shutdown();

            loop {
                if *shutdown_rx.borrow() {
                    break;
                }
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                    maybe_desired = cmd_rx.recv() => {
                        let Some(desired_list) = maybe_desired else {
                            break;
                        };
                        let desired: HashMap<String, String> = desired_list.into_iter().collect();
                        let to_remove: Vec<String> = active_rooms
                            .iter()
                            .filter(|(id, user)| desired.get(*id) != Some(*user))
                            .map(|(id, _)| id.clone())
                            .collect();
                        for id in to_remove {
                            client.unsubscribe(&id).await;
                            active_rooms.remove(&id);
                            tt_state.tiktok_cache.write().await.remove(&id);
                        }
                        let tt_cookies = tt_state
                            .profiles_dir
                            .join(crate::resolver::auth_cookies_slug(Platform::Tiktok));
                        let session_id = crate::stats::extract_cookie_value(
                            &tt_cookies,
                            "tiktok.com",
                            "sessionid",
                        );
                        for (id, user) in desired {
                            if active_rooms.get(&id) != Some(&user) {
                                client.subscribe(&id, &user, session_id.as_deref()).await;
                                active_rooms.insert(id, user);
                            }
                        }
                    }
                    maybe_ev = ev_rx.recv() => {
                        let Some(ev) = maybe_ev else {
                            break;
                        };
                        match ev {
                            crate::stats::SidecarEvent::Stats {
                                channel_id,
                                ccv,
                                total_views,
                                likes,
                                shares,
                                gifts,
                                chat_count,
                            } => {
                                let upd = crate::stats::ChannelStatsUpdate {
                                    channel_id: channel_id.clone(),
                                    ccv: ccv
                                        .map(|v| TriStateMetric::value(v, 100.0))
                                        .unwrap_or_else(|| TriStateMetric::unavailable("TikTok CCV missing")),
                                    total_views: total_views
                                        .map(|v| TriStateMetric::value(v, 100.0))
                                        .unwrap_or_else(|| TriStateMetric::unavailable("Awaiting TikTok total views")),
                                    likes: likes
                                        .map(|v| TriStateMetric::value(v, 100.0))
                                        .unwrap_or_else(|| TriStateMetric::unavailable("Awaiting TikTok like count")),
                                    chat_rate_per_min: chat_count
                                        .map(|v| TriStateMetric::value(v, 95.0))
                                        .unwrap_or_else(TriStateMetric::na),
                                    shares: shares
                                        .map(|v| TriStateMetric::value(v, 100.0))
                                        .unwrap_or_else(|| TriStateMetric::unavailable("Awaiting TikTok share count")),
                                    gifts_count: gifts
                                        .map(|v| TriStateMetric::value(v, 100.0))
                                        .unwrap_or_else(|| TriStateMetric::unavailable("Awaiting TikTok gift count")),
                                    extras: HashMap::new(),
                                };
                                let became_valued = {
                                    let mut cache = tt_state.tiktok_cache.write().await;
                                    let prev_valued = cache
                                        .get(&channel_id)
                                        .is_some_and(|p| p.ccv.status == MetricStatus::Value);
                                    cache.insert(channel_id.clone(), upd.clone());
                                    !prev_valued && upd.ccv.status == MetricStatus::Value
                                };
                                {
                                    let mut map = tt_state.channels.write().await;
                                    if let Some(m) = map.get_mut(&channel_id) {
                                        m.ccv = upd.ccv;
                                        m.total_views = upd.total_views;
                                        m.likes = upd.likes;
                                        m.shares = upd.shares;
                                        m.chat_rate_per_min = upd.chat_rate_per_min;
                                        m.gifts_count = upd.gifts_count;
                                    }
                                }
                                if became_valued {
                                    tt_state.stats_notify.notify_one();
                                }
                            }
                            crate::stats::SidecarEvent::Chat {
                                channel_id,
                                viewer_id,
                                text,
                            } => {
                                tt_state
                                    .ingest_chat_complaint_message(&channel_id, &viewer_id, &text)
                                    .await;
                            }
                            crate::stats::SidecarEvent::Error { channel_id, message } => {
                                tracing::debug!("TikTok sidecar error on {channel_id}: {message}");
                            }
                        }
                    }
                }
            }
            client.stop().await;
        });
    }

    pub async fn spawn_enabled_workers(self: &Arc<Self>) {
        let channels: Vec<ChannelConfig> = self.show.read().await.channels.clone();
        for ch in channels {
            if ch.enabled {
                let st = Arc::clone(self);
                tokio::spawn(async move {
                    st.start_or_restart_worker(ch).await;
                });
            }
        }
    }

    pub fn start_or_restart_worker(
        self: &Arc<Self>,
        ch: ChannelConfig,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(async move {
            self.stop_worker(&ch.id).await;

            {
                let mut map = self.channels.write().await;
                if let Some(m) = map.get_mut(&ch.id) {
                    m.tracker
                        .set_external_state(AlertState::Reacquiring, "Resolving media URL");
                }
            }

            let effective_tier = {
                let map = self.channels.read().await;
                map.get(&ch.id).map(|m| m.effective_tier).unwrap_or(ch.tier)
            };

            let resolved = match self
                .resolver
                .resolve(&ch.url, ch.platform, effective_tier, ch.live_streams_only)
                .await
            {
                Ok(r) => r,
                Err(err) => {
                    let (state, msg) = match err {
                        crate::resolver::ResolveError::LoginGated(m) => (AlertState::LoginGated, m),
                        crate::resolver::ResolveError::Offline(m) => (AlertState::Offline, m),
                        crate::resolver::ResolveError::ExtractorFailed(m) => {
                            (AlertState::ExtractorErr, m)
                        }
                    };
                    let mut map = self.channels.write().await;
                    if let Some(m) = map.get_mut(&ch.id) {
                        m.tracker.set_external_state(state, msg);
                    }
                    return;
                }
            };

            // Channel may have been paused, deleted, edited, or switched to another show while resolving.
            let still_active = {
                let map = self.channels.read().await;
                map.get(&ch.id)
                    .is_some_and(|m| m.config.enabled && m.config.url == ch.url)
            };
            if !still_active {
                tracing::debug!(
                    channel_id = %ch.id,
                    "Skipping worker spawn after resolve: channel paused, removed, or URL changed"
                );
                return;
            }

            let cfg_snap = self.cfg.read().await.clone();
            let target_luid = match effective_tier {
                FeedTier::Master => &cfg_snap.media.master_gpu_luid,
                _ => &cfg_snap.media.simulcast_gpu_luid,
            };
            let hwaccel_device = if !target_luid.trim().is_empty() {
                let adapters = crate::win32::query_dxgi_adapters();
                crate::win32::resolve_dxgi_adapter_index_for_luid(&adapters, target_luid)
            } else {
                None
            };

            let (target_lufs, lufs_tolerance) =
                pgmwatch_core::config::resolve_channel_lufs_spec(&ch, &cfg_snap.alerts);
            let target_lufs_arg = format!("--target-lufs={target_lufs}");
            let lufs_tol_arg = format!("--lufs-tolerance={lufs_tolerance}");

            let worker_exe = find_worker_exe(&self.base_dir);
            let mut cmd = Command::new(&worker_exe);
            use pgmwatch_core::process::BackgroundCommandExt;
            cmd.no_window();
            cmd.kill_on_drop(true);
            cmd.args([
                "--channel-id",
                &ch.id,
                "--url",
                &resolved.stream_url,
                "--platform",
                serde_json::to_string(&ch.platform)
                    .unwrap_or_else(|_| "\"custom\"".into())
                    .trim_matches('"'),
                "--tier",
                effective_tier.as_str(),
                "--clips-dir",
                &self.clips_dir.display().to_string(),
                "--clip-quota-gb",
                &cfg_snap.media.clip_quota_gb.to_string(),
                "--bind-ip",
                &cfg_snap.server.outbound_bind_ip,
                "--gpu-mode",
                &cfg_snap.media.gpu_mode,
                "--extra-hw-frames",
                &cfg_snap.media.extra_hw_frames.to_string(),
                &target_lufs_arg,
                &lufs_tol_arg,
            ]);
            if let Some(ref dev) = hwaccel_device {
                cmd.args(["--hwaccel-device", dev]);
            }
            if let Some(ref audio_url) = resolved.audio_url {
                cmd.args(["--audio-url", audio_url]);
            }
            if !resolved.headers.is_empty()
                && let Ok(hdr_json) = serde_json::to_string(&resolved.headers)
            {
                cmd.args(["--headers-json", &hdr_json]);
            }
            if ch.orientation_vertical {
                cmd.arg("--vertical");
            }
            cmd.stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(err) => {
                    tracing::error!("Failed to spawn pgmwatch-worker for {}: {err}", ch.id);
                    let mut map = self.channels.write().await;
                    if let Some(m) = map.get_mut(&ch.id) {
                        m.tracker.set_external_state(
                            AlertState::ExtractorErr,
                            format!("Worker spawn failed: {err}"),
                        );
                    }
                    return;
                }
            };

            if let Some(pid) = child.id() {
                let _ = self.job_guard.assign_pid(pid);
            }

            let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                let _ = child.kill().await;
                let mut map = self.channels.write().await;
                if let Some(m) = map.get_mut(&ch.id) {
                    m.tracker.set_external_state(
                        AlertState::ExtractorErr,
                        "Worker failed to open stdio pipes",
                    );
                }
                return;
            };
            if let Some(stderr) = child.stderr.take() {
                let err_ch_id = ch.id.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        if line.contains("WARN") || line.contains("ERROR") {
                            tracing::warn!("worker[{err_ch_id}]: {line}");
                        } else {
                            tracing::debug!("worker[{err_ch_id}]: {line}");
                        }
                    }
                });
            }

            {
                let map = self.channels.read().await;
                let still_valid = map
                    .get(&ch.id)
                    .is_some_and(|m| m.config.enabled && m.config.url == ch.url);
                if !still_valid {
                    let _ = child.start_kill();
                    return;
                }
            }
            {
                let mut map = self.channels.write().await;
                if let Some(m) = map.get_mut(&ch.id) {
                    m.resolved_via = Some(resolved.via.clone());
                    m.tracker.clear_external_state();
                }
            }
            self.resolved_media
                .write()
                .await
                .insert(ch.id.clone(), resolved);
            self.mse_hubs.invalidate(&ch.id).await;

            {
                let mut workers = self.workers.lock().await;
                let prev_faults = workers
                    .get(&ch.id)
                    .map(|w| w.gpu_fault_timestamps.clone())
                    .unwrap_or_default();
                if let Some(mut prev) = workers.insert(
                    ch.id.clone(),
                    WorkerHandle {
                        stdin,
                        child,
                        gpu_fault_timestamps: prev_faults,
                    },
                ) {
                    let _ = prev.child.start_kill();
                }
            }

            if self.active_pfl_channel.read().await.as_deref() == Some(ch.id.as_str()) {
                let mut workers = self.workers.lock().await;
                if let Some(w) = workers.get_mut(&ch.id) {
                    let _ = send_worker_cmd(
                        &mut w.stdin,
                        &SupervisorToWorker::SetPfl { enabled: true },
                    )
                    .await;
                }
            }

            let st = Arc::clone(self);
            let ch_id = ch.id.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(msg) = serde_json::from_str::<WorkerToSupervisor>(&line) {
                        st.handle_worker_message(msg).await;
                    }
                }
                tracing::debug!("Worker stdout closed for {ch_id}");
            });
        })
    }

    pub async fn stop_worker(&self, channel_id: &str) {
        self.mse_hubs.invalidate(channel_id).await;
        self.resolved_media.write().await.remove(channel_id);
        let mut workers = self.workers.lock().await;
        if let Some(mut handle) = workers.remove(channel_id) {
            let _ = send_worker_cmd(&mut handle.stdin, &SupervisorToWorker::Shutdown).await;
            let _ = handle.child.kill().await;
            let _ = handle.child.wait().await;
        }
    }

    async fn handle_worker_message(self: &Arc<Self>, msg: WorkerToSupervisor) {
        match msg {
            WorkerToSupervisor::Ready { channel_id, .. } => {
                let mut map = self.channels.write().await;
                if let Some(m) = map.get_mut(&channel_id) {
                    m.tracker.clear_external_state();
                    if matches!(m.tracker.state, AlertState::Offline | AlertState::Reacquiring) {
                        m.tracker.state = AlertState::Reacquiring;
                        m.tracker.status_reason = Some("Connecting media stream".to_string());
                    }
                }
            }
            WorkerToSupervisor::Thumbnail {
                channel_id,
                jpeg_b64,
                timestamp,
            } => {
                {
                    let mut map = self.channels.write().await;
                    if let Some(m) = map.get_mut(&channel_id) {
                        m.thumbnail_b64 = Some(jpeg_b64.clone());
                        m.thumbnail_updated_at = Some(timestamp);
                    }
                }
                let _ = self.ws_tx.send(WsBroadcast::Thumbnail {
                    channel_id,
                    jpeg_b64,
                });
            }
            WorkerToSupervisor::Meter { channel_id, audio } => {
                {
                    let mut map = self.channels.write().await;
                    if let Some(m) = map.get_mut(&channel_id) {
                        m.audio = audio.clone();
                    }
                }
                let _ = self.ws_tx.send(WsBroadcast::Meter { channel_id, audio });
            }
            WorkerToSupervisor::PflAudio {
                channel_id,
                sample_rate,
                channels,
                pcm_s16le_b64,
                ..
            } => {
                let active = self.active_pfl_channel.read().await.clone();
                if active.as_deref() == Some(channel_id.as_str()) {
                    let _ = self.ws_tx.send(WsBroadcast::PflAudio {
                        channel_id,
                        sample_rate,
                        channels,
                        pcm_s16le_b64,
                    });
                } else {
                    tracing::debug!(%channel_id, ?active, "Supervisor dropped stale PflAudio");
                }
            }
            WorkerToSupervisor::ClipSaved {
                channel_id,
                fault,
                filename,
                duration_secs: _,
                saved_at,
            } => {
                let mut incidents = self.incidents.write().await;
                let inc_to_emit = if let Some(inc) = incidents
                    .iter_mut()
                    .find(|i| i.channel_id == channel_id && i.resolved_at.is_none())
                {
                    inc.clip_filename = Some(filename.clone());
                    inc.clone()
                } else {
                    let ch_name = self
                        .channels
                        .read()
                        .await
                        .get(&channel_id)
                        .map(|c| (c.config.name.clone(), c.config.group_id.clone()))
                        .unwrap_or_else(|| (channel_id.clone(), "default".into()));
                    let manual_inc = IncidentRecord {
                        id: Uuid::new_v4().to_string(),
                        channel_id,
                        channel_name: ch_name.0,
                        group_id: ch_name.1,
                        fault: fault.unwrap_or(FaultKind::OperatorBookmark),
                        severity: if fault.is_some() {
                            AlertState::Warning
                        } else {
                            AlertState::Healthy
                        },
                        started_at: saved_at,
                        resolved_at: Some(saved_at),
                        duration_secs: 5.0,
                        clip_filename: Some(filename),
                        acknowledged: true,
                        detail: if fault.is_some() {
                            "Triggered evidence clip".into()
                        } else {
                            "Operator instant replay clip".into()
                        },
                    };
                    incidents.insert(0, manual_inc.clone());
                    manual_inc
                };
                self.db
                    .enqueue(DbCommand::UpsertIncident(inc_to_emit.clone()))
                    .await;
                let _ = self
                    .ws_tx
                    .send(WsBroadcast::Incident { incident: inc_to_emit });
            }
            WorkerToSupervisor::StreamError {
                channel_id,
                message,
                token_expired,
                gpu_fault,
                ..
            } => {
                if token_expired {
                    let ch_info = {
                        let map = self.channels.read().await;
                        map.get(&channel_id)
                            .map(|m| (m.config.clone(), m.effective_tier))
                    };
                    if let Some((ch, eff_tier)) = ch_info {
                        let st = Arc::clone(self);
                        tokio::spawn(async move {
                            st.resolver.invalidate_url(&ch.url).await;
                            match st
                                .resolver
                                .resolve(&ch.url, ch.platform, eff_tier, ch.live_streams_only)
                                .await
                            {
                                Ok(resolved) => {
                                    let still_valid = st
                                        .channels
                                        .read()
                                        .await
                                        .get(&ch.id)
                                        .is_some_and(|m| m.config.enabled && m.config.url == ch.url);
                                    if !still_valid {
                                        return;
                                    }
                                    st.mse_hubs.invalidate(&ch.id).await;
                                    {
                                        let mut workers = st.workers.lock().await;
                                        if let Some(w) = workers.get_mut(&ch.id) {
                                            let _ = send_worker_cmd(
                                                &mut w.stdin,
                                                &SupervisorToWorker::UpdateStream {
                                                    resolved_url: resolved.stream_url.clone(),
                                                    audio_url: resolved.audio_url.clone(),
                                                    headers: resolved.headers.clone(),
                                                    tier: eff_tier,
                                                },
                                            )
                                            .await;
                                        }
                                    }
                                    st.resolved_media
                                        .write()
                                        .await
                                        .insert(ch.id.clone(), resolved);
                                    if let Some(m) = st.channels.write().await.get_mut(&ch.id) {
                                        m.tracker.clear_external_state();
                                    }
                                }
                                Err(err) => {
                                    let target_state = match err {
                                        crate::resolver::ResolveError::LoginGated(_) => AlertState::LoginGated,
                                        crate::resolver::ResolveError::Offline(_) => AlertState::Offline,
                                        crate::resolver::ResolveError::ExtractorFailed(_) => AlertState::ExtractorErr,
                                    };
                                    let mut map = st.channels.write().await;
                                    if let Some(m) = map.get_mut(&ch.id)
                                        && m.config.enabled
                                        && m.config.url == ch.url
                                    {
                                        m.tracker.set_external_state(target_state, err.to_string());
                                    }
                                }
                            }
                        });
                    }
                    return;
                }

                if gpu_fault {
                    let mut lock_to_light = false;
                    {
                        let mut workers = self.workers.lock().await;
                        if let Some(w) = workers.get_mut(&channel_id) {
                            let now = Utc::now();
                            w.gpu_fault_timestamps.push_back(now);
                            while let Some(front) = w.gpu_fault_timestamps.front() {
                                if now - *front > chrono::Duration::seconds(60) {
                                    w.gpu_fault_timestamps.pop_front();
                                } else {
                                    break;
                                }
                            }
                            if w.gpu_fault_timestamps.len() >= 2 {
                                lock_to_light = true;
                            }
                        }
                    }
                    if lock_to_light {
                        let ch_opt = {
                            let mut map = self.channels.write().await;
                            map.get_mut(&channel_id).map(|m| {
                                m.forced_light = true;
                                m.effective_tier = FeedTier::Light;
                                m.config.clone()
                            })
                        };
                        if let Some(ch) = ch_opt {
                            let st = Arc::clone(self);
                            tokio::spawn(async move {
                                Box::pin(st.start_or_restart_worker(ch)).await;
                            });
                        }
                    }
                } else {
                    let mut map = self.channels.write().await;
                    if let Some(m) = map.get_mut(&channel_id) {
                        m.tracker.set_external_state(AlertState::ExtractorErr, message);
                    }
                }
            }
            WorkerToSupervisor::Telemetry {
                channel_id,
                timestamp,
                audio,
                video,
                active_faults,
                ad_break,
                rx_kbps,
            } => {
                let alerts_cfg = self.cfg.read().await.alerts.clone();
                let mut clip_fault: Option<FaultKind> = None;
                let mut new_incident: Option<IncidentRecord> = None;
                let mut resolved_inc: Option<IncidentRecord> = None;
                let mut master_lufs_update: Option<(String, u32, f32)> = None;

                let (group_id_for_ch, delay_secs_for_ch) = {
                    let map = self.channels.read().await;
                    map.get(&channel_id)
                        .map(|m| (m.config.group_id.clone(), m.config.broadcast_delay_secs))
                        .unwrap_or_else(|| ("default".into(), 0))
                };
                let interm = self
                    .effective_intermission_for(&group_id_for_ch, delay_secs_for_ch, timestamp)
                    .await;

                {
                    let mut map = self.channels.write().await;
                    if let Some(m) = map.get_mut(&channel_id) {
                        m.audio = audio.clone();
                        m.video = video;
                        m.rx_kbps = rx_kbps;
                        m.updated_at = timestamp;
                        if m.config.tier == FeedTier::Master {
                            master_lufs_update = Some((
                                m.config.group_id.clone(),
                                m.config.broadcast_delay_secs,
                                audio.short_term_lufs,
                            ));
                        }

                        let tr = m.tracker.evaluate(
                            timestamp,
                            &active_faults,
                            ad_break,
                            interm,
                            &alerts_cfg,
                        );
                        clip_fault = tr.clip_trigger_fault;

                        if let Some(fault) = tr.opened_incident_fault {
                            let id = Uuid::new_v4().to_string();
                            m.active_incident_id = Some(id.clone());
                            new_incident = Some(IncidentRecord {
                                id,
                                channel_id: m.config.id.clone(),
                                channel_name: m.config.name.clone(),
                                group_id: m.config.group_id.clone(),
                                fault,
                                severity: tr.new_state,
                                started_at: timestamp,
                                resolved_at: None,
                                duration_secs: 0.0,
                                clip_filename: None,
                                acknowledged: false,
                                detail: fault.label().to_string(),
                            });
                        } else if tr.resolved_incident {
                            if let Some(inc_id) = m.active_incident_id.take() {
                                let mut incidents = self.incidents.write().await;
                                if let Some(inc) = incidents.iter_mut().find(|i| i.id == inc_id) {
                                    inc.resolved_at = Some(timestamp);
                                    inc.duration_secs =
                                        (timestamp - inc.started_at).num_milliseconds() as f64
                                            / 1000.0;
                                    resolved_inc = Some(inc.clone());
                                }
                            }
                        } else if matches!(tr.new_state, AlertState::Critical)
                            && let Some(ref inc_id) = m.active_incident_id
                        {
                            let mut incidents = self.incidents.write().await;
                            if let Some(inc) = incidents.iter_mut().find(|i| i.id == *inc_id) {
                                inc.severity = AlertState::Critical;
                                inc.duration_secs =
                                    (timestamp - inc.started_at).num_milliseconds() as f64
                                        / 1000.0;
                            }
                        }
                    }
                }

                if let Some(fault) = clip_fault {
                    let mut workers = self.workers.lock().await;
                    if let Some(w) = workers.get_mut(&channel_id) {
                        let _ = send_worker_cmd(
                            &mut w.stdin,
                            &SupervisorToWorker::TriggerFaultClip {
                                fault,
                                detail: fault.label().to_string(),
                            },
                        )
                        .await;
                    }
                }

                if let Some(inc) = new_incident {
                    self.incidents.write().await.insert(0, inc.clone());
                    self.db.enqueue(DbCommand::UpsertIncident(inc.clone())).await;
                    let _ = self.ws_tx.send(WsBroadcast::Incident { incident: inc });
                }
                if let Some(inc) = resolved_inc {
                    self.db.enqueue(DbCommand::UpsertIncident(inc.clone())).await;
                    let _ = self.ws_tx.send(WsBroadcast::Incident { incident: inc });
                }

                if let Some((gid, master_delay, master_lufs)) = master_lufs_update {
                    {
                        let mut timeline_map = self.master_lufs_timeline.write().await;
                        let q = timeline_map.entry(gid.clone()).or_default();
                        q.push_back((timestamp, master_lufs));
                        let cutoff = timestamp - chrono::Duration::hours(3);
                        while let Some((ts, _)) = q.front()
                            && *ts < cutoff
                        {
                            q.pop_front();
                        }
                    }

                    let sims: Vec<(String, u32)> = self
                        .channels
                        .read()
                        .await
                        .values()
                        .filter(|c| c.config.group_id == gid && c.config.tier != FeedTier::Master)
                        .map(|c| (c.config.id.clone(), c.config.broadcast_delay_secs))
                        .collect();

                    let timeline_guard = self.master_lufs_timeline.read().await;
                    let group_hist = timeline_guard.get(&gid);
                    let mut targets_lufs = Vec::with_capacity(sims.len());
                    for (sid, sim_delay) in sims {
                        let delta_secs = i64::from(sim_delay) - i64::from(master_delay);
                        let ref_lufs = if delta_secs.abs() <= 10 {
                            Some(master_lufs)
                        } else {
                            let target_ts = timestamp - chrono::Duration::seconds(delta_secs);
                            group_hist.and_then(|q| {
                                q.iter()
                                    .min_by_key(|(ts, _)| (*ts - target_ts).num_milliseconds().abs())
                                    .filter(|(ts, _)| (*ts - target_ts).num_seconds().abs() <= 15)
                                    .map(|(_, val)| *val)
                            })
                        };
                        targets_lufs.push((sid, ref_lufs));
                    }
                    drop(timeline_guard);

                    let mut workers = self.workers.lock().await;
                    for (sid, ref_lufs) in targets_lufs {
                        if let Some(w) = workers.get_mut(&sid) {
                            let _ = send_worker_cmd(
                                &mut w.stdin,
                                &SupervisorToWorker::SetGroupReferenceLufs { lufs: ref_lufs },
                            )
                            .await;
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn find_worker_exe(base_dir: &Path) -> PathBuf {
    if let Ok(curr) = std::env::current_exe()
        && let Some(parent) = curr.parent()
    {
        let sibling = parent.join("pgmwatch-worker.exe");
        if sibling.exists() {
            return sibling;
        }
        let sibling_unix = parent.join("pgmwatch-worker");
        if sibling_unix.exists() {
            return sibling_unix;
        }
    }
    let rel = base_dir.join("pgmwatch-worker.exe");
    if rel.exists() {
        rel
    } else {
        PathBuf::from("pgmwatch-worker")
    }
}

pub(crate) async fn send_worker_cmd(
    stdin: &mut ChildStdin,
    cmd: &SupervisorToWorker,
) -> anyhow::Result<()> {
    let raw = serde_json::to_string(cmd)?;
    stdin.write_all(raw.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    Ok(())
}
