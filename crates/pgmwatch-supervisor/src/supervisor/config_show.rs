use super::{new_managed_channel, SupervisorState};
use crate::worker_bridge::send_worker_cmd;
use chrono::Utc;
use pgmwatch_core::config::{resolve_channel_lufs_spec, PgmWatchConfig};
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::ipc::SupervisorToWorker;
use pgmwatch_core::models::{FeedTier, ShowPreset};
use std::path::Path;
use std::sync::Arc;

impl SupervisorState {
    pub async fn persist_show_preset(&self, preset: &ShowPreset) {
        write_preset_json(&self.data_dir.join("presets"), preset);
        if let Err(err) = self.db.upsert_show_now(preset).await {
            tracing::error!("Failed synchronous show persistence: {err}");
        }
        if let Err(err) = self.db.set_active_show_id_now(&preset.show_id).await {
            tracing::error!("Failed updating active_show_id: {err}");
        }
    }

    pub async fn set_channel_tier(&self, channel_id: &str, tier: FeedTier) -> Option<ShowPreset> {
        {
            let mut map = self.channels.write().await;
            if let Some(m) = map.get_mut(channel_id) {
                m.config.tier = tier;
                m.effective_tier = tier;
                m.forced_light = false;
            }
        }
        let updated_show = {
            let mut show = self.show.write().await;
            let ch = show.channels.iter_mut().find(|c| c.id == channel_id)?;
            ch.tier = tier;
            show.clone()
        };
        self.persist_show_preset(&updated_show).await;
        self.broadcast_snapshot().await;
        Some(updated_show)
    }

    pub async fn set_channel_lufs_target(
        &self,
        channel_id: &str,
        target_lufs: Option<f32>,
        lufs_tolerance: Option<f32>,
    ) -> Option<ShowPreset> {
        let alerts_cfg = self.cfg.read().await.alerts.clone();
        let updated_cfg = {
            let mut map = self.channels.write().await;
            let m = map.get_mut(channel_id)?;
            m.config.target_lufs = target_lufs;
            m.config.lufs_tolerance = lufs_tolerance;
            m.config.clone()
        };
        let (eff_target, eff_tol) = resolve_channel_lufs_spec(&updated_cfg, &alerts_cfg);

        {
            let mut workers = self.workers.lock().await;
            if let Some(w) = workers.get_mut(channel_id)
                && let Err(err) = send_worker_cmd(
                    &mut w.stdin,
                    &SupervisorToWorker::SetLufsTarget {
                        target_lufs: eff_target,
                        tolerance_lu: eff_tol,
                    },
                )
                .await
            {
                tracing::warn!(%channel_id, "Failed sending SetLufsTarget to worker: {err}");
            }
        }

        let updated_show = {
            let mut show = self.show.write().await;
            let ch = show.channels.iter_mut().find(|c| c.id == channel_id)?;
            ch.target_lufs = target_lufs;
            ch.lufs_tolerance = lufs_tolerance;
            show.clone()
        };
        self.persist_show_preset(&updated_show).await;
        self.broadcast_snapshot().await;
        Some(updated_show)
    }

    pub async fn reset_accounting(&self) {
        *self.active_match.write().await = None;
        self.matches.write().await.clear();
        self.overrides.write().await.clear();
        self.grid_samples.write().await.clear();
        *self.combined_pcv.write().await = 0;
        *self.hours_watched.write().await = 0.0;
        if let Err(err) = self.db.reset_accounting_now().await {
            tracing::error!("Failed resetting accounting tables: {err}");
        }
        self.broadcast_snapshot().await;
    }

    pub async fn apply_runtime_config(
        self: &Arc<Self>,
        next: PgmWatchConfig,
    ) -> Result<bool, String> {
        next.validate()?;
        let (media_changed, lufs_changed, restart_needed) = {
            let cur = self.cfg.read().await;
            let media_diff = cur.media.gpu_mode != next.media.gpu_mode
                || cur.media.master_gpu_luid != next.media.master_gpu_luid
                || cur.media.simulcast_gpu_luid != next.media.simulcast_gpu_luid
                || cur.media.extra_hw_frames != next.media.extra_hw_frames
                || (cur.media.clip_quota_gb - next.media.clip_quota_gb).abs() > f64::EPSILON
                || cur.server.outbound_bind_ip != next.server.outbound_bind_ip;
            let lufs_diff = cur.alerts.custom_target_lufs != next.alerts.custom_target_lufs
                || (cur.alerts.lufs_tolerance - next.alerts.lufs_tolerance).abs() > f32::EPSILON;
            let bind_diff = cur.server.bind_host != next.server.bind_host
                || cur.server.port != next.server.port
                || cur.storage.data_dir != next.storage.data_dir;
            (media_diff, lufs_diff, bind_diff)
        };

        let cfg_path = self.base_dir.join("pgmwatch.toml");
        next.save(&cfg_path)
            .map_err(|e| format!("Failed writing pgmwatch.toml: {e}"))?;

        self.governor
            .lock()
            .await
            .update_config(next.governor.clone());
        self.resolver
            .update_config(next.extraction.clone())
            .await;
        self.resolver
            .set_bind_ip(&next.server.outbound_bind_ip)
            .await;
        let next_alerts = next.alerts.clone();
        *self.cfg.write().await = next;

        if media_changed {
            self.mse_hubs.invalidate_all().await;
            let existing_ids: Vec<String> = self.workers.lock().await.keys().cloned().collect();
            for id in existing_ids {
                self.stop_worker(&id).await;
            }
            self.spawn_enabled_workers().await;
        } else if lufs_changed {
            let specs: Vec<(String, f32, f32)> = self
                .channels
                .read()
                .await
                .values()
                .map(|m| {
                    let (t, tol) = resolve_channel_lufs_spec(&m.config, &next_alerts);
                    (m.config.id.clone(), t, tol)
                })
                .collect();
            let mut workers = self.workers.lock().await;
            for (cid, t, tol) in specs {
                if let Some(w) = workers.get_mut(&cid) {
                    let _ = send_worker_cmd(
                        &mut w.stdin,
                        &SupervisorToWorker::SetLufsTarget {
                            target_lufs: t,
                            tolerance_lu: tol,
                        },
                    )
                    .await;
                }
            }
            drop(workers);
            self.broadcast_snapshot().await;
        }
        Ok(restart_needed)
    }

    pub async fn apply_show_preset(self: &Arc<Self>, preset: ShowPreset) {
        self.mse_hubs.invalidate_all().await;
        self.resolved_media.write().await.clear();
        *self.active_pfl_channel.write().await = None;
        let existing_ids: Vec<String> = self.workers.lock().await.keys().cloned().collect();
        for id in existing_ids {
            self.stop_worker(&id).await;
        }
        let now = Utc::now();
        {
            let mut ghw = self.group_hold_windows.write().await;
            ghw.clear();
            for g in &preset.groups {
                if g.intermission_active {
                    ghw.insert(g.id.clone(), vec![(now, None)]);
                }
            }
        }
        self.master_lufs_timeline.write().await.clear();
        self.tiktok_cache.write().await.clear();
        self.chat_rates.write().await.clear();
        self.chat_spike_cooldown.lock().await.clear();
        self.mode_b_cache.write().await.clear();
        {
            let mut map = self.channels.write().await;
            map.clear();
            for ch in &preset.channels {
                map.insert(ch.id.clone(), new_managed_channel(ch.clone()));
            }
        }
        {
            let mut incidents = self.incidents.write().await;
            for inc in incidents.iter_mut() {
                if inc.resolved_at.is_none() {
                    inc.resolved_at = Some(now);
                    inc.acknowledged = true;
                    inc.duration_secs = (now - inc.started_at).num_milliseconds() as f64 / 1000.0;
                    self.db.enqueue(DbCommand::UpsertIncident(inc.clone())).await;
                }
            }
        }
        self.persist_show_preset(&preset).await;
        *self.show.write().await = preset;
        self.spawn_enabled_workers().await;
        self.sync_tiktok_subscriptions().await;
        self.sync_chat_subscriptions().await;
        self.stats_notify.notify_one();
        self.broadcast_snapshot().await;
    }

    pub async fn active_segment_label(&self) -> Option<String> {
        self.active_match.read().await.as_ref().map(|m| m.label.clone())
    }

    pub async fn switch_show_preset(self: &Arc<Self>, show_id: &str) -> Result<(), (u16, String)> {
        if let Some(label) = self.active_segment_label().await {
            return Err((
                409,
                format!(
                    "Segment \"{label}\" is currently recording. End the segment in Broadcast Accounting before switching shows."
                ),
            ));
        }
        let target = self
            .db
            .get_show_now(show_id)
            .await
            .map_err(|e| (500, format!("Failed reading show preset: {e}")))?
            .ok_or_else(|| (404, format!("Show preset '{show_id}' not found")))?;
        self.apply_show_preset(target).await;
        Ok(())
    }

    pub async fn rename_show_title(self: &Arc<Self>, title: String) {
        let updated = {
            let mut guard = self.show.write().await;
            guard.title = title;
            guard.clone()
        };
        self.persist_show_preset(&updated).await;
        self.broadcast_snapshot().await;
    }

    pub async fn reload_channel_worker(self: &Arc<Self>, channel_id: &str) {
        let ch_opt = self
            .channels
            .read()
            .await
            .get(channel_id)
            .map(|m| m.config.clone());
        if let Some(ch) = ch_opt {
            self.start_or_restart_worker(ch).await;
        }
    }
}

pub(super) fn write_preset_json(presets_dir: &Path, preset: &ShowPreset) {
    let target = presets_dir.join("active-show.json");
    let tmp = presets_dir.join("active-show.json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(preset)
        && std::fs::write(&tmp, json).is_ok()
        && std::fs::rename(&tmp, &target).is_err()
    {
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::rename(&tmp, &target);
    }
}

pub(super) fn load_newest_preset_file(presets_dir: &Path) -> Option<(ShowPreset, chrono::DateTime<Utc>)> {
    let entries = std::fs::read_dir(presets_dir).ok()?;
    let mut best: Option<(ShowPreset, chrono::DateTime<Utc>)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<ShowPreset>(&raw) else {
            continue;
        };
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(chrono::DateTime::<Utc>::from)
            .unwrap_or(parsed.created_at);
        if best.as_ref().is_none_or(|(_, bt)| mtime > *bt) {
            best = Some((parsed, mtime));
        }
    }
    best
}
