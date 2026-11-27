mod backend;
mod dsp;
#[cfg(feature = "ffmpeg-ffi")]
mod ffi_backend;
mod manifest_probe;
mod ring_buffer;
#[cfg(feature = "ffmpeg-ffi")]
mod telemetry;
mod video_simd;

use backend::{probe_ffmpeg9_ffi_version, synthesize_tick, LiveMediaReader, MediaOpenOptions};
use manifest_probe::{ManifestProbe, ProbeOutcome};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chrono::Utc;
use clap::Parser;
use dsp::AudioDspAnalyzer;
use pgmwatch_core::ipc::{SupervisorToWorker, WorkerToSupervisor};
use pgmwatch_core::models::{AudioTelemetry, FaultKind, FeedTier, Platform, VideoTelemetry};
use ring_buffer::{ClipSaver, GopRingBuffer};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use video_simd::VideoSimdAnalyzer;

#[derive(Parser, Debug)]
#[command(name = "pgmwatch-worker")]
struct Args {
    #[arg(long)]
    channel_id: String,
    #[arg(long)]
    url: String,
    #[arg(long)]
    audio_url: Option<String>,
    #[arg(long)]
    headers_json: Option<String>,
    #[arg(long)]
    platform: String,
    #[arg(long)]
    tier: String,
    #[arg(long, default_value_t = false)]
    vertical: bool,
    #[arg(long, default_value = "data/clips")]
    clips_dir: PathBuf,
    #[arg(long, default_value_t = 10.0)]
    clip_quota_gb: f64,
    #[arg(long, default_value = "")]
    bind_ip: String,
    #[arg(long, default_value = "auto")]
    gpu_mode: String,
    #[arg(long)]
    hwaccel_device: Option<String>,
    #[arg(long, default_value_t = 6)]
    extra_hw_frames: u32,
    #[arg(long, allow_hyphen_values = true)]
    target_lufs: Option<f32>,
    #[arg(long)]
    lufs_tolerance: Option<f32>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string()),
        )
        .init();

    let args = Args::parse();
    let platform: Platform = serde_json::from_str(&format!("\"{}\"", args.platform))
        .unwrap_or_else(|_| Platform::from_url(&args.url));
    let mut tier: FeedTier = match args.tier.to_uppercase().as_str() {
        "MASTER" => FeedTier::Master,
        "LIGHT" => FeedTier::Light,
        _ => FeedTier::Simulcast,
    };

    let force_cpu_light = args.gpu_mode.eq_ignore_ascii_case("cpu_light");
    let media_opts = MediaOpenOptions {
        gpu_mode: args.gpu_mode.clone(),
        hwaccel_device: args.hwaccel_device.clone(),
        extra_hw_frames: args.extra_hw_frames,
        bind_ip: (!args.bind_ip.trim().is_empty()).then(|| args.bind_ip.trim().to_string()),
    };

    let backend_label = probe_ffmpeg9_ffi_version();
    emit(&WorkerToSupervisor::Ready {
        channel_id: args.channel_id.clone(),
        pid: std::process::id(),
        backend: backend_label,
        hw_accelerated: tier != FeedTier::Light && !force_cpu_light,
        platform,
    })
    .await?;

    let mut current_url = args.url.clone();
    let mut current_audio_url = args.audio_url.clone();
    let mut headers: Vec<(String, String)> = args
        .headers_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<Vec<(String, String)>>(s).ok())
        .unwrap_or_default();
    let mut pfl_enabled = false;
    let mut thumb_fps = 1.5f32;
    let mut group_ref_lufs: Option<f32> = None;
    let mut live_reader = if !current_url.starts_with("sim://") {
        LiveMediaReader::start(
            &current_url,
            current_audio_url.as_deref(),
            &headers,
            args.vertical,
            tier == FeedTier::Light || force_cpu_light,
            &media_opts,
        )
        .ok()
    } else {
        None
    };

    let mut dsp = AudioDspAnalyzer::new(platform)?;
    if args.target_lufs.is_some() || args.lufs_tolerance.is_some() {
        dsp.set_lufs_target(
            args.target_lufs.unwrap_or_else(|| platform.target_lufs()),
            args.lufs_tolerance.unwrap_or_else(|| platform.lufs_tolerance()),
        );
    }
    let mut simd = VideoSimdAnalyzer::new(args.vertical);
    let mut ring = GopRingBuffer::new(6000, 8 * 1024 * 1024);
    let clip_saver = std::sync::Arc::new(ClipSaver::new(args.clips_dir.clone(), args.clip_quota_gb)?);
    let (clip_saved_tx, mut clip_saved_rx) =
        tokio::sync::mpsc::unbounded_channel::<WorkerToSupervisor>();
    let mut probe = ManifestProbe::new(if args.bind_ip.is_empty() {
        None
    } else {
        Some(args.bind_ip.as_str())
    })?;
    let (probe_target_tx, mut probe_target_rx) =
        tokio::sync::watch::channel((current_url.clone(), headers.clone()));
    let (probe_outcome_tx, probe_outcome_rx) =
        tokio::sync::watch::channel::<Option<ProbeOutcome>>(None);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                res = probe_target_rx.changed() => {
                    if res.is_err() {
                        break;
                    }
                }
            }
            let (u, h) = probe_target_rx.borrow().clone();
            let outcome = probe.probe(&u, &h).await;
            let _ = probe_outcome_tx.send(Some(outcome));
        }
    });

    let (stdin_tx, mut stdin_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let stdin = std::io::stdin();
        let locked = stdin.lock();
        for line in std::io::BufReader::new(locked).lines().map_while(Result::ok) {
            if stdin_tx.send(line).is_err() {
                break;
            }
        }
    });
    // 100ms tick (10 Hz) for real-time audio meter & low-latency PFL
    let mut tick_interval = tokio::time::interval(Duration::from_millis(100));
    tick_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut tick = 0u64;
    let mut bitrate_tracker = BitrateCollapseTracker::default();
    let mut av_sync_bad_ms: u64 = 0;

    let mut last_audio_tel = AudioTelemetry::default();
    let mut last_video_tel = VideoTelemetry::default();
    let mut last_audio_faults: Vec<FaultKind> = Vec::new();
    let mut last_video_faults: Vec<FaultKind> = Vec::new();
    let mut last_batch_faults: Vec<FaultKind> = Vec::new();
    let mut last_batch_ad_break = false;
    let mut last_rx_kbps = 0.0f32;
    let mut last_rx_time = Instant::now();
    let mut last_video_time = Instant::now();
    let mut last_thumb_time = Instant::now();
    let mut last_telemetry_emit = Instant::now() - Duration::from_secs(2);
    let mut token_expired_sent_for: Option<String> = None;
    let mut has_media_packet = current_url.starts_with("sim://");

    loop {
        tokio::select! {
            biased;
            cmd_line = stdin_rx.recv() => {
                match cmd_line {
                    Some(raw) => {
                        if let Ok(cmd) = serde_json::from_str::<SupervisorToWorker>(&raw) {
                            match cmd {
                                SupervisorToWorker::UpdateStream { resolved_url, audio_url, tier: new_tier, headers: new_headers } => {
                                    current_url = resolved_url;
                                    current_audio_url = audio_url;
                                    tier = new_tier;
                                    headers = new_headers;
                                    token_expired_sent_for = None;
                                    has_media_packet = current_url.starts_with("sim://");
                                    let _ = probe_target_tx.send((current_url.clone(), headers.clone()));
                                    // Drop & close the previous socket/threads BEFORE opening the new URL
                                    // so tier changes don't double-download two streams simultaneously.
                                    drop(live_reader.take());
                                    live_reader = if !current_url.starts_with("sim://") {
                                        LiveMediaReader::start(
                                            &current_url,
                                            current_audio_url.as_deref(),
                                            &headers,
                                            args.vertical,
                                            tier == FeedTier::Light || force_cpu_light,
                                            &media_opts,
                                        ).ok()
                                    } else {
                                        None
                                    };
                                    bitrate_tracker.reset();
                                    ring.clear();
                                    av_sync_bad_ms = 0;
                                    last_rx_time = Instant::now();
                                }
                                SupervisorToWorker::SetThumbnailFps { fps } => {
                                    thumb_fps = fps.max(0.2);
                                }
                                SupervisorToWorker::SetPfl { enabled } => {
                                    tracing::debug!(channel_id = %args.channel_id, enabled, "Worker applied SetPfl");
                                    pfl_enabled = enabled;
                                }
                                SupervisorToWorker::SetGroupReferenceLufs { lufs } => {
                                    group_ref_lufs = lufs;
                                }
                                SupervisorToWorker::SetLufsTarget { target_lufs, tolerance_lu } => {
                                    dsp.set_lufs_target(target_lufs, tolerance_lu);
                                }
                                SupervisorToWorker::ResetLufs => {
                                    dsp.reset_average_lufs();
                                    last_audio_tel = AudioTelemetry {
                                        integrated_lufs: -70.0,
                                        short_term_lufs: -70.0,
                                        true_peak_dbtp: -96.0,
                                        left_rms_dbfs: -96.0,
                                        right_rms_dbfs: -96.0,
                                        phase_correlation: 1.0,
                                        hum_peak_db: 0.0,
                                        hum_freq_hz: None,
                                    };
                                    last_audio_faults.clear();
                                    let _ = emit(&WorkerToSupervisor::Meter {
                                        channel_id: args.channel_id.clone(),
                                        audio: last_audio_tel.clone(),
                                    })
                                    .await;
                                }
                                SupervisorToWorker::ForceClip { reason: _ } => {
                                    spawn_clip_save(
                                        std::sync::Arc::clone(&clip_saver),
                                        clip_saved_tx.clone(),
                                        args.channel_id.clone(),
                                        None,
                                        current_url.clone(),
                                        ring.snapshot_from_pretrigger_idr(2000),
                                        ring.video_config().cloned(),
                                        ring.audio_config().cloned(),
                                    );
                                }
                                SupervisorToWorker::TriggerFaultClip { fault, detail: _ } => {
                                    // Dead-air / hum audio faults have already persisted for several seconds;
                                    // capture a tight in-fault window so normal pre-fault program audio doesn't leak in.
                                    let pretrigger_ms = match fault {
                                        FaultKind::AudioDeadAir => 1500,
                                        _ => 2500,
                                    };
                                    spawn_clip_save(
                                        std::sync::Arc::clone(&clip_saver),
                                        clip_saved_tx.clone(),
                                        args.channel_id.clone(),
                                        Some(fault),
                                        current_url.clone(),
                                        ring.snapshot_from_pretrigger_idr(pretrigger_ms),
                                        ring.video_config().cloned(),
                                        ring.audio_config().cloned(),
                                    );
                                }
                                SupervisorToWorker::Shutdown => break,
                            }
                        } else if !raw.trim().is_empty() {
                            tracing::warn!(channel_id = %args.channel_id, "Ignored invalid supervisor IPC command: {raw}");
                        }
                    }
                    None => break,
                }
            }
            Some(clip_msg) = clip_saved_rx.recv() => {
                let _ = emit(&clip_msg).await;
            }
            _ = tick_interval.tick() => {
                tick += 1;

                let last_probe_outcome = probe_outcome_rx.borrow().clone();
                if let Some(ref p) = last_probe_outcome
                    && p.token_expired
                    && token_expired_sent_for.as_deref() != Some(current_url.as_str())
                {
                    token_expired_sent_for = Some(current_url.clone());
                    let _ = emit(&WorkerToSupervisor::StreamError {
                        channel_id: args.channel_id.clone(),
                        code: "TOKEN_EXPIRED".into(),
                        message: "Upstream URL returned 401/403/410; re-resolving".into(),
                        token_expired: true,
                        gpu_fault: false,
                    }).await;
                    continue;
                }

                let light_mode = tier == FeedTier::Light || force_cpu_light;
                let min_thumb_gap_ms = (1000.0 / thumb_fps.max(0.2)) as u128;
                let emit_thumb = last_thumb_time.elapsed().as_millis() >= min_thumb_gap_ms;

                let maybe_batch = if let Some(ref reader) = live_reader {
                    if reader.has_ended() {
                        let _ = emit(&WorkerToSupervisor::StreamError {
                            channel_id: args.channel_id.clone(),
                            code: "STREAM_EOF".into(),
                            message: "FFmpeg stream ended; triggering URL re-resolution".into(),
                            token_expired: true,
                            gpu_fault: false,
                        })
                        .await;
                        live_reader = None;
                        continue;
                    }
                    reader.poll_batch(light_mode, tick, emit_thumb)
                } else if current_url.starts_with("sim://") {
                    Some(synthesize_tick(
                        &current_url,
                        platform,
                        args.vertical,
                        light_mode,
                        tick,
                        emit_thumb,
                    ))
                } else {
                    None
                };

                if let Some(batch) = maybe_batch {
                    if !has_media_packet {
                        has_media_packet = true;
                        last_telemetry_emit = Instant::now() - Duration::from_secs(2);
                    }
                    last_rx_time = Instant::now();
                    last_rx_kbps = batch.rx_kbps;
                    last_batch_ad_break = batch.ad_break;
                    last_batch_faults.clear();
                    if batch.manifest_stalled {
                        last_batch_faults.push(FaultKind::StreamStall);
                    }
                    if batch.corrupt_decode {
                        last_batch_faults.push(FaultKind::CorruptDecode);
                    }
                    if batch.pts_drift {
                        last_batch_faults.push(FaultKind::PtsDrift);
                    }
                    if is_av_sync_drifted(batch.av_sync_ms, &mut av_sync_bad_ms, 100) {
                        last_batch_faults.push(FaultKind::AvSyncDrift);
                    }
                    if batch.gpu_fault {
                        last_batch_faults.push(FaultKind::WorkerGpuFault);
                        let _ = emit(&WorkerToSupervisor::StreamError {
                            channel_id: args.channel_id.clone(),
                            code: "GPU_DECODE_FAULT".into(),
                            message: "D3D11VA hardware decode failed; fell back to software decode".into(),
                            token_expired: false,
                            gpu_fault: true,
                        })
                        .await;
                    }
                    if let Some(vcfg) = batch.video_codec_cfg {
                        ring.set_video_config(vcfg);
                    }
                    if let Some(acfg) = batch.audio_codec_cfg {
                        ring.set_audio_config(acfg);
                    }
                    for pkt in batch.packets {
                        ring.push(pkt);
                    }

                    if !batch.pcm_f32.is_empty() {
                        let (audio_tel, a_faults) =
                            dsp.process_f32_interleaved(&batch.pcm_f32, group_ref_lufs);
                        last_audio_tel = audio_tel.clone();
                        last_audio_faults = a_faults;

                        // Real-time 10 Hz audio meter push
                        let _ = emit(&WorkerToSupervisor::Meter {
                            channel_id: args.channel_id.clone(),
                            audio: audio_tel,
                        })
                        .await;

                        if pfl_enabled {
                            let pcm_b64 = encode_s16le_b64(&batch.pcm_f32);
                            let _ = emit(&WorkerToSupervisor::PflAudio {
                                channel_id: args.channel_id.clone(),
                                timestamp_ms: tick * 100,
                                sample_rate: 48_000,
                                channels: 2,
                                pcm_s16le_b64: pcm_b64,
                            })
                            .await;
                        }
                    }

                    let cdn_ratio = last_probe_outcome
                        .as_ref()
                        .map(|p| p.cdn_ratio)
                        .unwrap_or(batch.cdn_ratio);

                    if light_mode {
                        last_video_faults.clear();
                        last_video_tel = VideoTelemetry {
                            width: 0,
                            height: 0,
                            fps: 0.0,
                            bitrate_kbps: batch.bitrate_kbps,
                            avg_qp: 0.0,
                            luma_mean: 0.0,
                            frame_mad: 0.0,
                            luma_variance: 0.0,
                            changed_pct: 0.0,
                            blockiness_score: 0.0,
                            decode_ms: batch.decode_ms,
                            av_sync_ms: batch.av_sync_ms,
                            cdn_ratio,
                            hw_accelerated: false,
                        };
                    } else if batch.has_new_video && !batch.y_plane.is_empty() {
                        let v_elapsed_ms = (last_video_time.elapsed().as_millis() as u64).clamp(50, 2000);
                        last_video_time = Instant::now();
                        let vres = simd.analyze_y_plane(&batch.y_plane, v_elapsed_ms, batch.avg_qp);
                        last_video_faults = vres.faults;
                        last_video_tel = VideoTelemetry {
                            width: if batch.width > 0 {
                                batch.width
                            } else if args.vertical {
                                1080
                            } else {
                                1920
                            },
                            height: if batch.height > 0 {
                                batch.height
                            } else if args.vertical {
                                1920
                            } else {
                                1080
                            },
                            fps: if batch.fps > 0.0 {
                                batch.fps
                            } else if tier == FeedTier::Master {
                                60.0
                            } else {
                                30.0
                            },
                            bitrate_kbps: batch.bitrate_kbps,
                            avg_qp: batch.avg_qp,
                            luma_mean: vres.luma_mean,
                            frame_mad: vres.frame_mad,
                            luma_variance: vres.luma_variance,
                            changed_pct: vres.changed_pct,
                            blockiness_score: vres.blockiness_score,
                            decode_ms: batch.decode_ms,
                            av_sync_ms: batch.av_sync_ms,
                            cdn_ratio,
                            hw_accelerated: batch.hw_accelerated,
                        };
                    } else {
                        last_video_tel.bitrate_kbps = batch.bitrate_kbps;
                        last_video_tel.decode_ms = batch.decode_ms;
                        last_video_tel.av_sync_ms = batch.av_sync_ms;
                        last_video_tel.cdn_ratio = cdn_ratio;
                        last_video_tel.hw_accelerated = batch.hw_accelerated;
                    }

                    if let Some(jpeg) = batch.jpeg_b64 {
                        last_thumb_time = Instant::now();
                        let _ = emit(&WorkerToSupervisor::Thumbnail {
                            channel_id: args.channel_id.clone(),
                            timestamp: Utc::now(),
                            jpeg_b64: jpeg,
                        })
                        .await;
                    }
                }

                let stall_threshold = if has_media_packet {
                    Duration::from_secs(8)
                } else {
                    Duration::from_secs(15)
                };
                // Emit full state telemetry immediately on first decoded batch, then every 500ms
                if (has_media_packet || last_rx_time.elapsed() >= stall_threshold)
                    && last_telemetry_emit.elapsed() >= Duration::from_millis(500)
                {
                    let emit_elapsed_ms = last_telemetry_emit
                        .elapsed()
                        .as_millis()
                        .clamp(100, 2000) as u64;
                    last_telemetry_emit = Instant::now();
                    let mut faults = last_audio_faults.clone();
                    faults.extend(last_video_faults.iter().copied());
                    faults.extend(last_batch_faults.iter().copied());

                    if last_rx_time.elapsed() >= stall_threshold {
                        faults.push(FaultKind::StreamStall);
                    } else if let Some(ref p) = last_probe_outcome {
                        if p.stalled && last_rx_time.elapsed() >= Duration::from_secs(6) {
                            faults.push(FaultKind::StreamStall);
                        }
                        if p.cdn_starvation {
                            faults.push(FaultKind::CdnStarvation);
                        }
                    }

                    let light_only = tier == FeedTier::Light || force_cpu_light;
                    if !light_only
                        && bitrate_tracker.update(&last_video_tel, emit_elapsed_ms)
                    {
                        faults.push(FaultKind::BitrateCollapse);
                    }

                    let ad_break = last_batch_ad_break
                        || last_probe_outcome.as_ref().map(|p| p.ad_break).unwrap_or(false);
                    let _ = emit(&WorkerToSupervisor::Telemetry {
                        channel_id: args.channel_id.clone(),
                        timestamp: Utc::now(),
                        audio: last_audio_tel.clone(),
                        video: last_video_tel.clone(),
                        active_faults: faults,
                        ad_break,
                        rx_kbps: last_rx_kbps,
                    })
                    .await;
                }
            }
        }
    }

    Ok(())
}

fn encode_s16le_b64(pcm_f32: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(pcm_f32.len() * 2);
    for &s in pcm_f32 {
        let clamped = s.clamp(-1.0, 1.0);
        let i16_val = (clamped * 32767.0) as i16;
        bytes.extend_from_slice(&i16_val.to_le_bytes());
    }
    B64.encode(&bytes)
}

const BITRATE_WARMUP_MS: u64 = 5_000;
const BITRATE_COLLAPSE_HOLD_MS: u64 = 6_000;

#[derive(Default)]
struct BitrateCollapseTracker {
    warmup_ms: u64,
    baseline_kbps: Option<f32>,
    collapse_bad_ms: u64,
}

fn is_static_vbr_saving(video: &VideoTelemetry) -> bool {
    let low_motion = video.changed_pct < 2.5 && video.frame_mad < 1.5;
    let clean_quality = video.avg_qp <= 44.0 && video.blockiness_score <= 3.2;
    low_motion && clean_quality && video.bitrate_kbps > 20.0
}

impl BitrateCollapseTracker {
    fn reset(&mut self) {
        self.warmup_ms = 0;
        self.baseline_kbps = None;
        self.collapse_bad_ms = 0;
    }

    fn update(&mut self, video: &VideoTelemetry, elapsed_ms: u64) -> bool {
        let kbps = video.bitrate_kbps;
        if self.warmup_ms < BITRATE_WARMUP_MS {
            if kbps > 50.0 {
                self.warmup_ms = self.warmup_ms.saturating_add(elapsed_ms);
            }
            return false;
        }
        let Some(base) = self.baseline_kbps else {
            if kbps > 150.0 {
                self.baseline_kbps = Some(kbps);
            }
            return false;
        };
        if base > 400.0 && kbps < base * 0.35 {
            if is_static_vbr_saving(video) {
                // Static/clean slides legitimately drop CRF/VBR bitrate well below 35%;
                // pause collapse timer and preserve prior active baseline until motion resumes.
                self.collapse_bad_ms = 0;
            } else {
                self.collapse_bad_ms = self.collapse_bad_ms.saturating_add(elapsed_ms);
            }
        } else {
            self.collapse_bad_ms = 0;
            if kbps > 150.0 {
                self.baseline_kbps = Some(base * 0.96 + kbps * 0.04);
            }
        }
        self.collapse_bad_ms >= BITRATE_COLLAPSE_HOLD_MS
    }
}

fn is_av_sync_drifted(av_sync_ms: f32, bad_ms: &mut u64, elapsed_ms: u64) -> bool {
    if av_sync_ms.abs() > 150.0 {
        *bad_ms = bad_ms.saturating_add(elapsed_ms);
    } else {
        *bad_ms = 0;
    }
    *bad_ms >= 3000
}

#[allow(clippy::too_many_arguments)]
fn spawn_clip_save(
    clip_saver: std::sync::Arc<ClipSaver>,
    tx: tokio::sync::mpsc::UnboundedSender<WorkerToSupervisor>,
    channel_id: String,
    fault: Option<FaultKind>,
    source_url: String,
    pkts: Vec<ring_buffer::CompressedPacket>,
    video_cfg: Option<ring_buffer::StreamCodecConfig>,
    audio_cfg: Option<ring_buffer::StreamCodecConfig>,
) {
    tokio::task::spawn_blocking(move || {
        if let Ok(filename) = clip_saver.save_clip(
            &channel_id,
            fault,
            Some(&source_url),
            &pkts,
            video_cfg.as_ref(),
            audio_cfg.as_ref(),
            Utc::now(),
        ) {
            let _ = tx.send(WorkerToSupervisor::ClipSaved {
                channel_id,
                fault,
                filename,
                duration_secs: 5.0,
                saved_at: Utc::now(),
            });
        }
    });
}

async fn emit(msg: &WorkerToSupervisor) -> anyhow::Result<()> {
    let mut stdout = tokio::io::stdout();
    let line = serde_json::to_string(msg)?;
    stdout.write_all(line.as_bytes()).await?;
    stdout.write_all(b"\n").await?;
    stdout.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_av_sync_drift_hysteresis() {
        let mut bad_ms = 0u64;
        for _ in 0..29 {
            assert!(!is_av_sync_drifted(220.0, &mut bad_ms, 100));
        }
        assert!(is_av_sync_drifted(220.0, &mut bad_ms, 100));
        assert!(!is_av_sync_drifted(12.0, &mut bad_ms, 100));
        assert_eq!(bad_ms, 0);
    }

    #[test]
    fn test_bitrate_collapse_tracker_ignores_warmup_and_vbr_dips() {
        let mk_video = |kbps: f32, changed_pct: f32, frame_mad: f32, avg_qp: f32| VideoTelemetry {
            bitrate_kbps: kbps,
            changed_pct,
            frame_mad,
            avg_qp,
            blockiness_score: 1.0,
            ..VideoTelemetry::default()
        };

        let mut t = BitrateCollapseTracker::default();
        // Cold-start HLS spike (30 Mb/s for 5s) must not lock an inflated baseline
        let burst = mk_video(30_000.0, 10.0, 3.0, 28.0);
        for _ in 0..10 {
            assert!(!t.update(&burst, 500));
        }
        // Baseline seeds at settled 5,000 kbps after warmup
        let nominal = mk_video(5_000.0, 10.0, 3.0, 28.0);
        assert!(!t.update(&nominal, 500));
        // Quiet VBR scene down to 2,200 kbps (44%) for 20s must not trigger
        let mild_vbr = mk_video(2_200.0, 6.0, 2.0, 31.0);
        for _ in 0..40 {
            assert!(!t.update(&mild_vbr, 500));
        }
        // Very low VBR bitrate (250 kbps) on a clean static slide must NOT trigger BitrateCollapse
        let static_slide = mk_video(250.0, 0.8, 0.4, 34.0);
        for _ in 0..30 {
            assert!(!t.update(&static_slide, 500));
        }
        // Transient 4s drop below 35% with active motion must not trigger before 6s hysteresis
        let motion_collapse = mk_video(300.0, 12.0, 4.5, 39.0);
        for _ in 0..8 {
            assert!(!t.update(&motion_collapse, 500));
        }
        // Recovery resets timer
        assert!(!t.update(&mk_video(4_000.0, 9.0, 2.8, 30.0), 500));
        // Sustained severe collapse (<35%) with active motion triggers once >= 6s elapses
        let severe = mk_video(200.0, 11.0, 4.0, 40.0);
        for i in 0..12 {
            let tripped = t.update(&severe, 500);
            if i < 11 {
                assert!(!tripped, "tripped too early at step {i}");
            } else {
                assert!(tripped, "expected collapse at 6s");
            }
        }
        // Static low-motion scene with severe macroblocking/QP degradation MUST still alert
        t.reset();
        for _ in 0..10 {
            t.update(&nominal, 500);
        }
        assert!(!t.update(&nominal, 500));
        let crushed_static = mk_video(180.0, 0.5, 0.3, 48.0);
        for i in 0..12 {
            let tripped = t.update(&crushed_static, 500);
            if i < 11 {
                assert!(!tripped, "tripped too early at crushed step {i}");
            } else {
                assert!(tripped, "expected crushed low-motion collapse at 6s");
            }
        }
    }
}
