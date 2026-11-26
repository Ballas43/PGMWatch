use crate::backend::{DecodedFrameBatch, MediaOpenOptions};
use crate::dsp::{CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE};
use crate::ring_buffer::{CompressedPacket, StreamCodecConfig};
use crate::telemetry::{estimate_qp_from_bpp, BitrateEstimator};
use ffmpeg_sys_next as ff;
use std::collections::hash_map::DefaultHasher;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::os::raw::{c_int, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

mod demux;
#[cfg(test)]
mod tests;

const AUDIO_CHUNK_SAMPLES: usize =
    (CANONICAL_SAMPLE_RATE as usize / 10) * (CANONICAL_CHANNELS as usize);
const MAX_PACKETS_QUEUED: usize = 400;

struct FfiTelemetryState {
    width: u32,
    height: u32,
    fps: f32,
    decode_ms: f32,
    last_audio_pts_ms: Option<i64>,
    last_video_pts_ms: Option<i64>,
    prev_stream_audio_pts_ms: Option<i64>,
    prev_stream_video_pts_ms: Option<i64>,
    pts_drift_pending: bool,
    corrupt_pending: bool,
    av_sync_ms: f32,
    avg_qp: f32,
    bitrate: BitrateEstimator,
}

impl Default for FfiTelemetryState {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            fps: 0.0,
            decode_ms: 0.0,
            last_audio_pts_ms: None,
            last_video_pts_ms: None,
            prev_stream_audio_pts_ms: None,
            prev_stream_video_pts_ms: None,
            pts_drift_pending: false,
            corrupt_pending: false,
            av_sync_ms: 0.0,
            avg_qp: 22.0,
            bitrate: BitrateEstimator::default(),
        }
    }
}

struct FfiSharedState {
    audio_queue: Mutex<VecDeque<Vec<f32>>>,
    latest_rgb: Mutex<Vec<u8>>,
    rgb_seq: AtomicU64,
    last_consumed_rgb_seq: AtomicU64,
    packets: Mutex<VecDeque<CompressedPacket>>,
    video_codec_cfg: Mutex<Option<StreamCodecConfig>>,
    audio_codec_cfg: Mutex<Option<StreamCodecConfig>>,
    video_cfg_dirty: AtomicBool,
    audio_cfg_dirty: AtomicBool,
    telemetry: Mutex<FfiTelemetryState>,
    gpu_fault_pending: AtomicBool,
    hw_accelerated: AtomicBool,
    ended: AtomicBool,
    split_av_pair: bool,
    stop_flag: Arc<AtomicBool>,
}

pub struct FfiFfmpegReader {
    shared: Arc<FfiSharedState>,
    threads: Vec<JoinHandle<()>>,
    width: usize,
    height: usize,
}

impl FfiFfmpegReader {
    pub fn start(
        url: &str,
        audio_url: Option<&str>,
        headers: &[(String, String)],
        vertical: bool,
        light_mode: bool,
        opts: &MediaOpenOptions,
    ) -> anyhow::Result<Self> {
        let (tw, th) = if vertical { (180usize, 320usize) } else { (320usize, 180usize) };
        let stop_flag = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(FfiSharedState {
            audio_queue: Mutex::new(VecDeque::with_capacity(64)),
            latest_rgb: Mutex::new(Vec::new()),
            rgb_seq: AtomicU64::new(0),
            last_consumed_rgb_seq: AtomicU64::new(0),
            packets: Mutex::new(VecDeque::with_capacity(MAX_PACKETS_QUEUED)),
            video_codec_cfg: Mutex::new(None),
            audio_codec_cfg: Mutex::new(None),
            video_cfg_dirty: AtomicBool::new(false),
            audio_cfg_dirty: AtomicBool::new(false),
            telemetry: Mutex::new(FfiTelemetryState::default()),
            gpu_fault_pending: AtomicBool::new(false),
            hw_accelerated: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            split_av_pair: audio_url.is_some() && !light_mode,
            stop_flag: Arc::clone(&stop_flag),
        });

        let mut threads = Vec::new();
        let headers_vec = headers.to_vec();

        if let Some(au) = audio_url {
            spawn_demux_worker(
                "audio",
                &shared,
                &stop_flag,
                &mut threads,
                DemuxTask {
                    url: au.to_string(),
                    headers: headers_vec.clone(),
                    want_audio: true,
                    want_video: false,
                    thumb_w: 0,
                    thumb_h: 0,
                    media_opts: opts.clone(),
                },
            )?;

            if !light_mode {
                spawn_demux_worker(
                    "video",
                    &shared,
                    &stop_flag,
                    &mut threads,
                    DemuxTask {
                        url: url.to_string(),
                        headers: headers_vec,
                        want_audio: false,
                        want_video: true,
                        thumb_w: tw,
                        thumb_h: th,
                        media_opts: opts.clone(),
                    },
                )?;
            }
        } else {
            spawn_demux_worker(
                "stream",
                &shared,
                &stop_flag,
                &mut threads,
                DemuxTask {
                    url: url.to_string(),
                    headers: headers_vec,
                    want_audio: true,
                    want_video: !light_mode,
                    thumb_w: tw,
                    thumb_h: th,
                    media_opts: opts.clone(),
                },
            )?;
        }

        Ok(Self {
            shared,
            threads,
            width: tw,
            height: th,
        })
    }

    pub fn has_ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    pub fn poll_batch(&self, light_mode: bool, _tick: u64, emit_thumb: bool) -> Option<DecodedFrameBatch> {
        let pcm = self
            .shared
            .audio_queue
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
            .unwrap_or_default();

        let curr_seq = self.shared.rgb_seq.load(Ordering::Acquire);
        let prev_seq = self.shared.last_consumed_rgb_seq.load(Ordering::Relaxed);
        let has_new_video = !light_mode && curr_seq != prev_seq;

        let packets: Vec<CompressedPacket> = self
            .shared
            .packets
            .lock()
            .ok()
            .map(|mut q| q.drain(..).collect())
            .unwrap_or_default();

        let gpu_fault = self.shared.gpu_fault_pending.swap(false, Ordering::AcqRel);

        if pcm.is_empty() && !has_new_video && packets.is_empty() && !gpu_fault {
            return None;
        }

        let mut y_plane = Vec::new();
        let mut jpeg_b64 = None;

        if has_new_video {
            self.shared.last_consumed_rgb_seq.store(curr_seq, Ordering::Relaxed);
            let rgb = self
                .shared
                .latest_rgb
                .lock()
                .ok()
                .map(|g| g.clone())
                .unwrap_or_default();
            if rgb.len() >= self.width * self.height * 3 {
                let (yw, yh) = (self.width / 2, self.height / 2);
                y_plane = vec![16u8; yw * yh];
                for row in 0..yh {
                    for col in 0..yw {
                        let src_idx = ((row * 2) * self.width + (col * 2)) * 3;
                        let r = rgb[src_idx] as f32;
                        let g = rgb[src_idx + 1] as f32;
                        let b = rgb[src_idx + 2] as f32;
                        y_plane[row * yw + col] =
                            (0.257 * r + 0.504 * g + 0.098 * b + 16.0).clamp(16.0, 235.0) as u8;
                    }
                }
                if emit_thumb {
                    jpeg_b64 = crate::video_simd::encode_rgb_jpeg_b64(
                        &rgb,
                        self.width as u32,
                        self.height as u32,
                    )
                    .ok();
                }
            }
        }

        let (
            width,
            height,
            fps,
            rx_kbps,
            bitrate_kbps,
            decode_ms,
            av_sync_ms,
            avg_qp,
            corrupt_decode,
            pts_drift,
        ) = if let Ok(mut tel) = self.shared.telemetry.lock() {
            let now = Instant::now();
            let (total_kbps, video_kbps) = tel.bitrate.update(now);

            let corrupt = tel.corrupt_pending;
            tel.corrupt_pending = false;
            let pts_jump = tel.pts_drift_pending;
            tel.pts_drift_pending = false;

            let qp = if tel.avg_qp > 0.0 {
                tel.avg_qp
            } else if tel.width > 0 && tel.height > 0 && video_kbps > 1.0 {
                estimate_qp_from_bpp(video_kbps, tel.width, tel.height, tel.fps.max(15.0))
            } else {
                23.0
            };

            (
                tel.width,
                tel.height,
                tel.fps,
                total_kbps,
                if light_mode {
                    total_kbps
                } else if video_kbps > 0.0 {
                    video_kbps
                } else {
                    total_kbps
                },
                tel.decode_ms,
                tel.av_sync_ms,
                qp,
                corrupt,
                pts_jump,
            )
        } else {
            (0, 0, 0.0, 0.0, 0.0, 0.0, 0.0, 23.0, false, false)
        };

        let video_codec_cfg = if self.shared.video_cfg_dirty.swap(false, Ordering::AcqRel) {
            self.shared.video_codec_cfg.lock().ok().and_then(|g| g.clone())
        } else {
            None
        };
        let audio_codec_cfg = if self.shared.audio_cfg_dirty.swap(false, Ordering::AcqRel) {
            self.shared.audio_codec_cfg.lock().ok().and_then(|g| g.clone())
        } else {
            None
        };

        Some(DecodedFrameBatch {
            y_plane,
            has_new_video,
            jpeg_b64,
            pcm_f32: pcm,
            width,
            height,
            fps,
            rx_kbps,
            bitrate_kbps,
            decode_ms,
            av_sync_ms,
            avg_qp,
            cdn_ratio: 0.2,
            hw_accelerated: self.shared.hw_accelerated.load(Ordering::Relaxed),
            ad_break: false,
            manifest_stalled: false,
            corrupt_decode,
            pts_drift,
            gpu_fault,
            packets,
            video_codec_cfg,
            audio_codec_cfg,
        })
    }
}

impl Drop for FfiFfmpegReader {
    fn drop(&mut self) {
        self.shared.stop_flag.store(true, Ordering::Release);
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}

unsafe extern "C" fn interrupt_cb(opaque: *mut c_void) -> c_int {
    if opaque.is_null() {
        return 0;
    }
    let stop = unsafe { &*(opaque as *const AtomicBool) };
    if stop.load(Ordering::Relaxed) {
        1
    } else {
        0
    }
}

fn hash_codec_cfg(cfg: &StreamCodecConfig) -> u64 {
    let mut h = DefaultHasher::new();
    cfg.codec_id.hash(&mut h);
    cfg.width.hash(&mut h);
    cfg.height.hash(&mut h);
    cfg.sample_rate.hash(&mut h);
    cfg.channels.hash(&mut h);
    cfg.extradata.hash(&mut h);
    h.finish()
}

unsafe fn extract_codec_config(st: *const ff::AVStream) -> StreamCodecConfig {
    let par = unsafe { (*st).codecpar };
    let extra_len = unsafe { (*par).extradata_size.max(0) as usize };
    let extradata = if extra_len > 0 && unsafe { !(*par).extradata.is_null() } {
        unsafe { std::slice::from_raw_parts((*par).extradata, extra_len).to_vec() }
    } else {
        Vec::new()
    };
    unsafe {
        StreamCodecConfig {
            codec_id: (*par).codec_id as i32,
            extradata,
            width: (*par).width,
            height: (*par).height,
            sample_rate: (*par).sample_rate,
            channels: (*par).ch_layout.nb_channels,
            bit_rate: (*par).bit_rate,
        }
    }
}

fn ts_to_ms(ts: i64, tb: ff::AVRational) -> Option<i64> {
    if ts == ff::AV_NOPTS_VALUE || tb.den <= 0 || tb.num <= 0 {
        return None;
    }
    let ms = unsafe { ff::av_rescale_q(ts, tb, ff::AVRational { num: 1, den: 1000 }) };
    Some(ms)
}

fn sync_split_av_peer(
    shared: &FfiSharedState,
    want_audio: bool,
    want_video: bool,
    raw_pts_ms: Option<i64>,
    waited_for_video_start: &mut bool,
) {
    if want_audio && !want_video && !*waited_for_video_start {
        let wait_start = Instant::now();
        while !shared.stop_flag.load(Ordering::Relaxed) && wait_start.elapsed() < Duration::from_secs(4) {
            let has_video = shared
                .telemetry
                .lock()
                .ok()
                .and_then(|t| t.last_video_pts_ms)
                .is_some();
            if has_video {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
        *waited_for_video_start = true;
    }

    let Some(p) = raw_pts_ms.filter(|&v| v >= 0) else {
        return;
    };
    if want_audio && !want_video {
        let vp = shared.telemetry.lock().ok().and_then(|t| t.last_video_pts_ms);
        if let Some(v) = vp
            && p > v + 220
            && p - v < 15_000
        {
            thread::sleep(Duration::from_millis(20));
        }
    } else if want_video && !want_audio {
        let ap = shared.telemetry.lock().ok().and_then(|t| t.last_audio_pts_ms);
        if let Some(a) = ap
            && p > a + 350
            && p - a < 15_000
        {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

struct DemuxTask {
    url: String,
    headers: Vec<(String, String)>,
    want_audio: bool,
    want_video: bool,
    thumb_w: usize,
    thumb_h: usize,
    media_opts: MediaOpenOptions,
}

fn spawn_demux_worker(
    label: &str,
    shared: &Arc<FfiSharedState>,
    stop_flag: &Arc<AtomicBool>,
    threads: &mut Vec<JoinHandle<()>>,
    task: DemuxTask,
) -> anyhow::Result<()> {
    let (tx, rx) = mpsc::sync_channel::<Result<(), String>>(1);
    let sh = Arc::clone(shared);
    threads.push(thread::spawn(move || {
        demux::run_demux_loop(sh, task, tx);
    }));
    match rx.recv_timeout(Duration::from_secs(6)) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            stop_flag.store(true, Ordering::Release);
            for h in threads.drain(..) {
                let _ = h.join();
            }
            anyhow::bail!("FFI {label} open failed: {e}");
        }
        Err(_) => {
            stop_flag.store(true, Ordering::Release);
            for h in threads.drain(..) {
                let _ = h.join();
            }
            anyhow::bail!("FFI {label} open timed out");
        }
    }
}
