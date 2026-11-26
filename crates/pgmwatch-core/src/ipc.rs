use crate::models::{AudioTelemetry, FaultKind, FeedTier, Platform, VideoTelemetry};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SupervisorToWorker {
    UpdateStream {
        resolved_url: String,
        #[serde(default)]
        audio_url: Option<String>,
        headers: Vec<(String, String)>,
        tier: FeedTier,
    },
    SetThumbnailFps {
        fps: f32,
    },
    SetPfl {
        enabled: bool,
    },
    ForceClip {
        reason: String,
    },
    TriggerFaultClip {
        fault: FaultKind,
        detail: String,
    },
    SetGroupReferenceLufs {
        lufs: Option<f32>,
    },
    SetLufsTarget {
        target_lufs: f32,
        tolerance_lu: f32,
    },
    ResetLufs,
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkerToSupervisor {
    Ready {
        channel_id: String,
        pid: u32,
        backend: String,
        hw_accelerated: bool,
        platform: Platform,
    },
    Telemetry {
        channel_id: String,
        timestamp: DateTime<Utc>,
        audio: AudioTelemetry,
        video: VideoTelemetry,
        active_faults: Vec<FaultKind>,
        ad_break: bool,
        rx_kbps: f32,
    },
    Meter {
        channel_id: String,
        audio: AudioTelemetry,
    },
    Thumbnail {
        channel_id: String,
        timestamp: DateTime<Utc>,
        jpeg_b64: String,
    },
    PflAudio {
        channel_id: String,
        timestamp_ms: u64,
        sample_rate: u32,
        channels: u8,
        /// Base64-encoded signed 16-bit PCM interleaved stereo frames for zero-lag WebAudio playback
        pcm_s16le_b64: String,
    },
    ClipSaved {
        channel_id: String,
        fault: Option<FaultKind>,
        filename: String,
        duration_secs: f32,
        saved_at: DateTime<Utc>,
    },
    StreamError {
        channel_id: String,
        code: String,
        message: String,
        token_expired: bool,
        gpu_fault: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_lufs_ipc_roundtrip() {
        let json = serde_json::to_string(&SupervisorToWorker::ResetLufs).unwrap();
        assert_eq!(json, r#"{"type":"reset_lufs"}"#);
        let parsed: SupervisorToWorker = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, SupervisorToWorker::ResetLufs));

        let target_cmd = SupervisorToWorker::SetLufsTarget {
            target_lufs: -18.0,
            tolerance_lu: 1.5,
        };
        let tjson = serde_json::to_string(&target_cmd).unwrap();
        let tparsed: SupervisorToWorker = serde_json::from_str(&tjson).unwrap();
        assert!(matches!(
            tparsed,
            SupervisorToWorker::SetLufsTarget {
                target_lufs,
                tolerance_lu
            } if (target_lufs - (-18.0)).abs() < 0.01 && (tolerance_lu - 1.5).abs() < 0.01
        ));
    }
}
