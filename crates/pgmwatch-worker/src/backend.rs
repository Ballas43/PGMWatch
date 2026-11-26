use crate::dsp::{CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE};
use crate::ring_buffer::{CompressedPacket, StreamCodecConfig};
use crate::video_simd::encode_rgb_jpeg_b64;
use pgmwatch_core::models::Platform;
use std::collections::VecDeque;
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct MediaOpenOptions {
    pub gpu_mode: String,
    pub hwaccel_device: Option<String>,
    pub extra_hw_frames: u32,
    pub bind_ip: Option<String>,
}

impl Default for MediaOpenOptions {
    fn default() -> Self {
        Self {
            gpu_mode: "auto".to_string(),
            hwaccel_device: None,
            extra_hw_frames: 6,
            bind_ip: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DecodedFrameBatch {
    pub y_plane: Vec<u8>,
    pub has_new_video: bool,
    pub jpeg_b64: Option<String>,
    pub pcm_f32: Vec<f32>,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub rx_kbps: f32,
    pub bitrate_kbps: f32,
    pub decode_ms: f32,
    pub av_sync_ms: f32,
    pub avg_qp: f32,
    pub cdn_ratio: f32,
    pub hw_accelerated: bool,
    pub ad_break: bool,
    pub manifest_stalled: bool,
    pub corrupt_decode: bool,
    pub pts_drift: bool,
    pub gpu_fault: bool,
    pub packets: Vec<CompressedPacket>,
    pub video_codec_cfg: Option<StreamCodecConfig>,
    pub audio_codec_cfg: Option<StreamCodecConfig>,
}

pub fn find_ffmpeg9_binary() -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for rel in ["bin/ffmpeg.exe", "ffmpeg.exe"] {
            let cand = dir.join(rel);
            if cand.exists() {
                return cand.display().to_string();
            }
        }
    }
    for candidate in [
        "vendor/ffmpeg-9.0/bin/ffmpeg.exe",
        "bin/ffmpeg.exe",
        "ffmpeg.exe",
    ] {
        if Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "ffmpeg".to_string()
}

pub struct LiveFfmpegReader {
    child_audio: Option<Child>,
    child_video: Arc<Mutex<Option<Child>>>,
    audio_queue: Arc<Mutex<VecDeque<Vec<f32>>>>,
    latest_rgb: Arc<Mutex<Vec<u8>>>,
    rgb_seq: Arc<AtomicU64>,
    last_consumed_rgb_seq: AtomicU64,
    audio_eof: Arc<AtomicBool>,
    pub hw_accelerated: bool,
    width: usize,
    height: usize,
}

#[allow(clippy::too_many_arguments)]
fn append_common_ffmpeg_input_args(
    args: &mut Vec<String>,
    input_url: &str,
    header_str: &str,
    bind_ip: Option<&str>,
    use_hw: bool,
    hwaccel_device: Option<&str>,
    extra_hw_frames: u32,
) {
    let lower = input_url.to_ascii_lowercase();
    // Disable FFmpeg interactive keyboard reads on fd 0 so child processes never
    // compete with the worker's JSON IPC loop over an inherited stdin pipe.
    args.extend([
        "-nostdin".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-analyzeduration".to_string(),
        "1500000".to_string(),
        "-probesize".to_string(),
        "2000000".to_string(),
    ]);
    if lower.contains(".m3u8") || lower.contains(".mpd") {
        args.extend([
            "-fflags".to_string(),
            "+nobuffer".to_string(),
            "-flags".to_string(),
            "low_delay".to_string(),
            "-http_persistent".to_string(),
            "0".to_string(),
        ]);
    } else if lower.starts_with("http://") || lower.starts_with("https://") {
        args.extend([
            "-reconnect".to_string(),
            "1".to_string(),
            "-reconnect_streamed".to_string(),
            "1".to_string(),
        ]);
    }
    if !lower.starts_with("srt://")
        && (lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("rtmp://")
            || lower.starts_with("rtmps://"))
        && let Some(ip) = bind_ip
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.parse::<std::net::IpAddr>().is_ok())
    {
        args.extend(["-local_addr".to_string(), ip.to_string()]);
    }
    if use_hw {
        args.extend(["-hwaccel".to_string(), "d3d11va".to_string()]);
        if let Some(dev) = hwaccel_device.filter(|s| !s.trim().is_empty()) {
            args.extend(["-hwaccel_device".to_string(), dev.trim().to_string()]);
        }
        if extra_hw_frames > 0 {
            args.extend(["-extra_hw_frames".to_string(), extra_hw_frames.to_string()]);
        }
    }
    if !header_str.is_empty() {
        args.extend(["-headers".to_string(), header_str.to_string()]);
    }
    args.extend([
        "-readrate".to_string(),
        "1.0".to_string(),
        "-readrate_initial_burst".to_string(),
        "3.5".to_string(),
        "-readrate_catchup".to_string(),
        "1.5".to_string(),
        "-i".to_string(),
        input_url.to_string(),
    ]);
}

pub(crate) fn build_ffmpeg_audio_args(
    audio_src: &str,
    header_str: &str,
    bind_ip: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();
    append_common_ffmpeg_input_args(&mut args, audio_src, header_str, bind_ip, false, None, 0);
    args.extend([
        "-vn".to_string(),
        "-af".to_string(),
        "aresample=async=1".to_string(),
        "-ac".to_string(),
        "2".to_string(),
        "-ar".to_string(),
        "48000".to_string(),
        "-f".to_string(),
        "f32le".to_string(),
        "-flush_packets".to_string(),
        "1".to_string(),
        "pipe:1".to_string(),
    ]);
    args
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_ffmpeg_video_args(
    vid_url: &str,
    header_str: &str,
    bind_ip: Option<&str>,
    use_hw: bool,
    hwaccel_device: Option<&str>,
    extra_hw_frames: u32,
    tw: usize,
    th: usize,
) -> Vec<String> {
    let mut args = Vec::new();
    append_common_ffmpeg_input_args(
        &mut args,
        vid_url,
        header_str,
        bind_ip,
        use_hw,
        hwaccel_device,
        extra_hw_frames,
    );
    args.extend([
        "-an".to_string(),
        "-vf".to_string(),
        format!("fps=4,scale={tw}:{th}"),
        "-pix_fmt".to_string(),
        "rgb24".to_string(),
        "-f".to_string(),
        "rawvideo".to_string(),
        "pipe:1".to_string(),
    ]);
    args
}

impl LiveFfmpegReader {
    pub fn start(
        url: &str,
        audio_url: Option<&str>,
        headers: &[(String, String)],
        vertical: bool,
        light_mode: bool,
        opts: &MediaOpenOptions,
    ) -> anyhow::Result<Self> {
        let ffmpeg = find_ffmpeg9_binary();
        let mut header_str = String::new();
        for (k, v) in headers {
            let ck: String = k.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
            let cv: String = v.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
            if !ck.is_empty() && !cv.is_empty() {
                header_str.push_str(&format!("{ck}: {cv}\r\n"));
            }
        }

        let audio_src = audio_url.unwrap_or(url);
        let audio_queue = Arc::new(Mutex::new(VecDeque::with_capacity(64)));
        let latest_rgb = Arc::new(Mutex::new(Vec::new()));
        let rgb_seq = Arc::new(AtomicU64::new(0));
        let audio_eof = Arc::new(AtomicBool::new(false));

        use pgmwatch_core::process::BackgroundCommandExt;
        let mut audio_cmd = Command::new(&ffmpeg);
        audio_cmd
            .no_window()
            .args(build_ffmpeg_audio_args(
                audio_src,
                &header_str,
                opts.bind_ip.as_deref(),
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child_audio = audio_cmd.spawn()?;
        if let Some(mut stdout) = child_audio.stdout.take() {
            let sink = Arc::clone(&audio_queue);
            let eof_flag = Arc::clone(&audio_eof);
            std::thread::spawn(move || {
                // 100ms chunk (4800 stereo frames = 38,400 bytes) for crisp 10 Hz real-time VU metering
                let mut buf = vec![0u8; (CANONICAL_SAMPLE_RATE as usize / 10) * 2 * 4];
                while let Ok(()) = stdout.read_exact(&mut buf) {
                    let mut floats = Vec::with_capacity(buf.len() / 4);
                    for c in buf.as_chunks::<4>().0 {
                        floats.push(f32::from_le_bytes(*c));
                    }
                    if let Ok(mut q) = sink.lock() {
                        // Keep CLI PFL queue bounded (~800ms) so HLS segment bursts don't add lag.
                        while q.len() >= 8 {
                            q.pop_front();
                        }
                        q.push_back(floats);
                    }
                }
                eof_flag.store(true, Ordering::Relaxed);
            });
        }

        let (tw, th) = if vertical { (180usize, 320usize) } else { (320usize, 180usize) };
        let mut hw_accelerated = false;
        let child_video_slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let effective_light = light_mode || opts.gpu_mode.eq_ignore_ascii_case("cpu_light");
        if !effective_light {
            let ffmpeg_bin = ffmpeg.clone();
            let header_copy = header_str.clone();
            let vid_url = url.to_string();
            let dev_copy = opts.hwaccel_device.clone();
            let bind_copy = opts.bind_ip.clone();
            let extra_frames = opts.extra_hw_frames;

            let spawn_vid = move |use_hw: bool| {
                let mut vid_cmd = Command::new(&ffmpeg_bin);
                vid_cmd
                    .no_window()
                    .args(build_ffmpeg_video_args(
                        &vid_url,
                        &header_copy,
                        bind_copy.as_deref(),
                        use_hw,
                        dev_copy.as_deref(),
                        extra_frames,
                        tw,
                        th,
                    ))
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null());
                vid_cmd.spawn()
            };

            if let Ok(mut child) = spawn_vid(true) {
                hw_accelerated = true;
                let stdout_opt = child.stdout.take();
                if let Ok(mut slot) = child_video_slot.lock() {
                    *slot = Some(child);
                }
                if let Some(mut stdout) = stdout_opt {
                    let sink = Arc::clone(&latest_rgb);
                    let seq = Arc::clone(&rgb_seq);
                    let slot_clone = Arc::clone(&child_video_slot);
                    let frame_bytes = tw * th * 3;
                    std::thread::spawn(move || {
                        let mut buf = vec![0u8; frame_bytes];
                        let mut frames_read = 0u64;
                        while let Ok(()) = stdout.read_exact(&mut buf) {
                            frames_read += 1;
                            if let Ok(mut guard) = sink.lock() {
                                *guard = buf.clone();
                                seq.fetch_add(1, Ordering::Release);
                            }
                        }
                        // If hardware d3d11va failed before producing any frames, fall back to software decode
                        if frames_read == 0
                            && let Ok(mut sw_child) = spawn_vid(false)
                        {
                            let sw_stdout = sw_child.stdout.take();
                            if let Ok(mut slot) = slot_clone.lock()
                                && let Some(mut old) = slot.replace(sw_child)
                            {
                                let _ = old.kill();
                            }
                            if let Some(mut sw_out) = sw_stdout {
                                while let Ok(()) = sw_out.read_exact(&mut buf) {
                                    if let Ok(mut guard) = sink.lock() {
                                        *guard = buf.clone();
                                        seq.fetch_add(1, Ordering::Release);
                                    }
                                }
                            }
                        }
                    });
                }
            }
        }

        Ok(Self {
            child_audio: Some(child_audio),
            child_video: child_video_slot,
            audio_queue,
            latest_rgb,
            rgb_seq,
            last_consumed_rgb_seq: AtomicU64::new(0),
            audio_eof,
            hw_accelerated,
            width: tw,
            height: th,
        })
    }

    pub fn has_ended(&self) -> bool {
        self.audio_eof.load(Ordering::Relaxed)
    }

    /// Pops at most one 100ms audio chunk and checks whether a genuinely new video frame arrived.
    /// Never synthesizes artificial 0.0 silence when a frame isn't ready.
    pub fn poll_batch(&self, light_mode: bool, tick: u64, emit_thumb: bool) -> Option<DecodedFrameBatch> {
        let pcm = self
            .audio_queue
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
            .unwrap_or_default();

        let curr_seq = self.rgb_seq.load(Ordering::Acquire);
        let prev_seq = self.last_consumed_rgb_seq.load(Ordering::Relaxed);
        let has_new_video = !light_mode && curr_seq != prev_seq;

        if pcm.is_empty() && !has_new_video {
            return None;
        }

        let mut y_plane = Vec::new();
        let mut jpeg_b64 = None;

        if has_new_video {
            self.last_consumed_rgb_seq.store(curr_seq, Ordering::Relaxed);
            let rgb = self
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
                    jpeg_b64 = encode_rgb_jpeg_b64(&rgb, self.width as u32, self.height as u32).ok();
                }
            }
        }

        let est_kbps = if light_mode {
            192.0
        } else if has_new_video {
            3200.0
        } else {
            192.0
        };
        let (vw, vh, vfps) = if light_mode {
            (0, 0, 0.0)
        } else if self.height > self.width {
            (1080, 1920, 30.0)
        } else {
            (1920, 1080, 30.0)
        };

        Some(DecodedFrameBatch {
            y_plane,
            has_new_video,
            jpeg_b64,
            pcm_f32: pcm,
            width: vw,
            height: vh,
            fps: vfps,
            rx_kbps: est_kbps,
            bitrate_kbps: est_kbps,
            decode_ms: 2.2,
            av_sync_ms: 0.0,
            avg_qp: 23.0,
            cdn_ratio: 0.25,
            hw_accelerated: self.hw_accelerated,
            ad_break: false,
            manifest_stalled: false,
            corrupt_decode: false,
            pts_drift: false,
            gpu_fault: false,
            packets: vec![CompressedPacket {
                pts_ms: (tick * 100) as i64,
                dts_ms: (tick * 100) as i64,
                duration_ms: 100,
                is_keyframe: tick.is_multiple_of(20),
                is_video: !light_mode,
                codec_param_hash: 0x9001,
                data: Vec::new(),
            }],
            video_codec_cfg: None,
            audio_codec_cfg: None,
        })
    }
}

pub enum LiveMediaReader {
    #[cfg(feature = "ffmpeg-ffi")]
    Ffi(crate::ffi_backend::FfiFfmpegReader),
    Cli(LiveFfmpegReader),
}

impl LiveMediaReader {
    pub fn start(
        url: &str,
        audio_url: Option<&str>,
        headers: &[(String, String)],
        vertical: bool,
        light_mode: bool,
        opts: &MediaOpenOptions,
    ) -> anyhow::Result<Self> {
        let effective_light = light_mode || opts.gpu_mode.eq_ignore_ascii_case("cpu_light");
        #[cfg(feature = "ffmpeg-ffi")]
        if !opts.gpu_mode.eq_ignore_ascii_case("cli_ffmpeg") {
            match crate::ffi_backend::FfiFfmpegReader::start(
                url,
                audio_url,
                headers,
                vertical,
                effective_light,
                opts,
            ) {
                Ok(ffi) => {
                    tracing::info!("Worker opened live stream via in-process FFmpeg 9 FFI");
                    return Ok(Self::Ffi(ffi));
                }
                Err(err) => {
                    tracing::warn!("In-process FFmpeg FFI open failed ({err}); falling back to CLI ffmpeg.exe");
                }
            }
        }
        Ok(Self::Cli(LiveFfmpegReader::start(
            url,
            audio_url,
            headers,
            vertical,
            effective_light,
            opts,
        )?))
    }

    pub fn has_ended(&self) -> bool {
        match self {
            #[cfg(feature = "ffmpeg-ffi")]
            Self::Ffi(r) => r.has_ended(),
            Self::Cli(r) => r.has_ended(),
        }
    }

    pub fn poll_batch(&self, light_mode: bool, tick: u64, emit_thumb: bool) -> Option<DecodedFrameBatch> {
        match self {
            #[cfg(feature = "ffmpeg-ffi")]
            Self::Ffi(r) => r.poll_batch(light_mode, tick, emit_thumb),
            Self::Cli(r) => r.poll_batch(light_mode, tick, emit_thumb),
        }
    }
}

impl Drop for LiveFfmpegReader {
    fn drop(&mut self) {
        if let Some(mut c) = self.child_audio.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Ok(mut slot) = self.child_video.lock()
            && let Some(mut c) = slot.take()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// Generates 100ms canonical 48kHz audio + 160x90 / 90x160 Y-plane + 320x180 / 180x320 JPEG frames
/// for `sim://` test channels.
pub fn synthesize_tick(
    url: &str,
    platform: Platform,
    vertical: bool,
    light_mode: bool,
    tick: u64,
    emit_thumb: bool,
) -> DecodedFrameBatch {
    let samples_per_channel = (CANONICAL_SAMPLE_RATE / 10) as usize; // 100ms tick (10 Hz)
    let mut pcm = Vec::with_capacity(samples_per_channel * CANONICAL_CHANNELS as usize);
    let target_amp = match platform {
        Platform::Tiktok | Platform::Bigo => 0.72,
        Platform::Facebook => 0.58,
        _ => 0.52,
    };

    let fault_dead_air = url.contains("dead_air");
    let fault_clip = url.contains("clipping");
    let fault_hum = url.contains("hum");
    let fault_frozen = url.contains("frozen");
    let fault_black = url.contains("black");

    for i in 0..samples_per_channel {
        let t = (tick as f32 * 0.1) + (i as f32 / CANONICAL_SAMPLE_RATE as f32);
        let sample = if fault_dead_air {
            0.0
        } else if fault_clip {
            1.0
        } else if fault_hum {
            0.25 * (2.0 * std::f32::consts::PI * 50.0 * t).sin()
        } else {
            let carrier = (2.0 * std::f32::consts::PI * 220.0 * t).sin() * 0.55
                + (2.0 * std::f32::consts::PI * 330.0 * t).sin() * 0.30
                + (2.0 * std::f32::consts::PI * 440.0 * t).sin() * 0.15;
            let mod_env = 0.72 + 0.28 * (2.0 * std::f32::consts::PI * 1.4 * t).sin();
            target_amp * carrier * mod_env
        };
        pcm.push(sample);
        pcm.push(sample * 0.97);
    }

    let has_new_video = !light_mode && tick.is_multiple_of(2);
    let mut y_plane = Vec::new();
    let mut jpeg_b64 = None;

    if has_new_video {
        let (w, h) = if vertical { (90usize, 160usize) } else { (160usize, 90usize) };
        y_plane = vec![16u8; w * h];
        if !fault_black {
            let phase = if fault_frozen { 0 } else { (tick * 3) as usize };
            for row in 0..h {
                for col in 0..w {
                    let bar = ((col + phase) * 7 / w.max(1)) % 7;
                    let base_y: u8 = match bar {
                        0 => 205,
                        1 => 178,
                        2 => 145,
                        3 => 116,
                        4 => 86,
                        5 => 52,
                        _ => 34,
                    };
                    let scan = if (row + phase / 2) % 24 < 3 { 24 } else { 0 };
                    y_plane[row * w + col] = base_y.saturating_add(scan).min(235);
                }
            }
        }

        if emit_thumb {
            let (tw, th) = if vertical { (180usize, 320usize) } else { (320usize, 180usize) };
            let mut rgb = vec![16u8; tw * th * 3];
            if !fault_black {
                let phase = if fault_frozen { 0 } else { (tick * 6) as usize };
                for row in 0..th {
                    for col in 0..tw {
                        let bar = ((col + phase) * 7 / tw.max(1)) % 7;
                        let (r, g, b): (u8, u8, u8) = match bar {
                            0 => (196, 196, 196),
                            1 => (190, 190, 32),
                            2 => (32, 188, 190),
                            3 => (36, 176, 56),
                            4 => (176, 48, 176),
                            5 => (190, 44, 44),
                            _ => (40, 64, 186),
                        };
                        let idx = (row * tw + col) * 3;
                        let pulse: u8 = if (row + phase) % 36 < 4 { 20 } else { 0 };
                        rgb[idx] = r.saturating_add(pulse);
                        rgb[idx + 1] = g.saturating_add(pulse);
                        rgb[idx + 2] = b.saturating_add(pulse);
                    }
                }
            }
            jpeg_b64 = encode_rgb_jpeg_b64(&rgb, tw as u32, th as u32).ok();
        }
    }

    let (sw, sh) = if light_mode {
        (0, 0)
    } else if vertical {
        (1080, 1920)
    } else {
        (1920, 1080)
    };

    DecodedFrameBatch {
        y_plane,
        has_new_video,
        jpeg_b64,
        pcm_f32: pcm,
        width: sw,
        height: sh,
        fps: if light_mode { 0.0 } else { 30.0 },
        rx_kbps: if light_mode { 192.0 } else { 4200.0 },
        bitrate_kbps: if light_mode { 192.0 } else { 4150.0 },
        decode_ms: if light_mode { 0.3 } else { 2.4 },
        av_sync_ms: 4.0,
        avg_qp: 21.5,
        cdn_ratio: 0.21,
        hw_accelerated: !light_mode,
        ad_break: url.contains("ad_break"),
        manifest_stalled: false,
        corrupt_decode: false,
        pts_drift: false,
        gpu_fault: false,
        packets: vec![CompressedPacket {
            pts_ms: (tick * 100) as i64,
            dts_ms: (tick * 100) as i64,
            duration_ms: 100,
            is_keyframe: tick.is_multiple_of(20),
            is_video: !light_mode,
            codec_param_hash: 0x9000,
            data: vec![0xAA; if light_mode { 300 } else { 8 * 1024 }],
        }],
        video_codec_cfg: None,
        audio_codec_cfg: None,
    }
}

#[cfg(feature = "ffmpeg-ffi")]
pub fn probe_ffmpeg9_ffi_version() -> String {
    unsafe {
        let ver = ffmpeg_sys_next::avcodec_version();
        let major = ver >> 16;
        let minor = (ver >> 8) & 0xff;
        let micro = ver & 0xff;
        format!("ffmpeg-ffi libavcodec {major}.{minor}.{micro}")
    }
}

#[cfg(not(feature = "ffmpeg-ffi"))]
pub fn probe_ffmpeg9_ffi_version() -> String {
    "ffmpeg-9-cli+probe".to_string()
}

#[cfg(test)]
mod tests {
    use super::{build_ffmpeg_audio_args, build_ffmpeg_video_args};

    #[test]
    fn test_worker_ffmpeg_args_disable_stdin() {
        let a_hls = build_ffmpeg_audio_args(
            "https://cdn.example.com/live.m3u8",
            "Referer: https://x/\r\n",
            Some("192.168.10.50"),
        );
        assert_eq!(a_hls.first().map(String::as_str), Some("-nostdin"));
        assert!(a_hls.contains(&"-http_persistent".to_string()));
        assert!(a_hls.contains(&"-headers".to_string()));
        assert!(a_hls.windows(2).any(|w| w == ["-local_addr", "192.168.10.50"]));

        let a_flv = build_ffmpeg_audio_args("https://cdn.example.com/live.flv", "", None);
        assert_eq!(a_flv.first().map(String::as_str), Some("-nostdin"));
        assert!(a_flv.contains(&"-reconnect".to_string()));
        assert!(!a_flv.contains(&"-local_addr".to_string()));

        let a_srt = build_ffmpeg_audio_args("srt://192.168.1.10:9000", "", Some("192.168.10.50"));
        assert!(!a_srt.contains(&"-reconnect".to_string()));
        assert!(!a_srt.contains(&"-local_addr".to_string()));

        let v_hw = build_ffmpeg_video_args(
            "https://cdn.example.com/live.m3u8",
            "",
            None,
            true,
            Some("1"),
            8,
            320,
            180,
        );
        assert_eq!(v_hw.first().map(String::as_str), Some("-nostdin"));
        assert!(v_hw.contains(&"-hwaccel".to_string()));
        assert!(v_hw.contains(&"-hwaccel_device".to_string()));
        assert!(v_hw.contains(&"1".to_string()));
        assert!(v_hw.contains(&"-extra_hw_frames".to_string()));
        assert!(v_hw.contains(&"8".to_string()));
        assert!(v_hw.contains(&"fps=4,scale=320:180".to_string()));
    }
}
