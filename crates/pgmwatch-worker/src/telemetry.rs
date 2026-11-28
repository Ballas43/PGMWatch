use std::collections::VecDeque;
use std::thread;
use std::time::{Duration, Instant};

const BITRATE_WINDOW: Duration = Duration::from_secs(8);

#[derive(Clone, Copy)]
struct PktSample {
    wall: Instant,
    pts_ms: Option<i64>,
    bytes: usize,
}

#[derive(Default)]
struct StreamBitrateWindow {
    samples: VecDeque<PktSample>,
    smooth_kbps: f32,
}

impl StreamBitrateWindow {
    fn push(&mut self, wall: Instant, pts_ms: Option<i64>, bytes: usize) {
        if let (Some(cur), Some(last)) = (pts_ms, self.samples.back().and_then(|s| s.pts_ms)) {
            // Reset window on PTS discontinuities so media-span stays bounded to contiguous segments.
            if cur + 2_000 < last || cur > last + 20_000 {
                self.samples.clear();
            }
        }
        self.samples.push_back(PktSample { wall, pts_ms, bytes });
    }

    fn update_kbps(&mut self, now: Instant) -> f32 {
        while let Some(front) = self.samples.front() {
            if now.saturating_duration_since(front.wall) > BITRATE_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        let raw_kbps = if self.samples.is_empty() {
            0.0
        } else {
            let total_bytes: usize = self.samples.iter().map(|s| s.bytes).sum();
            let wall_span_s = self
                .samples
                .front()
                .map(|s| now.saturating_duration_since(s.wall).as_secs_f32())
                .unwrap_or(0.0);
            let mut min_pts = i64::MAX;
            let mut max_pts = i64::MIN;
            let mut valid_pts = 0usize;
            for s in &self.samples {
                if let Some(p) = s.pts_ms {
                    min_pts = min_pts.min(p);
                    max_pts = max_pts.max(p);
                    valid_pts += 1;
                }
            }
            let pts_span_s = if valid_pts >= 2 && max_pts > min_pts {
                ((max_pts - min_pts) as f32 / 1000.0).min(24.0)
            } else {
                0.0
            };
            let span_s = wall_span_s.max(pts_span_s).max(1.0);
            (total_bytes as f32 * 8.0) / (span_s * 1000.0)
        };

        if self.smooth_kbps <= 0.01 {
            self.smooth_kbps = raw_kbps;
        } else {
            self.smooth_kbps = self.smooth_kbps * 0.70 + raw_kbps * 0.30;
        }
        self.smooth_kbps
    }
}

/// Estimates smoothed video and total stream bitrates across bursty HLS/DASH segments.
#[derive(Default)]
pub(crate) struct BitrateEstimator {
    video: StreamBitrateWindow,
    audio: StreamBitrateWindow,
}

impl BitrateEstimator {
    pub(crate) fn push(&mut self, wall: Instant, pts_ms: Option<i64>, bytes: usize, is_video: bool) {
        if is_video {
            self.video.push(wall, pts_ms, bytes);
        } else {
            self.audio.push(wall, pts_ms, bytes);
        }
    }

    pub(crate) fn update(&mut self, now: Instant) -> (f32, f32) {
        let v = self.video.update_kbps(now);
        let a = self.audio.update_kbps(now);
        (v + a, v)
    }
}

pub fn estimate_qp_from_bpp(video_kbps: f32, width: u32, height: u32, fps: f32) -> f32 {
    let px_per_sec = (width.max(1) as f32) * (height.max(1) as f32) * fps.clamp(1.0, 120.0);
    let bpp = (video_kbps.max(1.0) * 1000.0) / px_per_sec;
    let qp = 16.0 - 6.5 * (bpp / 0.08).max(0.0005).log2();
    qp.clamp(12.0, 51.0)
}

pub fn smooth_av_sync(prev_ms: f32, sample_ms: f32) -> f32 {
    if !sample_ms.is_finite() {
        return prev_ms;
    }
    if prev_ms.abs() < 0.01 {
        sample_ms
    } else {
        prev_ms * 0.8 + sample_ms * 0.2
    }
}

/// Tracks true A/V clock drift and container PTS divergence without false positives
/// from constant muxer packet interleaving, B-frame/hardware decoder pipeline lag,
/// or transient HTTP segment fetch jitter on split A/V streams.
pub struct AvSyncTracker {
    split_av_pair: bool,
    last_audio: Option<(i64, Instant)>,
    last_video: Option<(i64, Instant)>,
    cal_samples: Vec<f32>,
    cal_started_at: Option<Instant>,
    baseline_offset_ms: Option<f32>,
    recent_drifts: VecDeque<(Instant, f32)>,
    median_scratch: Vec<f32>,
    last_sample_at: Option<Instant>,
    smoothed_drift_ms: f32,
}

impl AvSyncTracker {
    pub fn new(split_av_pair: bool) -> Self {
        Self {
            split_av_pair,
            last_audio: None,
            last_video: None,
            cal_samples: Vec::with_capacity(32),
            cal_started_at: None,
            baseline_offset_ms: None,
            recent_drifts: VecDeque::with_capacity(256),
            median_scratch: Vec::with_capacity(256),
            last_sample_at: None,
            smoothed_drift_ms: 0.0,
        }
    }

    pub fn reset_calibration(&mut self) {
        self.last_audio = None;
        self.last_video = None;
        self.cal_samples.clear();
        self.cal_started_at = None;
        self.baseline_offset_ms = None;
        self.recent_drifts.clear();
        self.last_sample_at = None;
        self.smoothed_drift_ms = 0.0;
    }

    pub fn push_audio(&mut self, pts_ms: i64, now: Instant) -> f32 {
        self.last_audio = Some((pts_ms, now));
        self.sample(now)
    }

    pub fn push_video(&mut self, pts_ms: i64, now: Instant) -> f32 {
        self.last_video = Some((pts_ms, now));
        self.sample(now)
    }

    fn sample(&mut self, now: Instant) -> f32 {
        let (Some((ap_ms, t_a)), Some((vp_ms, t_v))) = (self.last_audio, self.last_video) else {
            return self.smoothed_drift_ms;
        };

        let wall_delta_ms = if t_a >= t_v {
            t_a.duration_since(t_v).as_secs_f32() * 1000.0
        } else {
            -(t_v.duration_since(t_a).as_secs_f32() * 1000.0)
        };

        // Split A/V DASH streams (e.g. Facebook) download video and audio over two independent
        // HTTP connections. Segment-offset differences between the two download threads (even 6s+)
        // are NOT broadcast lip-sync drift; only actual track starvation (>6s gap where one track
        // stops arriving while the other advances) is a real A/V sync failure, and it must recover
        // immediately once both tracks resume delivering frames.
        if self.split_av_pair {
            let stalled = wall_delta_ms.abs() > 6_000.0;
            if stalled {
                let clamped = wall_delta_ms.clamp(-5_000.0, 5_000.0);
                self.smoothed_drift_ms = smooth_av_sync(self.smoothed_drift_ms, clamped);
            } else {
                self.smoothed_drift_ms = 0.0;
            }
            return self.smoothed_drift_ms;
        }

        // If one track has stalled for >1.5s while the other keeps advancing, do not mask it
        // with wall-clock compensation so true track starvation surfaces as A/V sync drift.
        let stalled = wall_delta_ms.abs() > 1_500.0;
        let max_comp_ms = 600.0;
        let comp_ms = if stalled {
            0.0
        } else {
            wall_delta_ms.clamp(-max_comp_ms, max_comp_ms)
        };

        let raw_offset_ms = ap_ms.saturating_sub(vp_ms) as f32 - comp_ms;
        if !raw_offset_ms.is_finite() {
            return self.smoothed_drift_ms;
        }

        // Muxed HLS/RTMP streams can legitimately carry up to ~600ms interleave offset
        // without any playback lip-sync error.
        let max_normal_interleave_ms = 600.0;

        if self.baseline_offset_ms.is_none() {
            let started = *self.cal_started_at.get_or_insert(now);
            if raw_offset_ms.abs() <= 5_000.0 && self.cal_samples.len() < 240 {
                self.cal_samples.push(raw_offset_ms);
            }
            let elapsed = now.saturating_duration_since(started);
            if self.cal_samples.len() >= 40 && elapsed >= Duration::from_millis(2_500) {
                self.cal_samples
                    .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let med = self.cal_samples[self.cal_samples.len() / 2];
                self.baseline_offset_ms = Some(if med.abs() <= max_normal_interleave_ms {
                    med
                } else {
                    0.0
                });
                self.cal_samples.clear();
                self.cal_samples.shrink_to_fit();
                self.last_sample_at = Some(now);
            } else {
                if raw_offset_ms.abs() > max_normal_interleave_ms {
                    let clamped = raw_offset_ms.clamp(-5_000.0, 5_000.0);
                    self.smoothed_drift_ms = smooth_av_sync(self.smoothed_drift_ms, clamped);
                } else {
                    self.smoothed_drift_ms = 0.0;
                }
                return self.smoothed_drift_ms;
            }
        }

        let dt_s = self
            .last_sample_at
            .map(|prev| now.saturating_duration_since(prev).as_secs_f32())
            .unwrap_or(0.02)
            .clamp(0.001, 2.0);
        self.last_sample_at = Some(now);

        let baseline = self.baseline_offset_ms.unwrap_or(0.0);
        let raw_drift = (raw_offset_ms - baseline).clamp(-5_000.0, 5_000.0);

        // Sliding 2-second median filter rejects transient HLS segment-reload bursts
        // while preserving 100% of sustained encoder clock drift and real lip-sync offsets.
        self.recent_drifts.push_back((now, raw_drift));
        let median_window = Duration::from_secs(2);
        while let Some(&(ts, _)) = self.recent_drifts.front() {
            if now.saturating_duration_since(ts) > median_window || self.recent_drifts.len() > 240 {
                self.recent_drifts.pop_front();
            } else {
                break;
            }
        }
        let filtered_drift = if stalled || self.recent_drifts.len() < 5 {
            raw_drift
        } else {
            self.median_scratch.clear();
            self.median_scratch
                .extend(self.recent_drifts.iter().map(|&(_, v)| v));
            let mid = self.median_scratch.len() / 2;
            let (_, &mut med, _) = self
                .median_scratch
                .select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
            med
        };

        // Slowly adapt baseline only for tiny steady-state interleave shifts inside the normal band
        // so gradual CDN playlist phase shifts don't accumulate, while never absorbing real drift.
        let adapt_band_ms = 40.0;
        if !stalled && filtered_drift.abs() <= adapt_band_ms {
            let adapt_alpha = 1.0 - (-dt_s / 60.0).exp();
            let next_base = (baseline + filtered_drift * adapt_alpha)
                .clamp(-max_normal_interleave_ms, max_normal_interleave_ms);
            self.baseline_offset_ms = Some(next_base);
        }

        // Soft deadband for normal HLS segment-burst jitter while keeping track stalls
        // and sustained encoder clock drift completely visible.
        let deadband_ms = if stalled { 0.0 } else { 35.0 };
        let effective_drift = if filtered_drift.abs() <= deadband_ms {
            0.0
        } else {
            filtered_drift - deadband_ms.copysign(filtered_drift)
        };

        // True wall-clock time-constant EMA (frame-rate independent across 30p/60p/48kHz audio).
        let tau_s = if stalled { 1.0 } else { 2.0 };
        let alpha = 1.0 - (-dt_s / tau_s).exp();
        self.smoothed_drift_ms = self.smoothed_drift_ms * (1.0 - alpha) + effective_drift * alpha;
        self.smoothed_drift_ms
    }
}

pub fn detect_pts_jump(prev_pts_ms: Option<i64>, curr_pts_ms: i64) -> bool {
    let Some(prev) = prev_pts_ms else {
        return false;
    };
    let delta = curr_pts_ms - prev;
    !(-500..=3000).contains(&delta)
}

/// Pace local files (~250ms lead) and network HLS/DASH bursts (~1.5s lead) to wall clock.
pub(crate) fn pace_demux_to_realtime(
    is_network: bool,
    pts_ms: Option<i64>,
    pace_origin_wall: &mut Option<Instant>,
    pace_origin_pts_ms: &mut Option<i64>,
) {
    let Some(pts_ms) = pts_ms.filter(|&p| p >= 0) else {
        return;
    };
    let wall_now = Instant::now();
    let base_w = *pace_origin_wall.get_or_insert(wall_now);
    let base_p = *pace_origin_pts_ms.get_or_insert(pts_ms);
    let pts_elapsed = pts_ms.saturating_sub(base_p);
    let wall_elapsed = wall_now.saturating_duration_since(base_w).as_millis() as i64;
    if pts_elapsed < -1_000 || pts_elapsed > wall_elapsed + 30_000 {
        // Re-anchor pacing across HLS timeline resets, splices, or backward PTS jumps.
        *pace_origin_wall = Some(wall_now);
        *pace_origin_pts_ms = Some(pts_ms);
        return;
    }
    let lead_allowance_ms = if is_network { 1_500 } else { 250 };
    let ahead = pts_elapsed - wall_elapsed;
    if ahead > lead_allowance_ms {
        let wait_ms = if is_network {
            (ahead - 1_200).clamp(10, 50) as u64
        } else {
            (ahead - 100).clamp(15, 100) as u64
        };
        thread::sleep(Duration::from_millis(wait_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pts_drift_and_qp_proxy() {
        assert!(!detect_pts_jump(None, 1000));
        assert!(!detect_pts_jump(Some(1000), 1033));
        assert!(detect_pts_jump(Some(5000), 4000));
        assert!(detect_pts_jump(Some(1000), 5000));

        let qp_high = estimate_qp_from_bpp(8000.0, 1920, 1080, 60.0);
        let qp_low = estimate_qp_from_bpp(200.0, 1920, 1080, 60.0);
        assert!((12.0..=25.0).contains(&qp_high), "qp_high={qp_high}");
        assert!((40.0..=51.0).contains(&qp_low), "qp_low={qp_low}");

        let smoothed = smooth_av_sync(0.0, 12.0);
        assert_eq!(smoothed, 12.0);
        let damped = smooth_av_sync(10.0, 100.0);
        assert!((damped - 28.0).abs() < 0.01, "damped={damped}");
    }

    #[test]
    fn test_bitrate_estimator_bounds_hls_bursts_by_media_pts_span() {
        let mut est = BitrateEstimator::default();
        let t0 = Instant::now();
        // Simulate 4s HLS video segment (2_000_000 bytes = 4000 kbps) bursting in over 20ms wall time
        for i in 0..=40 {
            let wall = t0 + Duration::from_micros(i * 500);
            let pts = 7_740_000 + i as i64 * 100;
            let bytes = if i < 40 { 50_000 } else { 0 };
            est.push(wall, Some(pts), bytes, true);
        }
        // Plus 4s AAC audio (64_000 bytes = 128 kbps)
        est.push(t0, Some(7_740_000), 32_000, false);
        est.push(t0 + Duration::from_millis(10), Some(7_744_000), 32_000, false);

        let (total_kbps, video_kbps) = est.update(t0 + Duration::from_millis(30));
        // Must report ~4000 kbps video / ~4128 kbps total, NOT 20ms-wall 800,000 kbps spike!
        assert!((video_kbps - 4000.0).abs() < 50.0, "video_kbps={video_kbps}");
        assert!((total_kbps - 4128.0).abs() < 50.0, "total_kbps={total_kbps}");

        // During a 2s HLS playlist pause before next segment, media span holds bitrate steady.
        let (_, video_mid) = est.update(t0 + Duration::from_secs(2));
        assert!((video_mid - 4000.0).abs() < 50.0, "video_mid={video_mid}");

        // After window expires with zero packets, EMA smoothly decays downward.
        let mut dec = video_mid;
        for s in 9..=12 {
            let (_, v) = est.update(t0 + Duration::from_secs(s));
            assert!(v < dec, "expected decay at t={s}: {v} < {dec}");
            dec = v;
        }
    }

    #[test]
    fn test_av_sync_tracker_absorbs_muxer_interleave_and_detects_real_drift() {
        let mut tracker = AvSyncTracker::new(false);
        let t0 = Instant::now();

        // Simulate a healthy YouTube HLS stream with constant -350ms muxing/B-frame offset
        for i in 0..80 {
            let now = t0 + Duration::from_millis(i * 40);
            let vp = 10_000 + (i as i64) * 40;
            let ap = vp - 350;
            tracker.push_video(vp, now);
            let drift = tracker.push_audio(ap, now);
            assert!(drift.abs() < 5.0, "healthy stream false drift at i={i}: {drift}");
        }

        // Now inject real +260ms encoder clock drift over subsequent frames
        let mut last_drift = 0.0;
        for i in 80..200 {
            let now = t0 + Duration::from_millis(i * 40);
            let vp = 10_000 + (i as i64) * 40;
            let ap = vp - 350 + 260;
            tracker.push_video(vp, now);
            last_drift = tracker.push_audio(ap, now);
        }
        assert!(
            last_drift > 180.0 && last_drift < 235.0,
            "expected ~+220ms effective drift (260ms - 40ms deadband), got {last_drift}"
        );
    }

    #[test]
    fn test_av_sync_tracker_preserves_severe_initial_pts_mismatch() {
        let mut tracker = AvSyncTracker::new(false);
        let t0 = Instant::now();

        // Broken encoder where audio PTS starts +850ms ahead of video PTS
        let mut last_drift = 0.0;
        for i in 0..120 {
            let now = t0 + Duration::from_millis(i * 40);
            let vp = 10_000 + (i as i64) * 40;
            let ap = vp + 850;
            tracker.push_video(vp, now);
            last_drift = tracker.push_audio(ap, now);
        }
        assert!(
            last_drift > 700.0,
            "severe initial PTS mismatch should not be zeroed out, got {last_drift}"
        );
    }

    #[test]
    fn test_av_sync_tracker_detects_track_stall() {
        let mut tracker = AvSyncTracker::new(false);
        let t0 = Instant::now();

        // Calibrate healthy stream with -60ms interleave (~3.2s)
        for i in 0..80 {
            let now = t0 + Duration::from_millis(i * 40);
            let vp = 10_000 + (i as i64) * 40;
            tracker.push_video(vp, now);
            tracker.push_audio(vp - 60, now);
        }

        // Video stalls while audio keeps advancing for 2.5 seconds
        let mut last_drift = 0.0;
        for i in 80..145 {
            let now = t0 + Duration::from_millis(i * 40);
            let ap = 10_000 + (i as i64) * 40 - 60;
            last_drift = tracker.push_audio(ap, now);
        }
        assert!(
            last_drift > 500.0,
            "video track stall should surface as A/V sync drift, got {last_drift}"
        );
    }

    #[test]
    fn test_av_sync_tracker_split_dash_absorbs_http_phase_shift_but_catches_stall() {
        let mut tracker = AvSyncTracker::new(true);
        let t0 = Instant::now();

        // Calibrate split-AV DASH stream
        for i in 0..80 {
            let now = t0 + Duration::from_millis(i * 40);
            let pts = 20_000 + (i as i64) * 40;
            tracker.push_video(pts, now);
            assert!(tracker.push_audio(pts, now).abs() < 1.0);
        }

        // Simulate a 6,000ms (3-segment) HTTP playlist offset between separate video/audio sockets
        for i in 80..200 {
            let now = t0 + Duration::from_millis(i * 40);
            let pts = 20_000 + (i as i64) * 40;
            tracker.push_video(pts + 6_000, now);
            let drift = tracker.push_audio(pts, now);
            assert!(
                drift.abs() < 1.0,
                "split-DASH HTTP socket segment offset must not false-alarm: {drift}"
            );
        }

        // Genuine 7-second video track stall while audio keeps advancing MUST still alert
        let mut stall_drift = 0.0;
        for i in 200..380 {
            let now = t0 + Duration::from_millis(i * 40);
            let ap = 20_000 + (i as i64) * 40;
            stall_drift = tracker.push_audio(ap, now);
        }
        assert!(
            stall_drift > 500.0,
            "split-DASH real track stall (>6s) must surface as A/V sync drift, got {stall_drift}"
        );

        // As soon as video resumes delivering frames (even 6s ahead in playlist), must recover to 0ms
        let t_rec = t0 + Duration::from_millis(381 * 40);
        let recovered = tracker.push_video(20_000 + 381 * 40 + 6_000, t_rec);
        assert!(
            recovered.abs() < 1.0,
            "split-DASH must recover to 0ms once both tracks resume, got {recovered}"
        );
    }
}
