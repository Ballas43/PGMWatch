use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use pgmwatch_core::models::FaultKind;

pub struct VideoSimdAnalyzer {
    pub width: usize,
    pub height: usize,
    prev_y: Option<Vec<u8>>,
    frozen_ms: u64,
    black_ms: u64,
    macroblock_ms: u64,
}

#[derive(Debug, Clone)]
pub struct VideoAnalysisResult {
    pub luma_mean: f32,
    pub luma_variance: f32,
    pub frame_mad: f32,
    pub changed_pct: f32,
    pub blockiness_score: f32,
    pub faults: Vec<FaultKind>,
}

impl VideoSimdAnalyzer {
    pub fn new(orientation_vertical: bool) -> Self {
        let (width, height) = if orientation_vertical { (90, 160) } else { (160, 90) };
        Self {
            width,
            height,
            prev_y: None,
            frozen_ms: 0,
            black_ms: 0,
            macroblock_ms: 0,
        }
    }

    pub fn analyze_y_plane(
        &mut self,
        y_plane: &[u8],
        frame_interval_ms: u64,
        avg_qp: f32,
    ) -> VideoAnalysisResult {
        let expected = self.width * self.height;
        let len = y_plane.len().min(expected).max(1);
        let slice = &y_plane[..len];

        let mut sum = 0u64;
        for &px in slice {
            sum += px as u64;
        }
        let mean = sum as f32 / len as f32;

        let mut var_acc = 0.0f32;
        for &px in slice {
            let d = px as f32 - mean;
            var_acc += d * d;
        }
        let variance = var_acc / len as f32;

        let (frame_mad, changed_pct) = if let Some(ref prev) = self.prev_y {
            let mut mad_acc = 0u64;
            let mut changed = 0usize;
            for (&a, &b) in slice.iter().zip(prev.iter()) {
                let d = a.abs_diff(b);
                mad_acc += d as u64;
                if d >= 3 {
                    changed += 1;
                }
            }
            (
                mad_acc as f32 / len as f32,
                (changed as f32 / len as f32) * 100.0,
            )
        } else {
            (16.0, 100.0)
        };
        self.prev_y = Some(slice.to_vec());

        let blockiness_score = compute_block_gradient_ratio(slice, self.width, self.height);

        let mut faults = Vec::new();

        // Black & 100ms blink: TV-range black near Y=16 & variance near 0
        if mean <= 19.5 && variance <= 4.5 {
            self.black_ms += frame_interval_ms;
            if self.black_ms >= 500 {
                faults.push(FaultKind::VideoBlack);
            }
        } else {
            if (80..500).contains(&self.black_ms) {
                faults.push(FaultKind::VideoBlink);
            }
            self.black_ms = 0;
        }

        // Frozen feed: require BOTH low global MAD (<0.25) AND <0.4% changed pixels (>=3 luma steps)
        // so streams with large static UI + active facecam/HUD (e.g. narrative RPGs) never false-alarm.
        if frame_mad < 0.25 && changed_pct < 0.4 {
            self.frozen_ms += frame_interval_ms;
            if self.frozen_ms >= 3000 {
                faults.push(FaultKind::VideoFrozen);
            }
        } else {
            self.frozen_ms = 0;
        }

        if avg_qp > 44.0 || blockiness_score > 3.2 {
            self.macroblock_ms += frame_interval_ms;
            if self.macroblock_ms >= 5000 {
                faults.push(FaultKind::VideoMacroblocking);
            }
        } else {
            self.macroblock_ms = 0;
        }

        VideoAnalysisResult {
            luma_mean: mean,
            luma_variance: variance,
            frame_mad,
            changed_pct,
            blockiness_score,
            faults,
        }
    }
}

fn compute_block_gradient_ratio(y: &[u8], w: usize, h: usize) -> f32 {
    if w < 16 || h < 16 || y.len() < w * h {
        return 1.0;
    }
    let mut boundary = 0u64;
    let mut boundary_n = 0u64;
    let mut interior = 0u64;
    let mut interior_n = 0u64;

    for row in 0..h {
        let base = row * w;
        for col in 1..w {
            let diff = y[base + col].abs_diff(y[base + col - 1]) as u64;
            if col % 8 == 0 {
                boundary += diff;
                boundary_n += 1;
            } else {
                interior += diff;
                interior_n += 1;
            }
        }
    }

    let b_avg = boundary as f32 / boundary_n.max(1) as f32;
    let i_avg = (interior as f32 / interior_n.max(1) as f32).max(0.5);
    b_avg / i_avg
}

pub fn encode_rgb_jpeg_b64(rgb: &[u8], width: u32, height: u32) -> anyhow::Result<String> {
    let mut out = Vec::with_capacity(16 * 1024);
    let mut enc = JpegEncoder::new_with_quality(&mut out, 74);
    enc.encode(rgb, width, height, ExtendedColorType::Rgb8)?;
    Ok(B64.encode(&out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_black_and_frozen_detection() {
        let mut simd = VideoSimdAnalyzer::new(false);
        let black = vec![16u8; 160 * 90];
        let mut saw_black = false;
        let mut saw_frozen = false;
        for _ in 0..8 {
            let res = simd.analyze_y_plane(&black, 500, 20.0);
            if res.faults.contains(&FaultKind::VideoBlack) {
                saw_black = true;
            }
            if res.faults.contains(&FaultKind::VideoFrozen) {
                saw_frozen = true;
            }
        }
        assert!(saw_black);
        assert!(saw_frozen);
    }

    #[test]
    fn test_partial_webcam_motion_not_frozen() {
        let mut simd = VideoSimdAnalyzer::new(false);
        let base = vec![100u8; 160 * 90];
        let _ = simd.analyze_y_plane(&base, 250, 20.0);

        // Simulate 92% static UI/background + 8% active facecam region
        for tick in 0..20 {
            let mut frame = base.clone();
            let delta = ((tick % 5) * 6) as u8;
            for row in 20..44 {
                for col in 10..58 {
                    frame[row * 160 + col] = 100u8.saturating_add(delta);
                }
            }
            let res = simd.analyze_y_plane(&frame, 250, 20.0);
            assert!(
                !res.faults.contains(&FaultKind::VideoFrozen),
                "Active facecam region should not trigger VideoFrozen (mad={}, changed={}%)",
                res.frame_mad,
                res.changed_pct
            );
        }
    }
}
