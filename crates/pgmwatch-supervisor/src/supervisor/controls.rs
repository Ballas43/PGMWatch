use super::{HoldWindow, SupervisorState, WsBroadcast, HOLD_END_GRACE_SECS};
use crate::worker_bridge::send_worker_cmd;
use chrono::Utc;
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::ipc::SupervisorToWorker;
use pgmwatch_core::models::{AlertState, FaultKind, IncidentRecord, MatchMarker};
use std::sync::Arc;
use uuid::Uuid;

impl SupervisorState {
    pub async fn ack_channel(&self, channel_id: &str) {
        {
            let mut map = self.channels.write().await;
            if let Some(m) = map.get_mut(channel_id) {
                m.tracker.acknowledge();
            }
        }
        let mut incidents = self.incidents.write().await;
        for inc in incidents.iter_mut() {
            if inc.channel_id == channel_id && !inc.acknowledged {
                inc.acknowledged = true;
                self.db.enqueue(DbCommand::AcknowledgeIncident(inc.id.clone())).await;
            }
        }
    }

    /// Acknowledge all active alerts and unacknowledged incidents in one operator action.
    pub async fn ack_all_channels(&self) -> usize {
        let mut acked_channels = 0usize;
        {
            let mut map = self.channels.write().await;
            for m in map.values_mut() {
                if matches!(m.tracker.state, AlertState::Warning | AlertState::Critical) {
                    acked_channels += 1;
                }
                m.tracker.acknowledge();
            }
        }
        let mut incidents = self.incidents.write().await;
        for inc in incidents.iter_mut() {
            if !inc.acknowledged {
                inc.acknowledged = true;
                self.db
                    .enqueue(DbCommand::AcknowledgeIncident(inc.id.clone()))
                    .await;
            }
        }
        drop(incidents);
        self.broadcast_snapshot().await;
        acked_channels
    }

    /// Enable or pause monitoring for a single feed directly from the live dashboard.
    pub async fn set_channel_enabled(
        self: &Arc<Self>,
        channel_id: &str,
        enabled: bool,
    ) -> Option<pgmwatch_core::models::ShowPreset> {
        let updated_cfg = {
            let mut map = self.channels.write().await;
            let m = map.get_mut(channel_id)?;
            m.config.enabled = enabled;
            if !enabled {
                m.tracker.set_external_state(AlertState::Offline, "Paused by operator");
            } else {
                m.tracker.set_external_state(AlertState::Reacquiring, "Resuming monitor");
            }
            m.config.clone()
        };
        let updated_show = {
            let mut show = self.show.write().await;
            let ch = show.channels.iter_mut().find(|c| c.id == channel_id)?;
            ch.enabled = enabled;
            show.clone()
        };
        self.persist_show_preset(&updated_show).await;
        if enabled {
            self.start_or_restart_worker(updated_cfg).await;
            self.stats_notify.notify_one();
        } else {
            self.mse_hubs.invalidate(channel_id).await;
            self.stop_worker(channel_id).await;
        }
        self.sync_tiktok_subscriptions().await;
        self.sync_chat_subscriptions().await;
        self.broadcast_snapshot().await;
        Some(updated_show)
    }

    /// Re-resolve and restart all enabled channels currently in Offline, ExtractorErr, or LoginGated state.
    pub async fn retry_offline_channels(self: &Arc<Self>) -> usize {
        let targets: Vec<(String, String)> = {
            let map = self.channels.read().await;
            map.values()
                .filter(|m| {
                    m.config.enabled
                        && matches!(
                            m.tracker.state,
                            AlertState::Offline | AlertState::ExtractorErr | AlertState::LoginGated
                        )
                })
                .map(|m| (m.config.id.clone(), m.config.url.clone()))
                .collect()
        };
        let count = targets.len();
        for (cid, url) in targets {
            self.resolver.invalidate_url(&url).await;
            let st = Arc::clone(self);
            tokio::spawn(async move {
                st.reload_channel_worker(&cid).await;
            });
        }
        count
    }

    /// Record a neutral timestamped operator note/cue into the event log & Excel export.
    pub async fn add_operator_bookmark(
        &self,
        channel_id: Option<&str>,
        note: String,
    ) -> IncidentRecord {
        let now = Utc::now();
        let (cid, cname, gid) = if let Some(target_id) = channel_id {
            let map = self.channels.read().await;
            if let Some(m) = map.get(target_id) {
                (m.config.id.clone(), m.config.name.clone(), m.config.group_id.clone())
            } else {
                (target_id.to_string(), target_id.to_string(), "general".to_string())
            }
        } else {
            ("show".to_string(), "Show-Wide".to_string(), "general".to_string())
        };
        let inc = IncidentRecord {
            id: Uuid::new_v4().to_string(),
            channel_id: cid,
            channel_name: cname,
            group_id: gid,
            fault: FaultKind::OperatorBookmark,
            severity: AlertState::Healthy,
            started_at: now,
            resolved_at: Some(now),
            duration_secs: 0.0,
            clip_filename: None,
            acknowledged: true,
            detail: note,
        };
        self.incidents.write().await.insert(0, inc.clone());
        self.db.enqueue(DbCommand::UpsertIncident(inc.clone())).await;
        let _ = self.ws_tx.send(WsBroadcast::Incident {
            incident: inc.clone(),
        });
        self.broadcast_snapshot().await;
        inc
    }

    pub async fn force_clip(&self, channel_id: &str) {
        let mut workers = self.workers.lock().await;
        if let Some(w) = workers.get_mut(channel_id) {
            let _ = send_worker_cmd(
                &mut w.stdin,
                &SupervisorToWorker::ForceClip {
                    reason: "Operator Force Clip".into(),
                },
            )
            .await;
        }
    }

    pub async fn toggle_pfl(&self, channel_id: &str) -> Option<String> {
        let mut current = self.active_pfl_channel.write().await;
        let next = if current.as_deref() == Some(channel_id) {
            None
        } else {
            Some(channel_id.to_string())
        };

        {
            let mut workers = self.workers.lock().await;
            tracing::debug!(?channel_id, ?next, "toggle_pfl dispatching SetPfl");
            for (wid, w) in workers.iter_mut() {
                let want = next.as_deref() == Some(wid.as_str());
                if let Err(err) = send_worker_cmd(&mut w.stdin, &SupervisorToWorker::SetPfl { enabled: want }).await {
                    tracing::warn!(%wid, want, "Failed sending SetPfl to worker: {err}");
                }
            }
        }
        *current = next.clone();
        drop(current);

        self.broadcast_snapshot().await;
        next
    }

    /// Reset the integrated average LUFS window for a single channel or all channels.
    pub async fn reset_lufs(&self, channel_id: Option<&str>) -> bool {
        let target_ids: Vec<String> = {
            let mut map = self.channels.write().await;
            if let Some(cid) = channel_id {
                let Some(m) = map.get_mut(cid) else {
                    return false;
                };
                m.audio.integrated_lufs = -70.0;
                m.audio.short_term_lufs = -70.0;
                m.audio.true_peak_dbtp = -96.0;
                m.audio.left_rms_dbfs = -96.0;
                m.audio.right_rms_dbfs = -96.0;
                m.audio.phase_correlation = 1.0;
                m.audio.hum_peak_db = 0.0;
                m.audio.hum_freq_hz = None;
                vec![cid.to_string()]
            } else {
                map.iter_mut()
                    .map(|(cid, m)| {
                        m.audio.integrated_lufs = -70.0;
                        m.audio.short_term_lufs = -70.0;
                        m.audio.true_peak_dbtp = -96.0;
                        m.audio.left_rms_dbfs = -96.0;
                        m.audio.right_rms_dbfs = -96.0;
                        m.audio.phase_correlation = 1.0;
                        m.audio.hum_peak_db = 0.0;
                        m.audio.hum_freq_hz = None;
                        cid.clone()
                    })
                    .collect()
            }
        };

        {
            let mut workers = self.workers.lock().await;
            for cid in &target_ids {
                if let Some(w) = workers.get_mut(cid)
                    && let Err(err) = send_worker_cmd(&mut w.stdin, &SupervisorToWorker::ResetLufs).await
                {
                    tracing::warn!(%cid, "Failed sending ResetLufs to worker: {err}");
                }
            }
        }

        let map = self.channels.read().await;
        for cid in target_ids {
            if let Some(m) = map.get(&cid) {
                let _ = self.ws_tx.send(WsBroadcast::Meter {
                    channel_id: cid,
                    audio: m.audio.clone(),
                });
            }
        }
        true
    }

    pub async fn set_intermission(&self, group_id: Option<&str>, active: bool) {
        let now = Utc::now();
        match group_id {
            None => {
                *self.global_intermission.write().await = active;
                let mut windows = self.global_hold_windows.write().await;
                record_hold_transition(&mut windows, active, now);
                self.db.enqueue(DbCommand::SetGlobalIntermission(active)).await;
            }
            Some(gid) => {
                {
                    let mut show = self.show.write().await;
                    if let Some(grp) = show.groups.iter_mut().find(|g| g.id == gid) {
                        grp.intermission_active = active;
                    }
                }
                let mut group_map = self.group_hold_windows.write().await;
                let windows = group_map.entry(gid.to_string()).or_default();
                record_hold_transition(windows, active, now);
                self.db
                    .enqueue(DbCommand::SetGroupIntermission {
                        group_id: gid.to_string(),
                        active,
                    })
                    .await;
            }
        }
    }

    /// Evaluate Hold/Standby at `now - delay` with a 30s Hold-OFF flush grace tail.
    pub(crate) async fn effective_intermission_for(
        &self,
        group_id: &str,
        delay_secs: u32,
        now: chrono::DateTime<Utc>,
    ) -> bool {
        if delay_secs == 0 {
            if *self.global_intermission.read().await {
                return true;
            }
            let group_active = self
                .show
                .read()
                .await
                .groups
                .iter()
                .find(|g| g.id == group_id)
                .is_some_and(|g| g.intermission_active);
            if group_active {
                return true;
            }
        }
        let eval_at = now - chrono::Duration::seconds(i64::from(delay_secs));
        let in_global =
            window_contains_time(&self.global_hold_windows.read().await, eval_at, HOLD_END_GRACE_SECS);
        if in_global {
            return true;
        }
        self.group_hold_windows
            .read()
            .await
            .get(group_id)
            .is_some_and(|w| window_contains_time(w, eval_at, HOLD_END_GRACE_SECS))
    }

    pub async fn start_match(&self, label: String) -> MatchMarker {
        self.end_active_match().await;
        let show_id = self.show.read().await.show_id.clone();
        let marker = MatchMarker {
            id: Uuid::new_v4().to_string(),
            show_id,
            label,
            started_at: Utc::now(),
            ended_at: None,
        };
        *self.active_match.write().await = Some(marker.clone());
        self.matches.write().await.push(marker.clone());
        self.db.enqueue(DbCommand::UpsertMatch(marker.clone())).await;
        marker
    }

    pub async fn end_active_match(&self) -> Option<MatchMarker> {
        let mut active = self.active_match.write().await;
        if let Some(mut m) = active.take() {
            m.ended_at = Some(Utc::now());
            let mut list = self.matches.write().await;
            if let Some(existing) = list.iter_mut().find(|x| x.id == m.id) {
                existing.ended_at = m.ended_at;
            }
            self.db.enqueue(DbCommand::UpsertMatch(m.clone())).await;
            return Some(m);
        }
        None
    }
}

fn record_hold_transition(windows: &mut Vec<HoldWindow>, active: bool, now: chrono::DateTime<Utc>) {
    if active {
        if windows.last().is_none_or(|(_, end)| end.is_some()) {
            windows.push((now, None));
        }
    } else if let Some((_, end)) = windows.last_mut().filter(|(_, e)| e.is_none()) {
        *end = Some(now);
    }
    let cutoff = now - chrono::Duration::hours(3) - chrono::Duration::seconds(HOLD_END_GRACE_SECS);
    windows.retain(|(_, end)| end.is_none_or(|e| e >= cutoff));
}

fn window_contains_time(
    windows: &[HoldWindow],
    t: chrono::DateTime<Utc>,
    grace_secs: i64,
) -> bool {
    let grace = chrono::Duration::seconds(grace_secs);
    windows.iter().any(|&(start, end)| match end {
        Some(e) => t >= start && t <= e + grace,
        None => t >= start,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_hold_window_delay_and_grace_tail() {
        let t_on = Utc.with_ymd_and_hms(2026, 11, 25, 18, 30, 0).unwrap();
        let t_off = Utc.with_ymd_and_hms(2026, 11, 25, 18, 45, 0).unwrap();
        let mut wins = Vec::new();
        record_hold_transition(&mut wins, true, t_on);
        record_hold_transition(&mut wins, false, t_off);

        assert!(window_contains_time(
            &wins,
            Utc.with_ymd_and_hms(2026, 11, 25, 18, 45, 20).unwrap(),
            HOLD_END_GRACE_SECS
        ));
        assert!(!window_contains_time(
            &wins,
            Utc.with_ymd_and_hms(2026, 11, 25, 18, 45, 35).unwrap(),
            HOLD_END_GRACE_SECS
        ));

        let delayed_now = Utc.with_ymd_and_hms(2026, 11, 25, 18, 55, 20).unwrap();
        assert!(window_contains_time(
            &wins,
            delayed_now - chrono::Duration::seconds(600),
            HOLD_END_GRACE_SECS
        ));
    }
}
