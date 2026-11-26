use crate::config::AlertsConfig;
use crate::models::{AlertState, FaultKind};
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ChannelAlertTracker {
    pub state: AlertState,
    pub active_faults: Vec<FaultKind>,
    fault_started_at: Option<DateTime<Utc>>,
    healthy_since: Option<DateTime<Utc>>,
    acknowledged_until_clear: bool,
    last_clip_at: HashMap<FaultKind, DateTime<Utc>>,
    external_override: Option<(AlertState, String)>,
    pub status_reason: Option<String>,
    healthy_ticks: u64,
    total_ticks: u64,
}

#[derive(Debug, Clone)]
pub struct StateTransition {
    pub prev_state: AlertState,
    pub new_state: AlertState,
    pub clip_trigger_fault: Option<FaultKind>,
    pub opened_incident_fault: Option<FaultKind>,
    pub resolved_incident: bool,
}

impl Default for ChannelAlertTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelAlertTracker {
    pub fn new() -> Self {
        Self {
            state: AlertState::Offline,
            active_faults: Vec::new(),
            fault_started_at: None,
            healthy_since: None,
            acknowledged_until_clear: false,
            last_clip_at: HashMap::new(),
            external_override: None,
            status_reason: None,
            healthy_ticks: 0,
            total_ticks: 0,
        }
    }

    pub fn new_connecting() -> Self {
        Self {
            state: AlertState::Reacquiring,
            active_faults: Vec::new(),
            fault_started_at: None,
            healthy_since: None,
            acknowledged_until_clear: false,
            last_clip_at: HashMap::new(),
            external_override: None,
            status_reason: Some("Queued for stream resolution".to_string()),
            healthy_ticks: 0,
            total_ticks: 0,
        }
    }

    pub fn set_external_state(&mut self, state: AlertState, reason: impl Into<String>) {
        let reason_str = reason.into();
        self.external_override = Some((state, reason_str.clone()));
        self.state = state;
        self.status_reason = Some(reason_str);
        self.fault_started_at = None;
        self.healthy_since = None;
        self.acknowledged_until_clear = false;
    }

    pub fn clear_external_state(&mut self) {
        self.external_override = None;
    }

    pub fn acknowledge(&mut self) {
        if matches!(self.state, AlertState::Warning | AlertState::Critical) {
            self.acknowledged_until_clear = true;
            self.state = AlertState::Acknowledged;
        }
    }

    pub fn uptime_pct(&self) -> f32 {
        if self.total_ticks == 0 {
            100.0
        } else {
            ((self.healthy_ticks as f64 / self.total_ticks as f64) * 100.0) as f32
        }
    }

    pub fn evaluate(
        &mut self,
        now: DateTime<Utc>,
        raw_faults: &[FaultKind],
        ad_break: bool,
        intermission: bool,
        cfg: &AlertsConfig,
    ) -> StateTransition {
        let prev_state = self.state;
        self.total_ticks += 1;

        if ad_break {
            self.state = AlertState::AdBreak;
            self.active_faults.clear();
            self.fault_started_at = None;
            self.healthy_since = None;
            self.acknowledged_until_clear = false;
            self.status_reason = Some("Platform ad break active".to_string());
            self.healthy_ticks += 1;
            return StateTransition {
                prev_state,
                new_state: self.state,
                clip_trigger_fault: None,
                opened_incident_fault: None,
                resolved_incident: matches!(
                    prev_state,
                    AlertState::Warning | AlertState::Critical | AlertState::Acknowledged
                ),
            };
        }

        if let Some((ext_state, ref reason)) = self.external_override {
            self.state = ext_state;
            self.status_reason = Some(reason.clone());
            return StateTransition {
                prev_state,
                new_state: self.state,
                clip_trigger_fault: None,
                opened_incident_fault: None,
                resolved_incident: false,
            };
        }

        let filtered: Vec<FaultKind> = raw_faults
            .iter()
            .copied()
            .filter(|f| !(intermission && f.suppressed_in_intermission()))
            .collect();

        if filtered.is_empty() {
            self.active_faults.clear();
            self.fault_started_at = None;

            if matches!(
                prev_state,
                AlertState::Warning | AlertState::Critical | AlertState::Acknowledged
            ) {
                let since = self.healthy_since.get_or_insert(now);
                let elapsed = (now - *since).num_seconds().max(0) as u64;
                if elapsed >= cfg.recovery_hysteresis_secs {
                    self.state = AlertState::Healthy;
                    self.healthy_since = None;
                    self.acknowledged_until_clear = false;
                    self.status_reason = None;
                    self.healthy_ticks += 1;
                    return StateTransition {
                        prev_state,
                        new_state: AlertState::Healthy,
                        clip_trigger_fault: None,
                        opened_incident_fault: None,
                        resolved_incident: true,
                    };
                }
                return StateTransition {
                    prev_state,
                    new_state: self.state,
                    clip_trigger_fault: None,
                    opened_incident_fault: None,
                    resolved_incident: false,
                };
            }

            self.state = AlertState::Healthy;
            self.healthy_since = None;
            self.acknowledged_until_clear = false;
            self.status_reason = None;
            self.healthy_ticks += 1;
            return StateTransition {
                prev_state,
                new_state: AlertState::Healthy,
                clip_trigger_fault: None,
                opened_incident_fault: None,
                resolved_incident: false,
            };
        }

        // Active fault(s) present
        self.healthy_since = None;
        self.active_faults = filtered.clone();
        let primary_fault = filtered[0];
        let started = self.fault_started_at.get_or_insert(now);
        let sustained_secs = (now - *started).num_seconds().max(0) as u64;

        let new_state = if self.acknowledged_until_clear {
            AlertState::Acknowledged
        } else if sustained_secs >= cfg.critical_escalation_secs {
            AlertState::Critical
        } else {
            AlertState::Warning
        };

        self.state = new_state;
        self.status_reason = Some(primary_fault.label().to_string());

        let opened_incident_fault = if !matches!(
            prev_state,
            AlertState::Warning | AlertState::Critical | AlertState::Acknowledged
        ) {
            Some(primary_fault)
        } else {
            None
        };

        let clip_trigger_fault = if self.should_trigger_clip(primary_fault, now, cfg.clip_cooldown_secs)
        {
            self.last_clip_at.insert(primary_fault, now);
            Some(primary_fault)
        } else {
            None
        };

        StateTransition {
            prev_state,
            new_state,
            clip_trigger_fault,
            opened_incident_fault,
            resolved_incident: false,
        }
    }

    fn should_trigger_clip(
        &self,
        fault: FaultKind,
        now: DateTime<Utc>,
        cooldown_secs: u64,
    ) -> bool {
        match self.last_clip_at.get(&fault) {
            Some(prev) => now - *prev >= Duration::seconds(cooldown_secs as i64),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hysteresis_escalation_and_intermission() {
        let cfg = AlertsConfig {
            recovery_hysteresis_secs: 3,
            critical_escalation_secs: 5,
            clip_cooldown_secs: 30,
            chat_spike_unique_viewers: 10,
            chat_spike_window_secs: 15,
            custom_target_lufs: None,
            lufs_tolerance: 2.5,
        };
        let mut tracker = ChannelAlertTracker::new();
        let t0 = Utc::now();

        let tr1 = tracker.evaluate(t0, &[FaultKind::AudioDeadAir], false, false, &cfg);
        assert_eq!(tr1.new_state, AlertState::Warning);
        assert_eq!(tr1.clip_trigger_fault, Some(FaultKind::AudioDeadAir));

        // 2s later still warning, clip suppressed by 30s cooldown
        let tr2 = tracker.evaluate(
            t0 + Duration::seconds(2),
            &[FaultKind::AudioDeadAir],
            false,
            false,
            &cfg,
        );
        assert_eq!(tr2.new_state, AlertState::Warning);
        assert_eq!(tr2.clip_trigger_fault, None);

        // 5s later escalates to critical
        let tr3 = tracker.evaluate(
            t0 + Duration::seconds(5),
            &[FaultKind::AudioDeadAir],
            false,
            false,
            &cfg,
        );
        assert_eq!(tr3.new_state, AlertState::Critical);

        // Intermission suppresses AudioDeadAir and starts 3s hysteresis recovery
        let tr4 = tracker.evaluate(
            t0 + Duration::seconds(6),
            &[FaultKind::AudioDeadAir],
            false,
            true,
            &cfg,
        );
        assert_eq!(tr4.new_state, AlertState::Critical);

        let tr5 = tracker.evaluate(
            t0 + Duration::seconds(9),
            &[FaultKind::AudioDeadAir],
            false,
            true,
            &cfg,
        );
        assert_eq!(tr5.new_state, AlertState::Healthy);
        assert!(tr5.resolved_incident);

        // Hold Mode also suppresses VBR BitrateCollapse on static standby slides
        let tr6 = tracker.evaluate(
            t0 + Duration::seconds(10),
            &[FaultKind::BitrateCollapse],
            false,
            true,
            &cfg,
        );
        assert_eq!(tr6.new_state, AlertState::Healthy);
    }
}
