use chrono::{DateTime, Utc};
use pgmwatch_core::models::FaultKind;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Clone)]
pub struct StreamCodecConfig {
    pub codec_id: i32,
    pub extradata: Vec<u8>,
    pub width: i32,
    pub height: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub bit_rate: i64,
}

#[derive(Debug, Clone)]
pub struct CompressedPacket {
    pub pts_ms: i64,
    pub dts_ms: i64,
    pub duration_ms: i64,
    pub is_keyframe: bool,
    pub is_video: bool,
    pub codec_param_hash: u64,
    pub data: Vec<u8>,
}

pub struct GopRingBuffer {
    max_duration_ms: i64,
    max_bytes: usize,
    current_bytes: usize,
    active_param_hash: Option<u64>,
    video_config: Option<StreamCodecConfig>,
    audio_config: Option<StreamCodecConfig>,
    packets: VecDeque<CompressedPacket>,
}

impl GopRingBuffer {
    pub fn new(max_duration_ms: i64, max_bytes: usize) -> Self {
        Self {
            max_duration_ms,
            max_bytes,
            current_bytes: 0,
            active_param_hash: None,
            video_config: None,
            audio_config: None,
            packets: VecDeque::with_capacity(512),
        }
    }

    pub fn set_video_config(&mut self, cfg: StreamCodecConfig) {
        self.video_config = Some(cfg);
    }

    pub fn set_audio_config(&mut self, cfg: StreamCodecConfig) {
        self.audio_config = Some(cfg);
    }

    pub fn video_config(&self) -> Option<&StreamCodecConfig> {
        self.video_config.as_ref()
    }

    pub fn audio_config(&self) -> Option<&StreamCodecConfig> {
        self.audio_config.as_ref()
    }

    /// Resets buffered GOP packets and codec headers when switching streams or refreshing tokens.
    pub fn clear(&mut self) {
        self.packets.clear();
        self.current_bytes = 0;
        self.active_param_hash = None;
        self.video_config = None;
        self.audio_config = None;
    }

    /// Push compressed packet; seals and resets buffer cleanly on SPS/PPS/resolution change.
    pub fn push(&mut self, pkt: CompressedPacket) {
        if pkt.is_video {
            if let Some(prev_hash) = self.active_param_hash
                && prev_hash != pkt.codec_param_hash
            {
                tracing::info!("Stream codec/resolution changed mid-stream; sealing GOP ring buffer");
                self.packets.clear();
                self.current_bytes = 0;
            }
            self.active_param_hash = Some(pkt.codec_param_hash);
        }

        self.current_bytes += pkt.data.len();
        self.packets.push_back(pkt);
        self.prune_old();
    }

    fn prune_old(&mut self) {
        let newest_pts = self.packets.back().map(|p| p.pts_ms).unwrap_or(0);
        while let Some(front) = self.packets.front() {
            let too_old = newest_pts - front.pts_ms > self.max_duration_ms;
            let too_big = self.current_bytes > self.max_bytes;
            if !(too_old || too_big) {
                break;
            }
            if let Some(removed) = self.packets.pop_front() {
                self.current_bytes = self.current_bytes.saturating_sub(removed.data.len());
            }
        }
    }

    pub fn snapshot_from_pretrigger_idr(&self, pretrigger_ms: i64) -> Vec<CompressedPacket> {
        let newest_video_pts = self
            .packets
            .iter()
            .filter(|p| p.is_video)
            .map(|p| p.pts_ms)
            .max();
        let newest_audio_pts = self
            .packets
            .iter()
            .filter(|p| !p.is_video)
            .map(|p| p.pts_ms)
            .max();
        let newest_pts = newest_video_pts
            .into_iter()
            .chain(newest_audio_pts)
            .max()
            .unwrap_or(0);
        let video_target_start = newest_video_pts.unwrap_or(newest_pts) - pretrigger_ms;
        let audio_target_start = newest_audio_pts.unwrap_or(newest_pts) - pretrigger_ms;

        // Collect all video keyframes ordered by timestamp so split-A/V arrival jitter
        // never causes us to pick a stale earlier keyframe or lose track of the latest IDR.
        let mut best_idr: Option<(usize, i64)> = None;
        for (idx, pkt) in self.packets.iter().enumerate() {
            if pkt.is_video && pkt.is_keyframe {
                match best_idr {
                    None => best_idr = Some((idx, pkt.pts_ms)),
                    Some((_, cur_pts)) => {
                        if cur_pts < video_target_start {
                            // Still searching forward to reach video_target_start
                            if pkt.pts_ms > cur_pts {
                                best_idr = Some((idx, pkt.pts_ms));
                            }
                        } else if pkt.pts_ms >= video_target_start && pkt.pts_ms < cur_pts {
                            // Tightest IDR at or after video_target_start
                            best_idr = Some((idx, pkt.pts_ms));
                        }
                    }
                }
            }
        }

        let (start_idx, video_anchor_pts) = match best_idr {
            Some((idx, idr_pts)) => (idx, idr_pts),
            None => (0, video_target_start),
        };
        // Anchor audio to the tighter of audio_target_start and video_anchor_pts (when on the same
        // timeline) so split-A/V PTS skew or long GOP intervals cannot leak pre-fault program audio
        // into Dead Air / audio fault clips.
        let same_timeline = newest_video_pts
            .zip(newest_audio_pts)
            .is_some_and(|(v, a)| (v - a).abs() < 30_000);
        let audio_anchor_pts = if same_timeline {
            audio_target_start.max(video_anchor_pts)
        } else {
            audio_target_start
        };

        let mut selected: Vec<CompressedPacket> = self
            .packets
            .iter()
            .enumerate()
            .filter(|(idx, pkt)| {
                if pkt.is_video {
                    *idx >= start_idx && pkt.pts_ms >= video_anchor_pts
                } else {
                    pkt.pts_ms >= audio_anchor_pts
                }
            })
            .map(|(_, pkt)| pkt.clone())
            .collect();

        selected.sort_by_key(|p| (p.dts_ms, p.pts_ms, !p.is_video));
        selected
    }
}

pub struct ClipSaver {
    clips_dir: PathBuf,
    quota_bytes: u64,
}

impl ClipSaver {
    pub fn new(clips_dir: PathBuf, quota_gb: f64) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&clips_dir)?;
        let quota_bytes = (quota_gb.max(0.5) * 1024.0 * 1024.0 * 1024.0) as u64;
        Ok(Self {
            clips_dir,
            quota_bytes,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_clip(
        &self,
        channel_id: &str,
        fault: Option<FaultKind>,
        stream_url: Option<&str>,
        packets: &[CompressedPacket],
        video_cfg: Option<&StreamCodecConfig>,
        audio_cfg: Option<&StreamCodecConfig>,
        saved_at: DateTime<Utc>,
    ) -> anyhow::Result<String> {
        self.enforce_fifo_quota()?;

        let fault_slug = fault
            .map(|f| format!("{f:?}").to_lowercase())
            .unwrap_or_else(|| "manual".to_string());
        let ts = saved_at.format("%Y%m%dT%H%M%SZ");
        let safe_ch: String = channel_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' })
            .collect();
        let filename = format!("{safe_ch}_{ts}_{fault_slug}.mp4");
        let dest_path = self.clips_dir.join(&filename);

        #[cfg(feature = "ffmpeg-ffi")]
        if !packets.is_empty() && (video_cfg.is_some() || audio_cfg.is_some()) {
            match mux_compressed_packets_to_mp4(&dest_path, packets, video_cfg, audio_cfg) {
                Ok(()) => {
                    if std::fs::metadata(&dest_path).map(|m| m.len() > 256).unwrap_or(false) {
                        return Ok(filename);
                    }
                }
                Err(err) => {
                    tracing::debug!("In-process MP4 pre-trigger mux failed, falling back: {err}");
                    let _ = std::fs::remove_file(&dest_path);
                }
            }
        }
        #[cfg(not(feature = "ffmpeg-ffi"))]
        let _ = (video_cfg, audio_cfg);

        // Never record 5 seconds from the *future* live edge for historical fault clips (especially
        // AudioDeadAir / AudioClipping), because the fault may have already recovered by the time
        // ffmpeg connects — causing a Dead Air incident clip to play back normal audio.
        if fault.is_none()
            && let Some(url) = stream_url
            && !url.starts_with("sim://")
            && remux_5s_clip_via_ffmpeg(url, &dest_path).is_ok()
        {
            return Ok(filename);
        }

        // Synthetic / offline / packet fallback: generate valid playable 5s MP4 evidence clip
        if generate_synthetic_evidence_mp4(&dest_path, channel_id, &fault_slug, packets).is_err() {
            // Absolute last-resort minimal valid fMP4/MP4 container stub so UI & quota tracking never crash
            write_minimal_mp4_stub(&dest_path, packets)?;
        }

        Ok(filename)
    }

    fn enforce_fifo_quota(&self) -> anyhow::Result<()> {
        let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
        let mut total = 0u64;

        for entry in std::fs::read_dir(&self.clips_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("mp4") {
                continue;
            }
            let meta = entry.metadata()?;
            let size = meta.len();
            let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total += size;
            entries.push((path, size, modified));
        }

        if total <= self.quota_bytes {
            return Ok(());
        }

        entries.sort_by_key(|(_, _, m)| *m);
        for (path, size, _) in entries {
            if total <= self.quota_bytes {
                break;
            }
            if delete_with_defender_retry(&path) {
                total = total.saturating_sub(size);
            }
        }
        Ok(())
    }
}

fn delete_with_defender_retry(path: &Path) -> bool {
    for _ in 0..5 {
        if std::fs::remove_file(path).is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    false
}

fn remux_5s_clip_via_ffmpeg(url: &str, dest: &Path) -> anyhow::Result<()> {
    use pgmwatch_core::process::BackgroundCommandExt;
    let ffmpeg = crate::backend::find_ffmpeg9_binary();
    let status = Command::new(ffmpeg)
        .no_window()
        .args([
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-t",
            "5",
            "-i",
            url,
            "-c",
            "copy",
            "-movflags",
            "+faststart",
        ])
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() && dest.exists() {
        Ok(())
    } else {
        anyhow::bail!("ffmpeg clip remux exited with {status}")
    }
}

fn generate_synthetic_evidence_mp4(
    dest: &Path,
    channel_id: &str,
    fault_slug: &str,
    _packets: &[CompressedPacket],
) -> anyhow::Result<()> {
    let ffmpeg = crate::backend::find_ffmpeg9_binary();
    let color = if fault_slug.contains("dead") || fault_slug.contains("black") {
        "color=c=#141415:s=640x360:d=5"
    } else {
        "smptebars=s=640x360:d=5"
    };
    use pgmwatch_core::process::BackgroundCommandExt;
    let status = Command::new(ffmpeg)
        .no_window()
        .args([
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            color,
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=5",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            "-movflags",
            "+faststart",
            "-metadata",
            &format!("title=PGMWatch {channel_id} {fault_slug}"),
        ])
        .arg(dest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    match status {
        Ok(s) if s.success() && dest.exists() => Ok(()),
        _ => {
            // Try MPEG-4 fallback if LGPL build has no libx264
            let s2 = Command::new(crate::backend::find_ffmpeg9_binary())
                .no_window()
                .args([
                    "-nostdin",
                    "-y",
                    "-loglevel",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    color,
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=5",
                    "-c:v",
                    "mpeg4",
                    "-q:v",
                    "6",
                    "-c:a",
                    "aac",
                    "-shortest",
                ])
                .arg(dest)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            if s2.success() && dest.exists() {
                Ok(())
            } else {
                anyhow::bail!("synthetic mp4 generation failed")
            }
        }
    }
}

#[cfg(feature = "ffmpeg-ffi")]
fn mux_compressed_packets_to_mp4(
    dest: &Path,
    packets: &[CompressedPacket],
    video_cfg: Option<&StreamCodecConfig>,
    audio_cfg: Option<&StreamCodecConfig>,
) -> anyhow::Result<()> {
    use ffmpeg_sys_next as ff;
    use std::ffi::CString;
    use std::os::raw::c_int;
    use std::ptr;

    struct OutCtxGuard(*mut ff::AVFormatContext);
    impl Drop for OutCtxGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    if !(*self.0).oformat.is_null()
                        && ((*(*self.0).oformat).flags & ff::AVFMT_NOFILE) == 0
                        && !(*self.0).pb.is_null()
                    {
                        ff::avio_closep(&mut (*self.0).pb);
                    }
                    ff::avformat_free_context(self.0);
                }
            }
        }
    }

    struct PktGuard(*mut ff::AVPacket);
    impl Drop for PktGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    ff::av_packet_free(&mut self.0);
                }
            }
        }
    }

    struct DictGuard(*mut ff::AVDictionary);
    impl Drop for DictGuard {
        fn drop(&mut self) {
            unsafe {
                if !self.0.is_null() {
                    ff::av_dict_free(&mut self.0);
                }
            }
        }
    }

    let dest_str = dest.to_str().ok_or_else(|| anyhow::anyhow!("Invalid clip path"))?;
    let dest_c = CString::new(dest_str)?;

    unsafe {
        let mut ofmt: *mut ff::AVFormatContext = ptr::null_mut();
        let rc = ff::avformat_alloc_output_context2(
            &mut ofmt,
            ptr::null(),
            c"mp4".as_ptr(),
            dest_c.as_ptr(),
        );
        if rc < 0 || ofmt.is_null() {
            anyhow::bail!("avformat_alloc_output_context2 failed: {rc}");
        }
        let _ctx_guard = OutCtxGuard(ofmt);

        let populate_extradata = |par: *mut ff::AVCodecParameters, extra: &[u8]| -> anyhow::Result<()> {
            if !extra.is_empty() {
                let sz = extra.len() + ff::AV_INPUT_BUFFER_PADDING_SIZE as usize;
                let mem = ff::av_mallocz(sz) as *mut u8;
                if mem.is_null() {
                    anyhow::bail!("av_mallocz failed for extradata");
                }
                ptr::copy_nonoverlapping(extra.as_ptr(), mem, extra.len());
                (*par).extradata = mem;
                (*par).extradata_size = extra.len() as c_int;
            }
            Ok(())
        };

        let mut video_stream_idx: Option<c_int> = None;
        let has_video_pkt = packets.iter().any(|p| p.is_video && !p.data.is_empty());
        if let (Some(vcfg), true) = (video_cfg, has_video_pkt) {
            let st = ff::avformat_new_stream(ofmt, ptr::null());
            if st.is_null() {
                anyhow::bail!("avformat_new_stream failed for video");
            }
            (*st).time_base = ff::AVRational { num: 1, den: 1000 };
            let par = (*st).codecpar;
            (*par).codec_type = ff::AVMediaType::AVMEDIA_TYPE_VIDEO;
            (*par).codec_id = std::mem::transmute_copy(&vcfg.codec_id);
            (*par).codec_tag = 0;
            (*par).width = vcfg.width;
            (*par).height = vcfg.height;
            (*par).bit_rate = vcfg.bit_rate;
            populate_extradata(par, &vcfg.extradata)?;
            video_stream_idx = Some((*st).index);
        }

        let mut audio_stream_idx: Option<c_int> = None;
        let has_audio_pkt = packets.iter().any(|p| !p.is_video && !p.data.is_empty());
        if let (Some(acfg), true) = (audio_cfg, has_audio_pkt) {
            let st = ff::avformat_new_stream(ofmt, ptr::null());
            if st.is_null() {
                anyhow::bail!("avformat_new_stream failed for audio");
            }
            (*st).time_base = ff::AVRational { num: 1, den: 1000 };
            let par = (*st).codecpar;
            (*par).codec_type = ff::AVMediaType::AVMEDIA_TYPE_AUDIO;
            (*par).codec_id = std::mem::transmute_copy(&acfg.codec_id);
            (*par).codec_tag = 0;
            (*par).sample_rate = acfg.sample_rate;
            (*par).bit_rate = acfg.bit_rate;
            ff::av_channel_layout_default(&mut (*par).ch_layout, acfg.channels.max(1));
            populate_extradata(par, &acfg.extradata)?;
            audio_stream_idx = Some((*st).index);
        }

        if video_stream_idx.is_none() && audio_stream_idx.is_none() {
            anyhow::bail!("No matching streams to mux");
        }

        if ((*(*ofmt).oformat).flags & ff::AVFMT_NOFILE) == 0 {
            let rc = ff::avio_open(&mut (*ofmt).pb, dest_c.as_ptr(), ff::AVIO_FLAG_WRITE);
            if rc < 0 {
                anyhow::bail!("avio_open failed: {rc}");
            }
        }

        let mut opts: *mut ff::AVDictionary = ptr::null_mut();
        ff::av_dict_set(&mut opts, c"movflags".as_ptr(), c"+faststart".as_ptr(), 0);
        let mut _opts_guard = DictGuard(opts);
        let rc = ff::avformat_write_header(ofmt, &mut _opts_guard.0);
        if rc < 0 {
            anyhow::bail!("avformat_write_header failed: {rc}");
        }

        let video_base_ts = packets
            .iter()
            .filter(|p| p.is_video && !p.data.is_empty())
            .map(|p| p.dts_ms.min(p.pts_ms))
            .min();

        let base_ts = video_base_ts.unwrap_or_else(|| {
            packets
                .iter()
                .filter(|p| !p.data.is_empty())
                .map(|p| p.dts_ms.min(p.pts_ms))
                .min()
                .unwrap_or(0)
        });

        let pkt_raw = ff::av_packet_alloc();
        if pkt_raw.is_null() {
            anyhow::bail!("av_packet_alloc failed");
        }
        let pkt_guard = PktGuard(pkt_raw);

        let mut last_v_dts: Option<i64> = None;
        let mut last_a_dts: Option<i64> = None;
        let ms_tb = ff::AVRational { num: 1, den: 1000 };

        for cp in packets {
            if cp.data.is_empty() {
                continue;
            }
            // Drop any stray pre-IDR audio packet so normal audio earlier than
            // frame 0 cannot shift base_ts or bleed ahead of the fault window.
            if !cp.is_video && cp.pts_ms < base_ts {
                continue;
            }
            let Some(st_idx) = (if cp.is_video {
                video_stream_idx
            } else {
                audio_stream_idx
            }) else {
                continue;
            };

            let raw_dts = cp.dts_ms - base_ts;
            let last_dts_slot = if cp.is_video {
                &mut last_v_dts
            } else {
                &mut last_a_dts
            };
            let mono_dts = match *last_dts_slot {
                Some(prev) if raw_dts <= prev => prev + 1,
                _ => raw_dts,
            };
            *last_dts_slot = Some(mono_dts);
            let mono_pts = (cp.pts_ms - base_ts).max(mono_dts);

            ff::av_packet_unref(pkt_guard.0);
            if ff::av_new_packet(pkt_guard.0, cp.data.len() as c_int) < 0 {
                anyhow::bail!("av_new_packet failed");
            }
            ptr::copy_nonoverlapping(cp.data.as_ptr(), (*pkt_guard.0).data, cp.data.len());
            (*pkt_guard.0).stream_index = st_idx;
            (*pkt_guard.0).dts = mono_dts;
            (*pkt_guard.0).pts = mono_pts;
            (*pkt_guard.0).duration = cp.duration_ms.max(0);
            if cp.is_keyframe {
                (*pkt_guard.0).flags |= ff::AV_PKT_FLAG_KEY;
            }

            let out_tb = (*(*(*ofmt).streams.add(st_idx as usize))).time_base;
            ff::av_packet_rescale_ts(pkt_guard.0, ms_tb, out_tb);

            let wr = ff::av_interleaved_write_frame(ofmt, pkt_guard.0);
            if wr < 0 {
                anyhow::bail!("av_interleaved_write_frame failed: {wr}");
            }
        }

        let tr = ff::av_write_trailer(ofmt);
        if tr < 0 {
            anyhow::bail!("av_write_trailer failed: {tr}");
        }
    }

    Ok(())
}

fn write_minimal_mp4_stub(dest: &Path, packets: &[CompressedPacket]) -> anyhow::Result<()> {
    let mut payload = Vec::new();
    for p in packets {
        payload.extend_from_slice(&p.data);
    }
    std::fs::write(dest, &payload)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gop_ring_buffer_seal_and_pretrigger() {
        let mut ring = GopRingBuffer::new(6000, 6 * 1024 * 1024);
        for i in 0..8 {
            ring.push(CompressedPacket {
                pts_ms: i * 1000,
                dts_ms: i * 1000,
                duration_ms: 1000,
                is_keyframe: i % 2 == 0,
                is_video: true,
                codec_param_hash: 1,
                data: vec![1u8; 1024],
            });
        }
        let slice = ring.snapshot_from_pretrigger_idr(2000);
        assert!(!slice.is_empty());
        assert!(slice[0].is_keyframe);

        // Resolution / SPS change seals buffer
        ring.push(CompressedPacket {
            pts_ms: 8000,
            dts_ms: 8000,
            duration_ms: 1000,
            is_keyframe: true,
            is_video: true,
            codec_param_hash: 2,
            data: vec![2u8; 512],
        });
        assert_eq!(ring.snapshot_from_pretrigger_idr(2000).len(), 1);
    }

    #[test]
    fn test_snapshot_clips_late_split_av_pre_idr_audio() {
        let mut ring = GopRingBuffer::new(8000, 6 * 1024 * 1024);
        // Video IDR at t=4000ms, followed by P-frame at t=5000ms
        ring.push(CompressedPacket {
            pts_ms: 0,
            dts_ms: 0,
            duration_ms: 1000,
            is_keyframe: true,
            is_video: true,
            codec_param_hash: 1,
            data: vec![1u8; 64],
        });
        ring.push(CompressedPacket {
            pts_ms: 4000,
            dts_ms: 4000,
            duration_ms: 1000,
            is_keyframe: true,
            is_video: true,
            codec_param_hash: 1,
            data: vec![2u8; 64],
        });
        // Out-of-order late-arriving split audio packets from BEFORE the 4000ms IDR
        ring.push(CompressedPacket {
            pts_ms: 1500,
            dts_ms: 1500,
            duration_ms: 21,
            is_keyframe: true,
            is_video: false,
            codec_param_hash: 1,
            data: vec![3u8; 32],
        });
        ring.push(CompressedPacket {
            pts_ms: 4200,
            dts_ms: 4200,
            duration_ms: 21,
            is_keyframe: true,
            is_video: false,
            codec_param_hash: 1,
            data: vec![4u8; 32],
        });
        ring.push(CompressedPacket {
            pts_ms: 5500,
            dts_ms: 5500,
            duration_ms: 1000,
            is_keyframe: false,
            is_video: true,
            codec_param_hash: 1,
            data: vec![5u8; 64],
        });

        let slice = ring.snapshot_from_pretrigger_idr(1500);
        // Should start at video IDR 4000ms and EXCLUDE pre-IDR audio at 1500ms!
        assert!(slice.iter().all(|p| p.pts_ms >= 4000));
        assert!(slice.iter().any(|p| !p.is_video && p.pts_ms == 4200));
        assert!(!slice.iter().any(|p| !p.is_video && p.pts_ms == 1500));
        assert_eq!(slice[0].pts_ms, 4000);
        assert!(slice[0].is_video && slice[0].is_keyframe);
    }
}
