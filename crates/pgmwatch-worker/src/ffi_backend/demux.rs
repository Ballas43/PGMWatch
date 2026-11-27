use super::{
    extract_codec_config, hash_codec_cfg, interrupt_cb, sync_split_av_peer, ts_to_ms, DemuxTask,
    FfiSharedState, AUDIO_CHUNK_SAMPLES, MAX_PACKETS_QUEUED,
};
use crate::dsp::{CANONICAL_CHANNELS, CANONICAL_SAMPLE_RATE};
use crate::ring_buffer::CompressedPacket;
use crate::telemetry::{detect_pts_jump, pace_demux_to_realtime};
use ffmpeg_sys_next as ff;
use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

struct InFmtGuard(*mut ff::AVFormatContext);
impl Drop for InFmtGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::avformat_close_input(&mut self.0);
            }
        }
    }
}

struct CodecGuard(*mut ff::AVCodecContext);
impl Drop for CodecGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::avcodec_free_context(&mut self.0);
            }
        }
    }
}

struct FrameGuard(*mut ff::AVFrame);
impl Drop for FrameGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::av_frame_free(&mut self.0);
            }
        }
    }
}

struct PacketGuard(*mut ff::AVPacket);
impl Drop for PacketGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::av_packet_free(&mut self.0);
            }
        }
    }
}

struct SwrGuard(*mut ff::SwrContext);
impl Drop for SwrGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::swr_free(&mut self.0);
            }
        }
    }
}

struct SwsGuard(*mut ff::SwsContext);
impl Drop for SwsGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::sws_freeContext(self.0);
            }
        }
    }
}

struct BufRefGuard(*mut ff::AVBufferRef);
impl Drop for BufRefGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                ff::av_buffer_unref(&mut self.0);
            }
        }
    }
}

pub(super) fn run_demux_loop(
    shared: Arc<FfiSharedState>,
    task: DemuxTask,
    ready_tx: mpsc::SyncSender<Result<(), String>>,
) {
    let url = task.url.as_str();
    let headers = task.headers.as_slice();
    let want_audio = task.want_audio;
    let want_video = task.want_video;
    let thumb_w = task.thumb_w;
    let thumb_h = task.thumb_h;
    let media_opts = &task.media_opts;

    let Ok(url_c) = CString::new(url) else {
        let _ = ready_tx.send(Err("Invalid URL with interior NUL".into()));
        return;
    };

    let stop_ref = Arc::clone(&shared.stop_flag);
    let lower_url = url.trim().to_ascii_lowercase();
    let is_network = lower_url.starts_with("http://")
        || lower_url.starts_with("https://")
        || lower_url.starts_with("rtmp://")
        || lower_url.starts_with("rtmps://")
        || lower_url.starts_with("srt://");

    unsafe {
        let mut ifmt = ff::avformat_alloc_context();
        if ifmt.is_null() {
            let _ = ready_tx.send(Err("avformat_alloc_context failed".into()));
            return;
        }
        (*ifmt).interrupt_callback = ff::AVIOInterruptCB {
            callback: Some(interrupt_cb),
            opaque: Arc::as_ptr(&stop_ref) as *mut c_void,
        };

        let mut opts: *mut ff::AVDictionary = ptr::null_mut();
        if !headers.is_empty() {
            let mut h_str = String::new();
            for (k, v) in headers {
                let ck: String = k.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
                let cv: String = v.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').collect();
                if !ck.is_empty() && !cv.is_empty() {
                    h_str.push_str(&format!("{ck}: {cv}\r\n"));
                }
            }
            if let Ok(hc) = CString::new(h_str) {
                ff::av_dict_set(&mut opts, c"headers".as_ptr(), hc.as_ptr(), 0);
            }
        }
        if is_network {
            let is_http = lower_url.starts_with("http://") || lower_url.starts_with("https://");
            ff::av_dict_set(&mut opts, c"rw_timeout".as_ptr(), c"5000000".as_ptr(), 0);
            if is_http {
                ff::av_dict_set(&mut opts, c"reconnect".as_ptr(), c"1".as_ptr(), 0);
                ff::av_dict_set(&mut opts, c"reconnect_streamed".as_ptr(), c"1".as_ptr(), 0);
            }
            if !lower_url.starts_with("srt://")
                && let Some(ip) = media_opts
                    .bind_ip
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty() && s.parse::<std::net::IpAddr>().is_ok())
                && let Ok(ip_c) = CString::new(ip)
            {
                ff::av_dict_set(&mut opts, c"local_addr".as_ptr(), ip_c.as_ptr(), 0);
            }
        }

        let open_rc = ff::avformat_open_input(&mut ifmt, url_c.as_ptr(), ptr::null(), &mut opts);
        if !opts.is_null() {
            ff::av_dict_free(&mut opts);
        }
        if open_rc < 0 || ifmt.is_null() {
            if !ifmt.is_null() {
                ff::avformat_free_context(ifmt);
            }
            let _ = ready_tx.send(Err(format!("avformat_open_input failed: {open_rc}")));
            return;
        }
        let _ifmt_guard = InFmtGuard(ifmt);

        if ff::avformat_find_stream_info(ifmt, ptr::null_mut()) < 0 {
            let _ = ready_tx.send(Err("avformat_find_stream_info failed".into()));
            return;
        }

        let mut audio_idx: c_int = -1;
        let mut video_idx: c_int = -1;
        for i in 0..(*ifmt).nb_streams as usize {
            let st = *(*ifmt).streams.add(i);
            let ct = (*(*st).codecpar).codec_type;
            if want_audio && audio_idx < 0 && ct == ff::AVMediaType::AVMEDIA_TYPE_AUDIO {
                audio_idx = i as c_int;
            }
            if want_video && video_idx < 0 && ct == ff::AVMediaType::AVMEDIA_TYPE_VIDEO {
                video_idx = i as c_int;
            }
        }

        if audio_idx < 0 && video_idx < 0 {
            let _ = ready_tx.send(Err("No requested audio/video streams found".into()));
            return;
        }

        let mut actx_guard = CodecGuard(ptr::null_mut());
        let mut audio_hash = 0u64;
        let mut audio_tb = ff::AVRational { num: 1, den: 1000 };
        if audio_idx >= 0 {
            let st = *(*ifmt).streams.add(audio_idx as usize);
            audio_tb = (*st).time_base;
            let cfg = extract_codec_config(st);
            audio_hash = hash_codec_cfg(&cfg);
            if let Ok(mut g) = shared.audio_codec_cfg.lock() {
                *g = Some(cfg);
            }
            shared.audio_cfg_dirty.store(true, Ordering::Release);

            let dec = ff::avcodec_find_decoder((*(*st).codecpar).codec_id);
            if dec.is_null() {
                let _ = ready_tx.send(Err("Audio decoder not found".into()));
                return;
            }
            let ctx = ff::avcodec_alloc_context3(dec);
            if ctx.is_null() {
                let _ = ready_tx.send(Err("Audio avcodec_alloc_context3 failed".into()));
                return;
            }
            actx_guard = CodecGuard(ctx);
            if ff::avcodec_parameters_to_context(ctx, (*st).codecpar) < 0
                || ff::avcodec_open2(ctx, dec, ptr::null_mut()) < 0
            {
                let _ = ready_tx.send(Err("Audio decoder open failed".into()));
                return;
            }
        }

        let mut hw_dev_guard = BufRefGuard(ptr::null_mut());
        let mut vctx_guard = CodecGuard(ptr::null_mut());
        let mut video_hash = 0u64;
        let mut hw_active = false;
        let mut video_tb = ff::AVRational { num: 1, den: 1000 };

        let open_video_ctx = |st: *const ff::AVStream, use_hw: bool, hw_dev: *mut ff::AVBufferRef| -> Option<(*mut ff::AVCodecContext, bool)> {
            let dec = ff::avcodec_find_decoder((*(*st).codecpar).codec_id);
            if dec.is_null() {
                return None;
            }
            let ctx = ff::avcodec_alloc_context3(dec);
            if ctx.is_null() {
                return None;
            }
            if ff::avcodec_parameters_to_context(ctx, (*st).codecpar) < 0 {
                let mut tmp = ctx;
                ff::avcodec_free_context(&mut tmp);
                return None;
            }
            let mut hw_on = false;
            if use_hw && !hw_dev.is_null() {
                (*ctx).hw_device_ctx = ff::av_buffer_ref(hw_dev);
                hw_on = !(*ctx).hw_device_ctx.is_null();
                if hw_on && media_opts.extra_hw_frames > 0 {
                    (*ctx).extra_hw_frames = media_opts.extra_hw_frames as c_int;
                }
            }
            if ff::avcodec_open2(ctx, dec, ptr::null_mut()) < 0 {
                let mut tmp = ctx;
                ff::avcodec_free_context(&mut tmp);
                return None;
            }
            Some((ctx, hw_on))
        };

        if video_idx >= 0 {
            let st = *(*ifmt).streams.add(video_idx as usize);
            video_tb = (*st).time_base;
            let cfg = extract_codec_config(st);
            video_hash = hash_codec_cfg(&cfg);
            let w = cfg.width.max(0) as u32;
            let h = cfg.height.max(0) as u32;
            let fr = if (*st).avg_frame_rate.den > 0 && (*st).avg_frame_rate.num > 0 {
                (*st).avg_frame_rate.num as f32 / (*st).avg_frame_rate.den as f32
            } else if (*st).r_frame_rate.den > 0 && (*st).r_frame_rate.num > 0 {
                (*st).r_frame_rate.num as f32 / (*st).r_frame_rate.den as f32
            } else {
                30.0
            };
            if let Ok(mut g) = shared.video_codec_cfg.lock() {
                *g = Some(cfg);
            }
            shared.video_cfg_dirty.store(true, Ordering::Release);
            if let Ok(mut tel) = shared.telemetry.lock() {
                tel.width = w;
                tel.height = h;
                tel.fps = fr.clamp(1.0, 120.0);
            }

            let mut hw_dev: *mut ff::AVBufferRef = ptr::null_mut();
            let dev_cstr = media_opts
                .hwaccel_device
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .and_then(|s| CString::new(s).ok());
            let hw_rc = ff::av_hwdevice_ctx_create(
                &mut hw_dev,
                ff::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
                dev_cstr.as_ref().map_or(ptr::null(), |c| c.as_ptr()),
                ptr::null_mut(),
                0,
            );
            if hw_rc >= 0 && !hw_dev.is_null() {
                hw_dev_guard = BufRefGuard(hw_dev);
            }
            if let Some((ctx, hw_on)) = open_video_ctx(st, hw_rc >= 0, hw_dev_guard.0) {
                vctx_guard = CodecGuard(ctx);
                hw_active = hw_on;
            } else if let Some((ctx, _)) = open_video_ctx(st, false, ptr::null_mut()) {
                vctx_guard = CodecGuard(ctx);
                hw_active = false;
            } else {
                let _ = ready_tx.send(Err("Video decoder open failed".into()));
                return;
            }
            shared.hw_accelerated.store(hw_active, Ordering::Release);
        }

        let _ = ready_tx.send(Ok(()));

        let pkt_guard = PacketGuard(ff::av_packet_alloc());
        let aframe_guard = FrameGuard(ff::av_frame_alloc());
        let vframe_guard = FrameGuard(ff::av_frame_alloc());
        let swframe_guard = FrameGuard(ff::av_frame_alloc());
        if pkt_guard.0.is_null()
            || aframe_guard.0.is_null()
            || vframe_guard.0.is_null()
            || swframe_guard.0.is_null()
        {
            shared.ended.store(true, Ordering::Relaxed);
            return;
        }

        let mut swr_guard = SwrGuard(ptr::null_mut());
        let mut sws_guard = SwsGuard(ptr::null_mut());
        let mut sws_src_w = 0;
        let mut sws_src_h = 0;
        let mut sws_src_fmt = ff::AVPixelFormat::AV_PIX_FMT_NONE;

        let mut pcm_accum: Vec<f32> = Vec::with_capacity(AUDIO_CHUNK_SAMPLES * 2);
        let mut rgb_buf: Vec<u8> = if thumb_w > 0 && thumb_h > 0 {
            vec![0u8; thumb_w * thumb_h * 3]
        } else {
            Vec::new()
        };
        let mut last_thumb_push = Instant::now() - Duration::from_secs(1);
        let mut hw_fail_streak = 0u32;
        let mut frames_decoded_hw = 0u64;

        let mut pace_origin_wall: Option<Instant> = None;
        let mut pace_origin_pts_ms: Option<i64> = None;
        let mut waited_for_video_start = false;

        while !shared.stop_flag.load(Ordering::Relaxed) {
            let rd = ff::av_read_frame(ifmt, pkt_guard.0);
            if rd < 0 {
                if rd == ff::AVERROR_EOF {
                    shared.ended.store(true, Ordering::Relaxed);
                }
                break;
            }

            let stream_idx = (*pkt_guard.0).stream_index;
            let pkt_size = (*pkt_guard.0).size.max(0) as usize;
            let st = *(*ifmt).streams.add(stream_idx as usize);
            let tb = (*st).time_base;
            let raw_pts_ms = ts_to_ms((*pkt_guard.0).pts, tb)
                .or_else(|| ts_to_ms((*pkt_guard.0).dts, tb));
            let pts_ms = raw_pts_ms.unwrap_or(0);
            let dts_ms = ts_to_ms((*pkt_guard.0).dts, tb).unwrap_or(pts_ms);
            let dur_ms = ts_to_ms((*pkt_guard.0).duration, tb).unwrap_or(0).max(0);
            let is_video_pkt = stream_idx == video_idx;
            let is_audio_pkt = stream_idx == audio_idx;

            if is_video_pkt || is_audio_pkt {
                if shared.split_av_pair {
                    sync_split_av_peer(
                        &shared,
                        want_audio,
                        want_video,
                        raw_pts_ms,
                        &mut waited_for_video_start,
                    );
                }

                if let Ok(mut tel) = shared.telemetry.lock() {
                    tel.bitrate.push(Instant::now(), raw_pts_ms, pkt_size, is_video_pkt);
                }

                if pkt_size > 0 && !(*pkt_guard.0).data.is_null() {
                    let data = std::slice::from_raw_parts((*pkt_guard.0).data, pkt_size).to_vec();
                    let cp = CompressedPacket {
                        pts_ms,
                        dts_ms,
                        duration_ms: dur_ms,
                        is_keyframe: ((*pkt_guard.0).flags & ff::AV_PKT_FLAG_KEY) != 0,
                        is_video: is_video_pkt,
                        codec_param_hash: if is_video_pkt { video_hash } else { audio_hash },
                        data,
                    };
                    if let Ok(mut pq) = shared.packets.lock() {
                        while pq.len() >= MAX_PACKETS_QUEUED {
                            pq.pop_front();
                        }
                        pq.push_back(cp);
                    }
                }

                pace_demux_to_realtime(
                    is_network,
                    raw_pts_ms,
                    &mut pace_origin_wall,
                    &mut pace_origin_pts_ms,
                );
            }

            if is_audio_pkt && !actx_guard.0.is_null() {
                if let Ok(mut tel) = shared.telemetry.lock() {
                    if detect_pts_jump(tel.prev_stream_audio_pts_ms, pts_ms) {
                        tel.pts_drift_pending = true;
                    }
                    tel.prev_stream_audio_pts_ms = Some(pts_ms);
                }

                let send_rc = ff::avcodec_send_packet(actx_guard.0, pkt_guard.0);
                if send_rc < 0
                    && send_rc != ff::AVERROR(ff::EAGAIN)
                    && let Ok(mut tel) = shared.telemetry.lock()
                {
                    tel.corrupt_pending = true;
                }
                loop {
                    let r = ff::avcodec_receive_frame(actx_guard.0, aframe_guard.0);
                    if r == ff::AVERROR(ff::EAGAIN) || r == ff::AVERROR_EOF || r < 0 {
                        break;
                    }
                    if (((*aframe_guard.0).flags & ff::AV_FRAME_FLAG_CORRUPT) != 0
                        || (*aframe_guard.0).decode_error_flags != 0)
                        && let Ok(mut tel) = shared.telemetry.lock()
                    {
                        tel.corrupt_pending = true;
                    }

                    let aframe_ts = if (*aframe_guard.0).pts != ff::AV_NOPTS_VALUE {
                        (*aframe_guard.0).pts
                    } else {
                        (*aframe_guard.0).pkt_dts
                    };
                    if let Some(ap_ms) = ts_to_ms(aframe_ts, audio_tb)
                        && let Ok(mut tel) = shared.telemetry.lock()
                    {
                        tel.last_audio_pts_ms = Some(ap_ms);
                        tel.av_sync_ms = tel.av_sync_tracker.push_audio(ap_ms, Instant::now());
                    }

                    if swr_guard.0.is_null() {
                        let mut out_ch: ff::AVChannelLayout = std::mem::zeroed();
                        ff::av_channel_layout_default(&mut out_ch, CANONICAL_CHANNELS as c_int);
                        let mut swr: *mut ff::SwrContext = ptr::null_mut();
                        let src_rate = (*aframe_guard.0).sample_rate.max(8000);
                        let src_fmt: ff::AVSampleFormat = std::mem::transmute_copy(&(*aframe_guard.0).format);
                        if ff::swr_alloc_set_opts2(
                            &mut swr,
                            &out_ch,
                            ff::AVSampleFormat::AV_SAMPLE_FMT_FLT,
                            CANONICAL_SAMPLE_RATE as c_int,
                            &(*aframe_guard.0).ch_layout,
                            src_fmt,
                            src_rate,
                            0,
                            ptr::null_mut(),
                        ) >= 0
                            && ff::swr_init(swr) >= 0
                        {
                            swr_guard = SwrGuard(swr);
                        }
                    }

                    if !swr_guard.0.is_null() {
                        let out_cap = (((*aframe_guard.0).nb_samples as i64
                            * CANONICAL_SAMPLE_RATE as i64)
                            / (*aframe_guard.0).sample_rate.max(1) as i64
                            + 64) as usize;
                        let mut out_buf = vec![0.0f32; out_cap * CANONICAL_CHANNELS as usize];
                        let out_ptr = out_buf.as_mut_ptr() as *mut u8;
                        let converted = ff::swr_convert(
                            swr_guard.0,
                            &out_ptr,
                            out_cap as c_int,
                            (*aframe_guard.0).extended_data as *mut *const u8,
                            (*aframe_guard.0).nb_samples,
                        );
                        if converted > 0 {
                            let n_floats = converted as usize * CANONICAL_CHANNELS as usize;
                            pcm_accum.extend_from_slice(&out_buf[..n_floats]);
                            while pcm_accum.len() >= AUDIO_CHUNK_SAMPLES {
                                let chunk: Vec<f32> = pcm_accum.drain(..AUDIO_CHUNK_SAMPLES).collect();
                                if let Ok(mut aq) = shared.audio_queue.lock() {
                                    while aq.len() >= 6 {
                                        aq.pop_front();
                                    }
                                    aq.push_back(chunk);
                                }
                            }
                        }
                    }
                    ff::av_frame_unref(aframe_guard.0);
                }
            } else if is_video_pkt && !vctx_guard.0.is_null() {
                if let Ok(mut tel) = shared.telemetry.lock() {
                    if detect_pts_jump(tel.prev_stream_video_pts_ms, pts_ms) {
                        tel.pts_drift_pending = true;
                    }
                    tel.prev_stream_video_pts_ms = Some(pts_ms);
                }

                let t0 = Instant::now();
                let send_rc = ff::avcodec_send_packet(vctx_guard.0, pkt_guard.0);
                if send_rc < 0 && send_rc != ff::AVERROR(ff::EAGAIN) {
                    if let Ok(mut tel) = shared.telemetry.lock() {
                        tel.corrupt_pending = true;
                    }
                    if hw_active && frames_decoded_hw == 0 {
                        hw_fail_streak += 1;
                    }
                }

                loop {
                    let r = ff::avcodec_receive_frame(vctx_guard.0, vframe_guard.0);
                    if r == ff::AVERROR(ff::EAGAIN) || r == ff::AVERROR_EOF || r < 0 {
                        break;
                    }

                    let mut src_frame = vframe_guard.0;
                    if (*vframe_guard.0).format == ff::AVPixelFormat::AV_PIX_FMT_D3D11 as c_int {
                        let xfer = ff::av_hwframe_transfer_data(swframe_guard.0, vframe_guard.0, 0);
                        if xfer < 0 {
                            hw_fail_streak += 1;
                            ff::av_frame_unref(vframe_guard.0);
                            if hw_fail_streak >= 3 {
                                shared.gpu_fault_pending.store(true, Ordering::Release);
                                shared.hw_accelerated.store(false, Ordering::Release);
                                if let Some((sw_ctx, _)) = open_video_ctx(st, false, ptr::null_mut()) {
                                    vctx_guard = CodecGuard(sw_ctx);
                                    hw_active = false;
                                }
                                hw_fail_streak = 0;
                            }
                            continue;
                        }
                        hw_fail_streak = 0;
                        frames_decoded_hw += 1;
                        src_frame = swframe_guard.0;
                    } else if hw_active {
                        frames_decoded_hw += 1;
                        hw_fail_streak = 0;
                    }

                    let dec_ms = t0.elapsed().as_secs_f32() * 1000.0;
                    let frame_corrupt = ((*vframe_guard.0).flags & ff::AV_FRAME_FLAG_CORRUPT) != 0
                        || (*vframe_guard.0).decode_error_flags != 0;

                    let vframe_ts = if (*vframe_guard.0).pts != ff::AV_NOPTS_VALUE {
                        (*vframe_guard.0).pts
                    } else {
                        (*vframe_guard.0).pkt_dts
                    };
                    let vpts_ms = ts_to_ms(vframe_ts, video_tb);

                    if let Ok(mut tel) = shared.telemetry.lock() {
                        tel.decode_ms = if tel.decode_ms <= 0.01 {
                            dec_ms
                        } else {
                            tel.decode_ms * 0.8 + dec_ms * 0.2
                        };
                        if let Some(vp_ms) = vpts_ms {
                            tel.last_video_pts_ms = Some(vp_ms);
                            tel.av_sync_ms = tel.av_sync_tracker.push_video(vp_ms, Instant::now());
                        }
                        if frame_corrupt {
                            tel.corrupt_pending = true;
                        }
                        tel.avg_qp = 0.0;
                    }

                    if !rgb_buf.is_empty() && last_thumb_push.elapsed() >= Duration::from_millis(240) {
                        let fw = (*src_frame).width;
                        let fh = (*src_frame).height;
                        let ffmt: ff::AVPixelFormat = std::mem::transmute_copy(&(*src_frame).format);
                        if fw > 0 && fh > 0 && ffmt != ff::AVPixelFormat::AV_PIX_FMT_NONE {
                            if sws_guard.0.is_null()
                                || sws_src_w != fw
                                || sws_src_h != fh
                                || sws_src_fmt != ffmt
                            {
                                let sws = ff::sws_getContext(
                                    fw,
                                    fh,
                                    ffmt,
                                    thumb_w as c_int,
                                    thumb_h as c_int,
                                    ff::AVPixelFormat::AV_PIX_FMT_RGB24,
                                    ff::SwsFlags::SWS_BILINEAR as c_int,
                                    ptr::null_mut(),
                                    ptr::null_mut(),
                                    ptr::null(),
                                );
                                sws_guard = SwsGuard(sws);
                                sws_src_w = fw;
                                sws_src_h = fh;
                                sws_src_fmt = ffmt;
                            }
                            if !sws_guard.0.is_null() {
                                let dst_stride = (thumb_w * 3) as c_int;
                                let mut dst_ptrs = [rgb_buf.as_mut_ptr(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()];
                                let mut dst_strides = [dst_stride, 0, 0, 0];
                                ff::sws_scale(
                                    sws_guard.0,
                                    (*src_frame).data.as_ptr() as *const *const u8,
                                    (*src_frame).linesize.as_ptr(),
                                    0,
                                    fh,
                                    dst_ptrs.as_mut_ptr(),
                                    dst_strides.as_mut_ptr(),
                                );
                                if let Ok(mut guard) = shared.latest_rgb.lock() {
                                    *guard = rgb_buf.clone();
                                    shared.rgb_seq.fetch_add(1, Ordering::Release);
                                    last_thumb_push = Instant::now();
                                }
                            }
                        }
                    }

                    ff::av_frame_unref(swframe_guard.0);
                    ff::av_frame_unref(vframe_guard.0);
                }

                if hw_active && hw_fail_streak >= 3 {
                    shared.gpu_fault_pending.store(true, Ordering::Release);
                    shared.hw_accelerated.store(false, Ordering::Release);
                    if let Some((sw_ctx, _)) = open_video_ctx(st, false, ptr::null_mut()) {
                        vctx_guard = CodecGuard(sw_ctx);
                        hw_active = false;
                    }
                    hw_fail_streak = 0;
                }
            }

            ff::av_packet_unref(pkt_guard.0);
        }
    }
}
