use crate::win32::{
    is_on_battery, query_dxgi_adapters, resolve_dxgi_adapter_index_for_luid, DxgiAdapterInfo,
    WindowsCpuUtilitySampler,
};
use chrono::Utc;
use pgmwatch_core::config::GovernorConfig;
use pgmwatch_core::models::GovernorTelemetry;
use sysinfo::System;

pub struct ResourceGovernor {
    sys: System,
    cpu_utility: Option<WindowsCpuUtilitySampler>,
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
        Self {
            sys,
            cpu_utility: WindowsCpuUtilitySampler::new(),
            cfg,
            last_wan_rx_mbps: 0.0,
            preferred_gpu_luid: String::new(),
        }
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
        app_wan_rx_mbps: f32,
        max_decode_latency_pct: f32,
        nominal_thumb_fps: f32,
    ) -> GovernorDecision {
        // Track ONLY PGMWatch's own measured download traffic (workers + Live 60p MSE hubs).
        // Sampling host-wide OS NIC counters would falsely count unrelated local LAN video
        // (e.g., OBS/vMix NDI feeds on Ethernet) as PGMWatch internet consumption.
        let wan_rx_mbps = if app_wan_rx_mbps.is_finite() && app_wan_rx_mbps >= 0.0 {
            app_wan_rx_mbps
        } else {
            0.0
        };
        self.last_wan_rx_mbps = wan_rx_mbps;
        self.sys.refresh_memory();
        self.sys.refresh_cpu_usage();

        let ram_used_mb = self.sys.used_memory() / (1024 * 1024);
        let ram_total_mb = (self.sys.total_memory() / (1024 * 1024)).max(1);
        let ram_pct = (ram_used_mb as f32 / ram_total_mb as f32) * 100.0;
        let cpu_pct = self
            .cpu_utility
            .as_mut()
            .and_then(|s| s.sample())
            .unwrap_or_else(|| self.sys.global_cpu_usage());
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
