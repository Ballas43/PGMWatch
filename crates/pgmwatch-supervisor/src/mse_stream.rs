use crate::resolver::ResolvedMedia;
use crate::win32::WindowsJobGuard;
use axum::body::Bytes;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Mutex as AsyncMutex, Notify};

const MAX_ATOM_BYTES: usize = 16 * 1024 * 1024;
const IDLE_SHUTDOWN_SECS: u64 = 4;
const MAX_GOP_PREBUFFER_CHUNKS: usize = 96;

fn find_ffmpeg_bin() -> String {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        for rel in ["bin/ffmpeg.exe", "ffmpeg.exe"] {
            let p = dir.join(rel);
            if p.exists() {
                return p.display().to_string();
            }
        }
    }
    for cand in [
        "vendor/ffmpeg-9.0/bin/ffmpeg.exe",
        "bin/ffmpeg.exe",
        "ffmpeg.exe",
    ] {
        if Path::new(cand).exists() {
            return cand.to_string();
        }
    }
    "ffmpeg".to_string()
}

pub struct MseHubManager {
    hubs: AsyncMutex<HashMap<String, Arc<MseChannelHub>>>,
    rx_bytes_total: Arc<AtomicU64>,
}

impl Default for MseHubManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MseHubManager {
    pub fn new() -> Self {
        Self {
            hubs: AsyncMutex::new(HashMap::new()),
            rx_bytes_total: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn take_rx_bytes(&self) -> u64 {
        self.rx_bytes_total.swap(0, Ordering::Relaxed)
    }

    pub async fn invalidate(&self, channel_id: &str) {
        let removed = {
            let mut map = self.hubs.lock().await;
            map.remove(channel_id)
        };
        if let Some(hub) = removed {
            hub.stop();
        }
    }

    pub async fn invalidate_all(&self) {
        let hubs: Vec<Arc<MseChannelHub>> = {
            let mut map = self.hubs.lock().await;
            map.drain().map(|(_, v)| v).collect()
        };
        for h in hubs {
            h.stop();
        }
    }

    pub async fn subscribe(
        &self,
        channel_id: &str,
        media: &ResolvedMedia,
        job_guard: Arc<WindowsJobGuard>,
    ) -> anyhow::Result<MseSubscription> {
        let hub = {
            let mut map = self.hubs.lock().await;
            if let Some(existing) = map.get(channel_id) {
                if !existing.is_closed()
                    && existing.stream_url == media.stream_url
                    && existing.audio_url == media.audio_url
                {
                    Arc::clone(existing)
                } else {
                    existing.stop();
                    let created =
                        MseChannelHub::spawn(media, job_guard, Arc::clone(&self.rx_bytes_total))?;
                    map.insert(channel_id.to_string(), Arc::clone(&created));
                    created
                }
            } else {
                let created =
                    MseChannelHub::spawn(media, job_guard, Arc::clone(&self.rx_bytes_total))?;
                map.insert(channel_id.to_string(), Arc::clone(&created));
                created
            }
        };

        hub.subscribe_stream().await
    }
}

struct MseBufferState {
    init: Option<(Bytes, String)>,
    current_gop: Vec<Bytes>,
}

pub struct MseChannelHub {
    pub stream_url: String,
    pub audio_url: Option<String>,
    buffer_state: Mutex<MseBufferState>,
    init_notify: Notify,
    tx: broadcast::Sender<Bytes>,
    child: Mutex<Option<Child>>,
    active_subs: AtomicUsize,
    idle_since: Mutex<Option<Instant>>,
    closed: AtomicBool,
    rx_bytes_total: Arc<AtomicU64>,
}

pub struct MseSubscription {
    pub init_segment: Bytes,
    pub prebuffer: VecDeque<Bytes>,
    pub codec: String,
    pub rx: broadcast::Receiver<Bytes>,
    hub: Arc<MseChannelHub>,
}

impl Drop for MseSubscription {
    fn drop(&mut self) {
        self.hub.release_subscriber();
    }
}

fn append_fmp4_input_args(args: &mut Vec<String>, url: &str, header_str: &str) {
    args.extend(["-analyzeduration", "1000000", "-probesize", "1500000"].map(String::from));
    if url.contains(".m3u8") || url.contains(".mpd") {
        args.push("-http_persistent".into());
        args.push("0".into());
        if url.contains(".m3u8") {
            // Start 2 segments from live edge: avoids FFmpeg's -3 lag while keeping 1 segment of
            // lookahead so split video+audio HLS inputs stay synchronized without starving on reload.
            args.extend(["-live_start_index", "-2"].map(String::from));
        }
    } else {
        args.extend(["-reconnect", "1", "-reconnect_streamed", "1"].map(String::from));
    }
    if !header_str.is_empty() {
        args.push("-headers".into());
        args.push(header_str.to_string());
    }
    args.extend(
        [
            "-readrate",
            "1.0",
            "-readrate_initial_burst",
            "2.5",
            "-readrate_catchup",
            "1.25",
            "-i",
            url,
        ]
        .map(String::from),
    );
}

/// TikTok SD/LD/HD/UHD FLV streams (`_ld5`, `_sd5`, `_hd5`, `_uhd5`, `h265`, `hevc`) use HEVC,
/// which Firefox and many Chromium builds cannot decode via MSE `-c:v copy`. Transcode those to
/// ultrafast H.264 on demand (only when the operator explicitly clicks HQ on that card).
pub(crate) fn is_hevc_stream_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or("");
    path.contains("_ld5.")
        || path.contains("_sd5.")
        || path.contains("_hd5.")
        || path.contains("_uhd5.")
        || path.contains("h265")
        || path.contains("hevc")
}

fn build_ffmpeg_fmp4_args(media: &ResolvedMedia) -> Vec<String> {
    let mut header_str = String::new();
    for (k, v) in &media.headers {
        let ck: String = k.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
        let cv: String = v.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
        if !ck.is_empty() && !cv.is_empty() {
            header_str.push_str(&format!("{ck}: {cv}\r\n"));
        }
    }

    let mut args = vec!["-nostdin", "-hide_banner", "-loglevel", "error"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();

    append_fmp4_input_args(&mut args, &media.stream_url, &header_str);
    if let Some(ref audio_url) = media.audio_url {
        append_fmp4_input_args(&mut args, audio_url, &header_str);
    }

    args.extend(["-map", "0:v:0"].map(String::from));
    if media.audio_url.is_some() {
        args.extend(["-map", "1:a:0?"].map(String::from));
    } else {
        args.extend(["-map", "0:a:0?"].map(String::from));
    }

    if is_hevc_stream_url(&media.stream_url) {
        args.extend(
            [
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-tune",
                "zerolatency",
                "-pix_fmt",
                "yuv420p",
                "-g",
                "60",
                "-b:v",
                "1500k",
                "-maxrate",
                "2000k",
                "-bufsize",
                "3000k",
            ]
            .map(String::from),
        );
    } else {
        args.extend(["-c:v", "copy"].map(String::from));
    }

    args.extend(
        [
            "-c:a",
            "aac",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-b:a",
            "128k",
            "-max_muxing_queue_size",
            "1024",
            "-avoid_negative_ts",
            "make_zero",
            "-f",
            "mp4",
            "-movflags",
            "empty_moov+default_base_moof+negative_cts_offsets",
            "-frag_duration",
            "400000",
            "-flush_packets",
            "1",
            "pipe:1",
        ]
        .map(String::from),
    );
    args
}

impl MseChannelHub {
    fn spawn(
        media: &ResolvedMedia,
        job_guard: Arc<WindowsJobGuard>,
        rx_bytes_total: Arc<AtomicU64>,
    ) -> anyhow::Result<Arc<Self>> {
        let ffmpeg = find_ffmpeg_bin();
        let args = build_ffmpeg_fmp4_args(media);

        use pgmwatch_core::process::BackgroundCommandExt;
        let mut cmd = Command::new(&ffmpeg);
        cmd.no_window()
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn()?;
        let _ = job_guard.assign_pid(child.id());
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Failed to open ffmpeg fMP4 stdout"))?;

        let (tx, _) = broadcast::channel(256);
        let hub = Arc::new(Self {
            stream_url: media.stream_url.clone(),
            audio_url: media.audio_url.clone(),
            buffer_state: Mutex::new(MseBufferState {
                init: None,
                current_gop: Vec::new(),
            }),
            init_notify: Notify::new(),
            tx,
            child: Mutex::new(Some(child)),
            active_subs: AtomicUsize::new(0),
            idle_since: Mutex::new(Some(Instant::now())),
            closed: AtomicBool::new(false),
            rx_bytes_total,
        });

        let reader_hub = Arc::clone(&hub);
        std::thread::spawn(move || {
            pump_fmp4_boxes(stdout, &reader_hub);
            reader_hub.stop();
        });

        let watchdog_hub = Arc::clone(&hub);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            loop {
                ticker.tick().await;
                if watchdog_hub.is_closed() {
                    break;
                }
                if watchdog_hub.active_subs.load(Ordering::Acquire) == 0 {
                    let expired = watchdog_hub
                        .idle_since
                        .lock()
                        .ok()
                        .and_then(|g| *g)
                        .map(|t| t.elapsed() >= Duration::from_secs(IDLE_SHUTDOWN_SECS))
                        .unwrap_or(false);
                    if expired {
                        watchdog_hub.stop();
                        break;
                    }
                }
            }
        });

        Ok(hub)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn release_subscriber(&self) {
        let prev = self.active_subs.fetch_sub(1, Ordering::AcqRel);
        if prev <= 1
            && let Ok(mut guard) = self.idle_since.lock()
        {
            *guard = Some(Instant::now());
        }
    }

    pub fn stop(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            if let Ok(mut guard) = self.child.lock()
                && let Some(mut c) = guard.take()
            {
                let _ = c.kill();
                let _ = c.wait();
            }
            if let Ok(mut state) = self.buffer_state.lock() {
                state.init = None;
                state.current_gop.clear();
            }
            self.init_notify.notify_waiters();
        }
    }

    async fn subscribe_stream(self: &Arc<Self>) -> anyhow::Result<MseSubscription> {
        self.active_subs.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut guard) = self.idle_since.lock() {
            *guard = None;
        }
        let guard = ActiveSubDecrementOnErr { hub: Arc::clone(self), committed: false };

        let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
        loop {
            // Register waiter before checking init/closed to prevent lost-wakeup races on notify_waiters().
            let notified = self.init_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            if let Ok(state) = self.buffer_state.lock()
                && let Some((ref init_segment, ref codec)) = state.init
            {
                let sub = MseSubscription {
                    init_segment: init_segment.clone(),
                    prebuffer: state.current_gop.iter().cloned().collect(),
                    codec: codec.clone(),
                    rx: self.tx.subscribe(),
                    hub: Arc::clone(self),
                };
                guard.commit();
                return Ok(sub);
            }
            if self.is_closed() {
                anyhow::bail!("fMP4 stream closed before init segment");
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                anyhow::bail!("Timed out waiting for fMP4 init segment");
            }
            let _ = tokio::time::timeout(deadline - now, notified).await;
        }
    }
}

struct ActiveSubDecrementOnErr {
    hub: Arc<MseChannelHub>,
    committed: bool,
}

impl ActiveSubDecrementOnErr {
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for ActiveSubDecrementOnErr {
    fn drop(&mut self) {
        if !self.committed {
            self.hub.release_subscriber();
        }
    }
}

fn pump_fmp4_boxes(mut stdout: std::process::ChildStdout, hub: &MseChannelHub) {
    let mut raw_buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    let mut read_tmp = [0u8; 64 * 1024];
    let mut ftyp_box: Option<Vec<u8>> = None;
    let mut pending_moof: Option<(Vec<u8>, bool)> = None;
    let mut video_track_id: Option<u32> = None;

    while !hub.is_closed() {
        let n = match stdout.read(&mut read_tmp) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        hub.rx_bytes_total.fetch_add(n as u64, Ordering::Relaxed);
        raw_buf.extend_from_slice(&read_tmp[..n]);

        let mut consumed = 0usize;
        while raw_buf.len() - consumed >= 8 {
            let slice = &raw_buf[consumed..];
            let sz = u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]) as usize;
            if !(8..=MAX_ATOM_BYTES).contains(&sz) {
                return;
            }
            if slice.len() < sz {
                break;
            }
            let atom = &slice[..sz];
            let typ = &atom[4..8];
            match typ {
                b"ftyp" => {
                    ftyp_box = Some(atom.to_vec());
                }
                b"moov" => {
                    let mut init = ftyp_box.take().unwrap_or_default();
                    init.extend_from_slice(atom);
                    let Some(codec) = extract_mse_codec(atom) else {
                        return;
                    };
                    video_track_id = extract_video_track_id(atom);
                    if let Ok(mut state) = hub.buffer_state.lock() {
                        state.init = Some((Bytes::from(init), codec));
                        state.current_gop.clear();
                    }
                    hub.init_notify.notify_waiters();
                }
                b"moof" => {
                    let is_key = is_keyframe_moof_for_track(atom, video_track_id);
                    pending_moof = Some((atom.to_vec(), is_key));
                }
                b"mdat" => {
                    if let Some((mut frag, is_key)) = pending_moof.take() {
                        frag.extend_from_slice(atom);
                        let bytes = Bytes::from(frag);
                        if let Ok(mut state) = hub.buffer_state.lock() {
                            if is_key {
                                state.current_gop.clear();
                                state.current_gop.push(bytes.clone());
                            } else if !state.current_gop.is_empty() {
                                if state.current_gop.len() < MAX_GOP_PREBUFFER_CHUNKS {
                                    state.current_gop.push(bytes.clone());
                                } else {
                                    // Never leave a truncated mid-GOP prebuffer that would create a timeline hole.
                                    state.current_gop.clear();
                                }
                            }
                            let _ = hub.tx.send(bytes);
                        }
                    }
                }
                _ => {}
            }
            consumed += sz;
        }
        if consumed > 0 {
            raw_buf.drain(..consumed);
        }
        if raw_buf.len() > MAX_ATOM_BYTES * 2 {
            return;
        }
    }
}

#[cfg(test)]
pub fn is_keyframe_moof(moof: &[u8]) -> bool {
    is_keyframe_moof_for_track(moof, None)
}

pub fn is_keyframe_moof_for_track(moof: &[u8], video_track_id: Option<u32>) -> bool {
    if moof.len() < 8 || &moof[4..8] != b"moof" {
        return false;
    }
    let mut pos = 8usize;
    while pos + 8 <= moof.len() {
        let sz = u32::from_be_bytes([moof[pos], moof[pos + 1], moof[pos + 2], moof[pos + 3]]) as usize;
        if sz < 8 || pos + sz > moof.len() {
            break;
        }
        if &moof[pos + 4..pos + 8] == b"traf" {
            let traf = &moof[pos + 8..pos + sz];
            let mut tfhd_track_id: Option<u32> = None;
            let mut tfhd_default_flags: Option<u32> = None;
            let mut trun_is_sync: Option<bool> = None;
            let mut sub_pos = 0usize;
            while sub_pos + 8 <= traf.len() {
                let s_sz = u32::from_be_bytes([
                    traf[sub_pos],
                    traf[sub_pos + 1],
                    traf[sub_pos + 2],
                    traf[sub_pos + 3],
                ]) as usize;
                if s_sz < 8 || sub_pos + s_sz > traf.len() {
                    break;
                }
                let s_typ = &traf[sub_pos + 4..sub_pos + 8];
                let s_body = &traf[sub_pos + 8..sub_pos + s_sz];
                if s_typ == b"tfhd" && s_body.len() >= 8 {
                    let tf_flags = u32::from_be_bytes([0, s_body[1], s_body[2], s_body[3]]);
                    tfhd_track_id = Some(u32::from_be_bytes([
                        s_body[4],
                        s_body[5],
                        s_body[6],
                        s_body[7],
                    ]));
                    let mut idx = 8usize;
                    if (tf_flags & 0x01) != 0 { idx += 8; }
                    if (tf_flags & 0x02) != 0 { idx += 4; }
                    if (tf_flags & 0x08) != 0 { idx += 4; }
                    if (tf_flags & 0x10) != 0 { idx += 4; }
                    if (tf_flags & 0x20) != 0 && idx + 4 <= s_body.len() {
                        tfhd_default_flags = Some(u32::from_be_bytes([
                            s_body[idx],
                            s_body[idx + 1],
                            s_body[idx + 2],
                            s_body[idx + 3],
                        ]));
                    }
                } else if s_typ == b"trun" && s_body.len() >= 8 {
                    let tr_flags = u32::from_be_bytes([0, s_body[1], s_body[2], s_body[3]]);
                    let sample_count =
                        u32::from_be_bytes([s_body[4], s_body[5], s_body[6], s_body[7]]);
                    let mut idx = 8usize;
                    if (tr_flags & 0x01) != 0 { idx += 4; }
                    if (tr_flags & 0x04) != 0 {
                        if idx + 4 <= s_body.len() {
                            let first_flags = u32::from_be_bytes([
                                s_body[idx],
                                s_body[idx + 1],
                                s_body[idx + 2],
                                s_body[idx + 3],
                            ]);
                            trun_is_sync = Some((first_flags & 0x0001_0000) == 0);
                        }
                    } else if sample_count > 0 && (tr_flags & 0x0400) != 0 {
                        if (tr_flags & 0x0100) != 0 { idx += 4; }
                        if (tr_flags & 0x0200) != 0 { idx += 4; }
                        if idx + 4 <= s_body.len() {
                            let s0_flags = u32::from_be_bytes([
                                s_body[idx],
                                s_body[idx + 1],
                                s_body[idx + 2],
                                s_body[idx + 3],
                            ]);
                            trun_is_sync = Some((s0_flags & 0x0001_0000) == 0);
                        }
                    }
                }
                sub_pos += s_sz;
            }
            if video_track_id.is_some() && tfhd_track_id != video_track_id {
                pos += sz;
                continue;
            }
            if let Some(sync) = trun_is_sync {
                return sync;
            }
            if let Some(def) = tfhd_default_flags {
                return (def & 0x0001_0000) == 0;
            }
            return false;
        }
        pos += sz;
    }
    false
}

pub fn extract_video_track_id(moov: &[u8]) -> Option<u32> {
    if moov.len() < 8 || &moov[4..8] != b"moov" {
        return None;
    }
    let mut pos = 8usize;
    while pos + 8 <= moov.len() {
        let sz = u32::from_be_bytes([moov[pos], moov[pos + 1], moov[pos + 2], moov[pos + 3]]) as usize;
        if sz < 8 || pos + sz > moov.len() {
            break;
        }
        if &moov[pos + 4..pos + 8] == b"trak" {
            let trak = &moov[pos + 8..pos + sz];
            let is_video = find_bmff_box(trak, b"hdlr")
                .is_some_and(|hdlr| hdlr.len() >= 20 && &hdlr[16..20] == b"vide");
            if is_video
                && let Some(tkhd) = find_bmff_box(trak, b"tkhd")
                && tkhd.len() >= 24
            {
                let ver = tkhd[8];
                let tid_off = if ver == 1 { 28 } else { 20 };
                if tid_off + 4 <= tkhd.len() {
                    return Some(u32::from_be_bytes([
                        tkhd[tid_off],
                        tkhd[tid_off + 1],
                        tkhd[tid_off + 2],
                        tkhd[tid_off + 3],
                    ]));
                }
            }
        }
        pos += sz;
    }
    None
}

pub fn extract_mse_codec(moov: &[u8]) -> Option<String> {
    let mut video_codec: Option<String> = None;
    if let Some(avcc) = find_bmff_box(moov, b"avcC") {
        if avcc.len() >= 12 {
            video_codec = Some(format!(
                "avc1.{:02x}{:02x}{:02x}",
                avcc[9], avcc[10], avcc[11]
            ));
        }
    } else if let Some(hvcc) = find_bmff_box(moov, b"hvcC")
        && hvcc.len() >= 21
    {
        let profile_idc = hvcc[9] & 0x1f;
        let tier = if (hvcc[9] & 0x20) != 0 { 'H' } else { 'L' };
        let level_idc = hvcc[20];
        video_codec = Some(format!("hvc1.{profile_idc}.4.{tier}{level_idc}.B0"));
    } else if let Some(vpcc) = find_bmff_box(moov, b"vpcC") {
        if vpcc.len() >= 15 {
            let profile = vpcc[12];
            let level = vpcc[13];
            let bit_depth = (vpcc[14] >> 4) & 0x0f;
            video_codec = Some(format!("vp09.{profile:02}.{level:02}.{bit_depth:02}"));
        }
    } else if let Some(av1c) = find_bmff_box(moov, b"av1C")
        && av1c.len() >= 11
    {
        let seq_profile = (av1c[9] >> 5) & 0x07;
        let seq_level = av1c[9] & 0x1f;
        let tier = if (av1c[10] & 0x80) != 0 { 'H' } else { 'M' };
        let high_bit = (av1c[10] & 0x40) != 0;
        let twelve = (av1c[10] & 0x20) != 0;
        let bits = if !high_bit { 8 } else if twelve { 12 } else { 10 };
        video_codec = Some(format!("av01.{seq_profile}.{seq_level:02}{tier}.{bits:02}"));
    }
    video_codec.map(|vc| {
        if find_bmff_box(moov, b"mp4a").is_some() {
            format!("{vc}, mp4a.40.2")
        } else {
            vc
        }
    })
}

fn find_bmff_box<'a>(data: &'a [u8], target: &[u8; 4]) -> Option<&'a [u8]> {
    let mut pos = 0usize;
    while pos + 8 <= data.len() {
        let sz = u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        if sz < 8 || pos + sz > data.len() {
            break;
        }
        let atom = &data[pos..pos + sz];
        let typ = &atom[4..8];
        if typ == target {
            return Some(atom);
        }
        // Recurse into container boxes inside moov -> trak -> mdia -> minf -> stbl -> stsd -> SampleEntry
        let child_start = match typ {
            b"stsd" => Some(16),
            b"avc1" | b"avc3" | b"hvc1" | b"hev1" | b"vp09" | b"vp08" | b"av01" => Some(86),
            b"moov" | b"trak" | b"mdia" | b"minf" | b"stbl" => Some(8),
            _ => None,
        };
        if let Some(offset) = child_start
            && atom.len() > offset
            && let Some(found) = find_bmff_box(&atom[offset..], target)
        {
            return Some(found);
        }
        pos += sz;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_mse_codec_avcc_and_vpcc() {
        // Minimal stsd -> avc1 (86-byte VisualSampleEntry header) -> avcC container inside moov
        let avcc = [
            0, 0, 0, 14, b'a', b'v', b'c', b'C', // box hdr
            1, 0x64, 0x00, 0x2a, 0xff, 0xe1,     // profile=High(64), compat=00, level=4.2(2a)
        ];
        let mut avc1 = vec![0u8; 86];
        let avc1_len = (86 + avcc.len()) as u32;
        avc1[0..4].copy_from_slice(&avc1_len.to_be_bytes());
        avc1[4..8].copy_from_slice(b"avc1");
        avc1.extend_from_slice(&avcc);
        let mut moov = vec![0u8; 8];
        let moov_len = (8 + avc1.len()) as u32;
        moov[0..4].copy_from_slice(&moov_len.to_be_bytes());
        moov[4..8].copy_from_slice(b"moov");
        moov.extend_from_slice(&avc1);
        assert_eq!(extract_mse_codec(&moov).as_deref(), Some("avc1.64002a"));

        // Minimal moov -> vp09 (86-byte VisualSampleEntry header) -> vpcC
        let vpcc = [
            0, 0, 0, 16, b'v', b'p', b'c', b'C',
            1, 0, 0, 0, // version/flags
            0, 41, 0x80, 0, // profile=0, level=41, bit_depth=8
        ];
        let mut vp09 = vec![0u8; 86];
        let vp09_len = (86 + vpcc.len()) as u32;
        vp09[0..4].copy_from_slice(&vp09_len.to_be_bytes());
        vp09[4..8].copy_from_slice(b"vp09");
        vp09.extend_from_slice(&vpcc);
        let mut moov2 = vec![0u8; 8];
        let moov2_len = (8 + vp09.len()) as u32;
        moov2[0..4].copy_from_slice(&moov2_len.to_be_bytes());
        moov2[4..8].copy_from_slice(b"moov");
        moov2.extend_from_slice(&vp09);
        assert_eq!(extract_mse_codec(&moov2).as_deref(), Some("vp09.00.41.08"));

        // Muxed moov with both avc1 and mp4a exposes "avc1..., mp4a.40.2"
        let mut mp4a = vec![0u8; 36];
        mp4a[0..4].copy_from_slice(&36u32.to_be_bytes());
        mp4a[4..8].copy_from_slice(b"mp4a");
        let mut mux_moov = vec![0u8; 8];
        let mux_len = (8 + avc1.len() + mp4a.len()) as u32;
        mux_moov[0..4].copy_from_slice(&mux_len.to_be_bytes());
        mux_moov[4..8].copy_from_slice(b"moov");
        mux_moov.extend_from_slice(&avc1);
        mux_moov.extend_from_slice(&mp4a);
        assert_eq!(
            extract_mse_codec(&mux_moov).as_deref(),
            Some("avc1.64002a, mp4a.40.2")
        );
    }

    #[test]
    fn test_is_keyframe_moof_detection() {
        let make_traf = |track_id: u32, first_sample_flags: u32| -> Vec<u8> {
            let mut tfhd = vec![0u8; 16];
            tfhd[0..4].copy_from_slice(&16u32.to_be_bytes());
            tfhd[4..8].copy_from_slice(b"tfhd");
            tfhd[12..16].copy_from_slice(&track_id.to_be_bytes());

            let mut trun = vec![0u8; 24];
            trun[0..4].copy_from_slice(&24u32.to_be_bytes());
            trun[4..8].copy_from_slice(b"trun");
            // version=0, flags=0x000005 (data_offset + first_sample_flags)
            trun[8..12].copy_from_slice(&0x0000_0005u32.to_be_bytes());
            trun[12..16].copy_from_slice(&1u32.to_be_bytes());
            trun[16..20].copy_from_slice(&100i32.to_be_bytes());
            trun[20..24].copy_from_slice(&first_sample_flags.to_be_bytes());

            let mut traf = vec![0u8; 8];
            let traf_len = (8 + tfhd.len() + trun.len()) as u32;
            traf[0..4].copy_from_slice(&traf_len.to_be_bytes());
            traf[4..8].copy_from_slice(b"traf");
            traf.extend_from_slice(&tfhd);
            traf.extend_from_slice(&trun);
            traf
        };
        let make_moof = |trafs: &[Vec<u8>]| -> Vec<u8> {
            let body_len: usize = trafs.iter().map(|t| t.len()).sum();
            let mut moof = vec![0u8; 8];
            moof[0..4].copy_from_slice(&((8 + body_len) as u32).to_be_bytes());
            moof[4..8].copy_from_slice(b"moof");
            for t in trafs {
                moof.extend_from_slice(t);
            }
            moof
        };

        assert!(is_keyframe_moof(&make_moof(&[make_traf(1, 0x0200_0000)])));
        assert!(!is_keyframe_moof(&make_moof(&[make_traf(1, 0x0101_0000)])));
        // Audio traf (track 2, sync) must not spoof video keyframe when video traf (track 1) is P/B
        let mixed_pframe = make_moof(&[make_traf(2, 0x0200_0000), make_traf(1, 0x0101_0000)]);
        assert!(!is_keyframe_moof_for_track(&mixed_pframe, Some(1)));
        let mixed_idr = make_moof(&[make_traf(2, 0x0200_0000), make_traf(1, 0x0200_0000)]);
        assert!(is_keyframe_moof_for_track(&mixed_idr, Some(1)));
    }

    #[test]
    fn test_mse_ffmpeg_args_support_bframes_and_negative_cts() {
        let media = ResolvedMedia {
            stream_url: "http://127.0.0.1:8787/api/internal/dash-hls/abc.m3u8".into(),
            audio_url: None,
            headers: vec![("User-Agent".into(), "Mozilla/5.0".into())],
            via: "yt-dlp".into(),
        };
        let args = build_ffmpeg_fmp4_args(&media);
        assert!(!args.contains(&"low_delay".to_string()));
        assert!(!args.contains(&"+nobuffer".to_string()));
        assert!(!args.contains(&"-an".to_string()));
        assert!(args.contains(&"aac".to_string()));
        assert!(args.contains(&"0:a:0?".to_string()));
        assert!(args.contains(&"make_zero".to_string()));
        assert!(args.iter().any(|a| a.contains("negative_cts_offsets")));

        let split_media = ResolvedMedia {
            stream_url: "https://example.com/video.mp4".into(),
            audio_url: Some("https://example.com/audio.mp4".into()),
            headers: vec![],
            via: "yt-dlp".into(),
        };
        let split_args = build_ffmpeg_fmp4_args(&split_media);
        assert!(split_args.contains(&"1:a:0?".to_string()));
    }
}
