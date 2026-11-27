use crate::win32::{is_on_battery, query_dxgi_adapters, resolve_dxgi_adapter_index_for_luid, DxgiAdapterInfo};
use chrono::Utc;
use pgmwatch_core::config::GovernorConfig;
use pgmwatch_core::models::GovernorTelemetry;
use std::time::Instant;
use sysinfo::{Networks, System};

pub struct ResourceGovernor {
    sys: System,
    networks: Networks,
    last_net_sample: Instant,
    os_wan_rx_mbps: f32,
    cfg: GovernorConfig,
    last_wan_rx_mbps: f32,
    preferred_gpu_luid: String,
}

#[derive(Debug, Clone)]
pub struct GovernorDecision {
    pub telemetry: GovernorTelemetry,
    pub target_thumbnail_fps: f32,
    pub shed_to_light: bool,
}

pub fn check_wan_admission(
    wan_rx_mbps: f32,
    wan_cap_mbps: f32,
    wan_admit_max_pct: f32,
) -> Result<(), String> {
    let wan_pct = (wan_rx_mbps / wan_cap_mbps.max(1.0)) * 100.0;
    if wan_pct > wan_admit_max_pct {
        return Err(format!(
            "WAN {wan_pct:.1}% ({wan_rx_mbps:.1}/{} Mbps) exceeds admission cap {wan_admit_max_pct}%",
            wan_cap_mbps
        ));
    }
    Ok(())
}

impl ResourceGovernor {
    pub fn new(cfg: GovernorConfig) -> Self {
        let mut sys = System::new_all();
        sys.refresh_all();
        let networks = Networks::new_with_refreshed_list();
        Self {
            sys,
            networks,
            last_net_sample: Instant::now(),
            os_wan_rx_mbps: 0.0,
            cfg,
            last_wan_rx_mbps: 0.0,
            preferred_gpu_luid: String::new(),
        }
    }

    fn sample_os_wan_rx_mbps(&mut self) -> f32 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_net_sample).as_secs_f64();
        if elapsed < 0.25 {
            return self.os_wan_rx_mbps;
        }
        self.last_net_sample = now;
        self.networks.refresh(true);
        let mut rx_bytes: u64 = 0;
        for (name, data) in &self.networks {
            let lower = name.to_ascii_lowercase();
            if lower.contains("loopback") || lower == "lo" {
                continue;
            }
            rx_bytes = rx_bytes.saturating_add(data.received());
        }
        let instant_mbps = ((rx_bytes as f64) * 8.0 / (elapsed * 1_000_000.0)) as f32;
        if instant_mbps.is_finite() && instant_mbps >= 0.0 {
            self.os_wan_rx_mbps = if self.os_wan_rx_mbps <= 0.01 {
                instant_mbps
            } else {
                self.os_wan_rx_mbps * 0.5 + instant_mbps * 0.5
            };
        }
        self.os_wan_rx_mbps
    }

    pub fn update_config(&mut self, cfg: GovernorConfig) {
        self.cfg = cfg;
    }

    pub fn set_preferred_gpu_luid(&mut self, luid: String) {
        self.preferred_gpu_luid = luid;
    }

    fn pick_monitored_adapter<'a>(&self, adapters: &'a [DxgiAdapterInfo]) -> Option<&'a DxgiAdapterInfo> {
        if let Some(target_idx) =
            resolve_dxgi_adapter_index_for_luid(adapters, &self.preferred_gpu_luid)
            && let Some(found) = adapters
                .iter()
                .find(|a| a.adapter_index.to_string() == target_idx)
        {
            return Some(found);
        }
        adapters.first()
    }

    pub fn admit_tier_upgrade(&mut self) -> Result<(), String> {
        self.sys.refresh_memory();
        let ram_total = self.sys.total_memory().max(1);
        let ram_pct = (self.sys.used_memory() as f32 / ram_total as f32) * 100.0;
        if ram_pct > self.cfg.ram_admit_max_pct {
            return Err(format!("RAM {ram_pct:.1}% exceeds admission cap {}%", self.cfg.ram_admit_max_pct));
        }
        let adapters = query_dxgi_adapters();
        if let Some(a) = self.pick_monitored_adapter(&adapters) {
            let vram_pct = (a.vram_used_mb as f32 / a.vram_budget_mb.max(1) as f32) * 100.0;
            if vram_pct > self.cfg.vram_admit_max_pct {
                return Err(format!("VRAM {vram_pct:.1}% exceeds admission cap {}%", self.cfg.vram_admit_max_pct));
            }
        }
        check_wan_admission(
            self.last_wan_rx_mbps,
            self.cfg.wan_cap_mbps,
            self.cfg.wan_admit_max_pct,
        )?;
        Ok(())
    }

    pub fn evaluate(
        &mut self,
        active_workers: usize,
        shed_channels: usize,
        worker_wan_rx_mbps: f32,
        max_decode_latency_pct: f32,
        nominal_thumb_fps: f32,
    ) -> GovernorDecision {
        // Measure real host NIC RX (captures MSE Live 60p, muxed LIGHT downloads, and HTTP overhead)
        // and take max with reported worker media bitrates so the UI & WAN cap reflect actual load.
        let os_rx_mbps = self.sample_os_wan_rx_mbps();
        let wan_rx_mbps = worker_wan_rx_mbps.max(os_rx_mbps);
        self.last_wan_rx_mbps = wan_rx_mbps;
        self.sys.refresh_memory();
        self.sys.refresh_cpu_usage();

        let ram_used_mb = self.sys.used_memory() / (1024 * 1024);
        let ram_total_mb = (self.sys.total_memory() / (1024 * 1024)).max(1);
        let ram_pct = (ram_used_mb as f32 / ram_total_mb as f32) * 100.0;
        let cpu_pct = self.sys.global_cpu_usage();
        let wan_pct = (wan_rx_mbps / self.cfg.wan_cap_mbps.max(1.0)) * 100.0;

        let adapters = query_dxgi_adapters();
        let (gpu_adapter_name, vram_used_mb, vram_budget_mb, vram_pct) =
            if let Some(primary) = self.pick_monitored_adapter(&adapters) {
                let pct = (primary.vram_used_mb as f32 / primary.vram_budget_mb.max(1) as f32) * 100.0;
                (
                    primary.name.clone(),
                    primary.vram_used_mb,
                    primary.vram_budget_mb,
                    pct,
                )
            } else {
                ("CPU / No DXGI Adapter".to_string(), 0, 0, 0.0)
            };

        let on_battery = is_on_battery();

        let stage2 = vram_pct >= self.cfg.vram_stage2_pct
            || max_decode_latency_pct >= self.cfg.decode_latency_stage2_pct
            || wan_pct >= 100.0;
        let stage1 = stage2
            || vram_pct >= self.cfg.vram_stage1_pct
            || max_decode_latency_pct >= self.cfg.decode_latency_stage1_pct
            || wan_pct >= self.cfg.wan_admit_max_pct
            || on_battery;

        let degradation_stage = if stage2 {
            2
        } else if stage1 {
            1
        } else {
            0
        };

        let target_thumbnail_fps = if stage1 { 0.5 } else { nominal_thumb_fps };

        GovernorDecision {
            telemetry: GovernorTelemetry {
                gpu_adapter_name,
                vram_used_mb,
                vram_budget_mb,
                vram_pct,
                ram_used_mb,
                ram_total_mb,
                ram_pct,
                cpu_pct,
                wan_rx_mbps,
                wan_cap_mbps: self.cfg.wan_cap_mbps,
                max_decode_latency_pct,
                on_battery,
                degradation_stage,
                active_workers,
                shed_channels,
                updated_at: Utc::now(),
            },
            target_thumbnail_fps,
            shed_to_light: stage2,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::check_wan_admission;

    #[test]
    fn test_wan_admission_threshold() {
        assert!(check_wan_admission(180.0, 250.0, 85.0).is_ok());
        assert!(check_wan_admission(220.0, 250.0, 85.0).is_err());
    }
}
