use super::{FfiFfmpegReader, MediaOpenOptions};
use crate::backend::find_ffmpeg9_binary;
use crate::ring_buffer::{ClipSaver, GopRingBuffer};
use chrono::Utc;
use ffmpeg_sys_next as ff;
use std::ffi::CString;
use std::process::{Command, Stdio};
use std::ptr;
use std::thread;
use std::time::Duration;

#[test]
fn test_ffi_decode_real_mp4_telemetry_and_pretrigger_clip() {
    let ffmpeg = find_ffmpeg9_binary();
    let tmp = std::env::temp_dir().join(format!("pgmwatch_ffi_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let src_mp4 = tmp.join("source.mp4");
    let clips_dir = tmp.join("clips");

    let status = Command::new(&ffmpeg)
        .args([
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "smptebars=s=320x180:r=30:d=4",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=4",
            "-c:v",
            "mpeg4",
            "-q:v",
            "5",
            "-g",
            "30",
            "-c:a",
            "aac",
            "-shortest",
        ])
        .arg(&src_mp4)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("ffmpeg test source generation failed");
    assert!(status.success() && src_mp4.exists());

    let reader = FfiFfmpegReader::start(
        src_mp4.to_str().unwrap(),
        None,
        &[],
        false,
        false,
        &MediaOpenOptions::default(),
    )
    .expect("FfiFfmpegReader::start failed on local mp4");

    let mut ring = GopRingBuffer::new(6000, 8 * 1024 * 1024);
    let saver = ClipSaver::new(clips_dir.clone(), 1.0).unwrap();

    let mut saw_pcm = false;
    let mut saw_video = false;
    let mut measured_w = 0u32;
    let mut measured_h = 0u32;
    let mut max_kbps = 0.0f32;
    let mut saw_keyframe = false;

    for tick in 1..=30u64 {
        thread::sleep(Duration::from_millis(80));
        if let Some(batch) = reader.poll_batch(false, tick, tick == 1) {
            if !batch.pcm_f32.is_empty() {
                saw_pcm = true;
            }
            if batch.has_new_video && !batch.y_plane.is_empty() {
                saw_video = true;
            }
            if batch.width > 0 {
                measured_w = batch.width;
                measured_h = batch.height;
            }
            if batch.rx_kbps > max_kbps {
                max_kbps = batch.rx_kbps;
            }
            if let Some(vcfg) = batch.video_codec_cfg {
                ring.set_video_config(vcfg);
            }
            if let Some(acfg) = batch.audio_codec_cfg {
                ring.set_audio_config(acfg);
            }
            for p in batch.packets {
                if p.is_video && p.is_keyframe {
                    saw_keyframe = true;
                }
                ring.push(p);
            }
            if saw_pcm && saw_video && saw_keyframe && ring.video_config().is_some() {
                break;
            }
        }
    }

    assert!(saw_pcm, "Expected decoded 48kHz PCM samples");
    assert!(saw_video, "Expected decoded Y-plane video frame");
    assert_eq!((measured_w, measured_h), (320, 180));
    assert!(max_kbps > 0.0, "Expected non-zero measured rx_kbps, got {max_kbps}");
    assert!(ring.video_config().is_some(), "Expected extracted video codec config");
    assert!(ring.audio_config().is_some(), "Expected extracted audio codec config");

    let snap = ring.snapshot_from_pretrigger_idr(2000);
    assert!(!snap.is_empty(), "Expected buffered pre-trigger packets");
    let clip_name = saver
        .save_clip(
            "ffi-test",
            None,
            Some("sim://offline"),
            &snap,
            ring.video_config(),
            ring.audio_config(),
            Utc::now(),
        )
        .expect("Pre-trigger MP4 mux failed");
    let clip_path = clips_dir.join(&clip_name);
    assert!(clip_path.exists());

    let clip_c = CString::new(clip_path.to_str().unwrap()).unwrap();
    unsafe {
        let mut fmt: *mut ff::AVFormatContext = ptr::null_mut();
        let rc = ff::avformat_open_input(&mut fmt, clip_c.as_ptr(), ptr::null(), ptr::null_mut());
        assert!(rc >= 0 && !fmt.is_null(), "avformat_open_input failed on muxed clip: {rc}");
        assert!((*fmt).nb_streams >= 1, "Expected >= 1 stream in muxed pre-trigger clip");
        ff::avformat_close_input(&mut fmt);
    }

    drop(reader);
    let _ = std::fs::remove_dir_all(&tmp);
}
