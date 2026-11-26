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
}
