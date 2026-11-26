use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DxgiAdapterInfo {
    pub adapter_index: u32,
    pub name: String,
    pub luid_hex: String,
    pub vram_used_mb: u64,
    pub vram_budget_mb: u64,
}

#[cfg(windows)]
mod imp {
    use super::DxgiAdapterInfo;
    use std::mem::size_of;
    use std::sync::OnceLock;
    use windows::core::Interface;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory4, DXGI_ADAPTER_FLAG_SOFTWARE,
        DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    };
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
    use windows::Win32::System::Power::{
        GetSystemPowerStatus, SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
        SYSTEM_POWER_STATUS,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, ProcessPowerThrottling, SetProcessInformation,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    pub struct WindowsJobGuard {
        handle: HANDLE,
    }

    unsafe impl Send for WindowsJobGuard {}
    unsafe impl Sync for WindowsJobGuard {}

    impl WindowsJobGuard {
        pub fn new(process_memory_limit_mb: u64) -> anyhow::Result<Self> {
            unsafe {
                let job = CreateJobObjectW(None, None)?;
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags =
                    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = (process_memory_limit_mb as usize) * 1024 * 1024;
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )?;
                Ok(Self { handle: job })
            }
        }

        pub fn assign_pid(&self, pid: u32) -> anyhow::Result<()> {
            unsafe {
                let proc = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
                let res = AssignProcessToJobObject(self.handle, proc);
                let _ = CloseHandle(proc);
                res?;
                Ok(())
            }
        }
    }

    impl Drop for WindowsJobGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.handle);
            }
        }
    }

    pub fn prevent_sleep_and_eco_qos() {
        unsafe {
            let _ = SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED);
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: 1,
                ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: 0,
            };
            let _ = SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &state as *const _ as *const _,
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }

    pub fn is_on_battery() -> bool {
        unsafe {
            let mut status = SYSTEM_POWER_STATUS::default();
            if GetSystemPowerStatus(&mut status).is_ok() {
                status.ACLineStatus == 0
            } else {
                false
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    type D3dkmtQueryStatsFn = unsafe extern "system" fn(*mut u8) -> i32;

    #[cfg(target_arch = "x86_64")]
    fn resolve_d3dkmt_query_statistics() -> Option<D3dkmtQueryStatsFn> {
        static FN_PTR: OnceLock<Option<usize>> = OnceLock::new();
        let addr = (*FN_PTR.get_or_init(|| unsafe {
            let gdi_name = windows::core::w!("gdi32.dll");
            let mut hmod = GetModuleHandleW(gdi_name).ok();
            if hmod.is_none() {
                hmod = LoadLibraryW(gdi_name).ok();
            }
            let hmod = hmod?;
            let proc_name = windows::core::s!("D3DKMTQueryStatistics");
            GetProcAddress(hmod, proc_name).map(|f| f as usize)
        }))?;
        Some(unsafe { std::mem::transmute::<usize, D3dkmtQueryStatsFn>(addr) })
    }

    #[cfg(target_arch = "x86_64")]
    fn read_u32_le(buf: &[u8], off: usize) -> u32 {
        u32::from_le_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
    }

    #[cfg(target_arch = "x86_64")]
    fn read_u64_le(buf: &[u8], off: usize) -> u64 {
        u64::from_le_bytes([
            buf[off],
            buf[off + 1],
            buf[off + 2],
            buf[off + 3],
            buf[off + 4],
            buf[off + 5],
            buf[off + 6],
            buf[off + 7],
        ])
    }

    /// Queries system-wide (all processes) WDDM adapter memory usage in MB for `luid`
    /// using `gdi32!D3DKMTQueryStatistics`, since `IDXGIAdapter3::QueryVideoMemoryInfo`
    /// only measures the calling supervisor process (0 MB when workers decode in child processes).
    #[cfg(target_arch = "x86_64")]
    pub(crate) fn query_wddm_adapter_vram_used_mb(luid: LUID, has_dedicated_vram: bool) -> Option<u64> {
        // Exact x64 ABI from Windows SDK d3dkmthk.h:
        // sizeof(D3DKMT_QUERYSTATISTICS) == 0x328, Type at 0, AdapterLuid at 4..12,
        // QueryResult at 24, tail QuerySegment.SegmentId at 0x320.
        const STRUCT_SIZE: usize = 0x328;
        const QUERY_TYPE_ADAPTER: u32 = 0;
        const QUERY_TYPE_SEGMENT: u32 = 3;
        const TAIL_SEGMENT_ID_OFF: usize = 0x320;

        #[repr(C, align(8))]
        struct QueryStatsBuf([u8; STRUCT_SIZE]);

        let query_fn = resolve_d3dkmt_query_statistics()?;
        let mut buf = QueryStatsBuf([0u8; STRUCT_SIZE]);
        buf.0[0..4].copy_from_slice(&QUERY_TYPE_ADAPTER.to_le_bytes());
        buf.0[4..8].copy_from_slice(&luid.LowPart.to_le_bytes());
        buf.0[8..12].copy_from_slice(&luid.HighPart.to_le_bytes());

        let rc = unsafe { query_fn(buf.0.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        let nb_segments = read_u32_le(&buf.0, 24).min(64);
        if nb_segments == 0 {
            return None;
        }

        let mut dedicated_bytes = 0u64;
        let mut shared_bytes = 0u64;
        for seg_id in 0..nb_segments {
            let mut sbuf = QueryStatsBuf([0u8; STRUCT_SIZE]);
            sbuf.0[0..4].copy_from_slice(&QUERY_TYPE_SEGMENT.to_le_bytes());
            sbuf.0[4..8].copy_from_slice(&luid.LowPart.to_le_bytes());
            sbuf.0[8..12].copy_from_slice(&luid.HighPart.to_le_bytes());
            sbuf.0[TAIL_SEGMENT_ID_OFF..TAIL_SEGMENT_ID_OFF + 4]
                .copy_from_slice(&seg_id.to_le_bytes());

            let rc_seg = unsafe { query_fn(sbuf.0.as_mut_ptr()) };
            if rc_seg != 0 {
                continue;
            }
            let bytes_committed = read_u64_le(&sbuf.0, 32);
            let aperture = read_u32_le(&sbuf.0, 64);
            if aperture == 0 {
                dedicated_bytes = dedicated_bytes.saturating_add(bytes_committed);
            } else {
                shared_bytes = shared_bytes.saturating_add(bytes_committed);
            }
        }

        let chosen = if has_dedicated_vram || dedicated_bytes > 0 {
            dedicated_bytes
        } else {
            shared_bytes
        };
        Some(chosen / (1024 * 1024))
    }

    #[cfg(not(target_arch = "x86_64"))]
    pub(crate) fn query_wddm_adapter_vram_used_mb(_luid: LUID, _has_dedicated_vram: bool) -> Option<u64> {
        None
    }

    pub fn query_dxgi_adapters() -> Vec<DxgiAdapterInfo> {
        let mut out = Vec::new();
        unsafe {
            let factory: IDXGIFactory4 = match CreateDXGIFactory1() {
                Ok(f) => f,
                Err(_) => return out,
            };
            let mut idx = 0u32;
            while let Ok(adapter) = factory.EnumAdapters1(idx) {
                let adapter_index = idx;
                idx += 1;
                let Ok(desc) = adapter.GetDesc1() else {
                    continue;
                };
                if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 {
                    continue;
                }
                let name_end = desc
                    .Description
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(desc.Description.len());
                let name = String::from_utf16_lossy(&desc.Description[..name_end]);
                let luid = ((desc.AdapterLuid.HighPart as i64 as u64) << 32)
                    | (desc.AdapterLuid.LowPart as u64);

                let mut vram_used_mb = 0u64;
                let mut vram_budget_mb = (desc.DedicatedVideoMemory as u64) / (1024 * 1024);
                if let Ok(adapter3) = adapter.cast::<IDXGIAdapter3>() {
                    let mut mem_info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                    if adapter3
                        .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut mem_info)
                        .is_ok()
                    {
                        vram_used_mb = mem_info.CurrentUsage / (1024 * 1024);
                        if mem_info.Budget > 0 {
                            vram_budget_mb = mem_info.Budget / (1024 * 1024);
                        }
                    }
                }
                if let Some(system_used_mb) =
                    query_wddm_adapter_vram_used_mb(desc.AdapterLuid, desc.DedicatedVideoMemory > 0)
                {
                    vram_used_mb = vram_used_mb.max(system_used_mb);
                }
                out.push(DxgiAdapterInfo {
                    adapter_index,
                    name,
                    luid_hex: format!("0x{luid:016x}"),
                    vram_used_mb,
                    vram_budget_mb: vram_budget_mb.max(256),
                });
            }
        }
        out
    }
}

#[cfg(not(windows))]
mod imp {
    use super::DxgiAdapterInfo;
    pub struct WindowsJobGuard;
    impl WindowsJobGuard {
        pub fn new(_mb: u64) -> anyhow::Result<Self> {
            Ok(Self)
        }
        pub fn assign_pid(&self, _pid: u32) -> anyhow::Result<()> {
            Ok(())
        }
    }
    pub fn prevent_sleep_and_eco_qos() {}
    pub fn is_on_battery() -> bool {
        false
    }
    pub fn query_dxgi_adapters() -> Vec<DxgiAdapterInfo> {
        vec![]
    }
}

pub use imp::{
    is_on_battery, prevent_sleep_and_eco_qos, query_dxgi_adapters, WindowsJobGuard,
};

pub fn resolve_dxgi_adapter_index_for_luid(
    adapters: &[DxgiAdapterInfo],
    configured_luid: &str,
) -> Option<String> {
    let trimmed = configured_luid.trim();
    if trimmed.is_empty() {
        return None;
    }
    let norm_cfg = trimmed
        .trim_start_matches("0x")
        .trim_start_matches("0X")
        .to_ascii_lowercase();
    let cfg_val = u64::from_str_radix(&norm_cfg, 16).ok();

    for a in adapters {
        let norm_a = a
            .luid_hex
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .to_ascii_lowercase();
        let a_val = u64::from_str_radix(&norm_a, 16).ok();
        let exact = norm_a == norm_cfg;
        let numeric_eq = matches!(
            (cfg_val, a_val),
            (Some(c), Some(v)) if c == v || (c <= 0xffff_ffff && (v & 0xffff_ffff) == c)
        );
        if exact || numeric_eq {
            return Some(a.adapter_index.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_dxgi_adapter_index_for_luid() {
        let adapters = vec![
            DxgiAdapterInfo {
                adapter_index: 0,
                name: "Primary iGPU".into(),
                luid_hex: "0x0000000000010ab4".into(),
                vram_used_mb: 100,
                vram_budget_mb: 2048,
            },
            DxgiAdapterInfo {
                adapter_index: 2,
                name: "Secondary dGPU".into(),
                luid_hex: "0x0000000100021f00".into(),
                vram_used_mb: 200,
                vram_budget_mb: 8192,
            },
        ];
        assert_eq!(resolve_dxgi_adapter_index_for_luid(&adapters, ""), None);
        assert_eq!(
            resolve_dxgi_adapter_index_for_luid(&adapters, "0x00010ab4").as_deref(),
            Some("0")
        );
        assert_eq!(
            resolve_dxgi_adapter_index_for_luid(&adapters, "0x0000000100021F00").as_deref(),
            Some("2")
        );
        assert_eq!(resolve_dxgi_adapter_index_for_luid(&adapters, "0xdeadbeef"), None);
    }

    #[cfg(windows)]
    #[test]
    fn test_dxgi_adapters_query_does_not_fail() {
        let adapters = query_dxgi_adapters();
        for a in &adapters {
            assert!(a.vram_budget_mb >= 256);
        }
        if let Some(first) = adapters.first() {
            // Confirm system-wide GPU memory on active hardware desktop GPUs is readable (> 0 MB).
            assert!(
                first.vram_used_mb > 0,
                "expected non-zero system-wide VRAM usage on hardware adapter {}",
                first.name
            );
        }
    }
}
