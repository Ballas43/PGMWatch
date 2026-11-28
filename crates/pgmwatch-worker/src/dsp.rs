use ebur128::{EbuR128, Mode};
use pgmwatch_core::models::{AudioTelemetry, FaultKind, Platform};
use std::collections::VecDeque;

pub const CANONICAL_SAMPLE_RATE: u32 = 48_000;
pub const CANONICAL_CHANNELS: u32 = 2;
/// 16x boxcar decimation (48 kHz -> 3 kHz) gives 1.0 Hz resolution over a 1.0-second (3000-sample)
/// Goertzel window, sharp enough to separate exact 50/60/100/120 Hz AC mains hum from musical bass notes.
const HUM_DECIM_FACTOR: usize = 16;
const HUM_SAMPLE_RATE: f32 = (CANONICAL_SAMPLE_RATE as f32) / (HUM_DECIM_FACTOR as f32);
const HUM_WIN_SAMPLES: usize = 3_000;
const HUM_PERSIST_MS: u64 = 5_000;

pub struct AudioDspAnalyzer {
    target_lufs: f32,
    lufs_tolerance: f32,
    ebur: EbuR128,
    hum_hann: Vec<f32>,
    hum_win_buf: Vec<f32>,
    hum_decim_ring: VecDeque<f32>,
    hum_decim_acc: f32,
    hum_decim_cnt: usize,
    master_lufs_history: VecDeque<f32>,
    self_lufs_history: VecDeque<f32>,
    lufs_bad_ms: u64,
    dead_air_ms: u64,
    micro_silence_ms: u64,
    missing_channel_ms: u64,
    phase_inv_ms: u64,
    hum_ms: u64,
    hum_sample_ms: u64,
    wrong_bus_ms: u64,
    wrong_bus_sample_ms: u64,
    last_wrong_bus_corr: f32,
    consecutive_clip_samples: u32,
    hum_candidate_hz: Option<f32>,
    hum_ref_dbfs: Option<f32>,
    last_hum_freq: Option<f32>,
    last_hum_peak_db: f32,
    decayed_true_peak_dbtp: f32,
}

impl AudioDspAnalyzer {
    pub fn new(platform: Platform) -> anyhow::Result<Self> {
        let ebur = EbuR128::new(
            CANONICAL_CHANNELS,
            CANONICAL_SAMPLE_RATE,
            Mode::I | Mode::S | Mode::TRUE_PEAK,
        )?;
        let hum_hann: Vec<f32> = (0..HUM_WIN_SAMPLES)
            .map(|i| {
                0.5 * (1.0
                    - ((2.0 * std::f32::consts::PI * i as f32) / (HUM_WIN_SAMPLES as f32 - 1.0))
                        .cos())
            })
            .collect();

        Ok(Self {
            target_lufs: platform.target_lufs(),
            lufs_tolerance: platform.lufs_tolerance(),
            ebur,
            hum_hann,
            hum_win_buf: vec![0.0; HUM_WIN_SAMPLES],
            hum_decim_ring: VecDeque::with_capacity(HUM_WIN_SAMPLES + 64),
            hum_decim_acc: 0.0,
            hum_decim_cnt: 0,
            master_lufs_history: VecDeque::with_capacity(180),
            self_lufs_history: VecDeque::with_capacity(180),
            lufs_bad_ms: 0,
            dead_air_ms: 0,
            micro_silence_ms: 0,
            missing_channel_ms: 0,
            phase_inv_ms: 0,
            hum_ms: 0,
            hum_sample_ms: 0,
            wrong_bus_ms: 0,
            wrong_bus_sample_ms: 0,
            last_wrong_bus_corr: 1.0,
            consecutive_clip_samples: 0,
            hum_candidate_hz: None,
            hum_ref_dbfs: None,
            last_hum_freq: None,
            last_hum_peak_db: 0.0,
            decayed_true_peak_dbtp: -96.0,
        })
    }

    /// Update the active target LUFS and tolerance window live without restarting the decoder.
    pub fn set_lufs_target(&mut self, target_lufs: f32, tolerance_lu: f32) {
        if target_lufs.is_finite() {
            self.target_lufs = target_lufs.clamp(-36.0, -5.0);
        }
        if tolerance_lu.is_finite() {
            self.lufs_tolerance = tolerance_lu.clamp(0.5, 10.0);
        }
        self.lufs_bad_ms = 0;
    }

    /// Reset all audio metrics (integrated LUFS, short-term LUFS, true peak hold, hum, and fault timers).
    pub fn reset_average_lufs(&mut self) {
        if let Ok(fresh) = EbuR128::new(
            CANONICAL_CHANNELS,
            CANONICAL_SAMPLE_RATE,
            Mode::I | Mode::S | Mode::TRUE_PEAK,
        ) {
            self.ebur = fresh;
        }
        self.hum_decim_ring.clear();
        self.hum_decim_acc = 0.0;
        self.hum_decim_cnt = 0;
        self.master_lufs_history.clear();
        self.self_lufs_history.clear();
        self.lufs_bad_ms = 0;
        self.dead_air_ms = 0;
        self.micro_silence_ms = 0;
        self.missing_channel_ms = 0;
        self.phase_inv_ms = 0;
        self.hum_ms = 0;
        self.hum_sample_ms = 0;
        self.wrong_bus_ms = 0;
        self.wrong_bus_sample_ms = 0;
        self.last_wrong_bus_corr = 1.0;
        self.consecutive_clip_samples = 0;
        self.hum_candidate_hz = None;
        self.hum_ref_dbfs = None;
        self.last_hum_freq = None;
        self.last_hum_peak_db = 0.0;
        self.decayed_true_peak_dbtp = -96.0;
    }

    /// Analyze interleaved 48 kHz Stereo F32LE frames and return telemetry + triggered faults.
    pub fn process_f32_interleaved(
        &mut self,
        interleaved: &[f32],
        group_master_lufs: Option<f32>,
    ) -> (AudioTelemetry, Vec<FaultKind>) {
        let frame_count = interleaved.len() / 2;
        if frame_count == 0 {
            return (AudioTelemetry::default(), Vec::new());
        }

        let block_ms = ((frame_count as f64 / CANONICAL_SAMPLE_RATE as f64) * 1000.0).round() as u64;
        let _ = self.ebur.add_frames_f32(interleaved);

        let mut left_sq = 0.0f64;
        let mut right_sq = 0.0f64;
        let mut cross_lr = 0.0f64;
        let mut max_consec_clip = 0u32;

        for pair in interleaved.as_chunks::<2>().0 {
            let l = pair[0];
            let r = pair[1];
            left_sq += (l as f64) * (l as f64);
            right_sq += (r as f64) * (r as f64);
            cross_lr += (l as f64) * (r as f64);

            if l.abs() >= 0.9999 || r.abs() >= 0.9999 {
                self.consecutive_clip_samples += 1;
                max_consec_clip = max_consec_clip.max(self.consecutive_clip_samples);
            } else {
                self.consecutive_clip_samples = 0;
            }

            self.hum_decim_acc += 0.5 * (l + r);
            self.hum_decim_cnt += 1;
            if self.hum_decim_cnt == HUM_DECIM_FACTOR {
                if self.hum_decim_ring.len() >= HUM_WIN_SAMPLES {
                    self.hum_decim_ring.pop_front();
                }
                self.hum_decim_ring
                    .push_back(self.hum_decim_acc / (HUM_DECIM_FACTOR as f32));
                self.hum_decim_acc = 0.0;
                self.hum_decim_cnt = 0;
            }
        }

        let n = frame_count as f64;
        let left_rms = (left_sq / n).sqrt();
        let right_rms = (right_sq / n).sqrt();
        let combined_rms = ((left_sq + right_sq) / (2.0 * n)).sqrt();

        let left_rms_dbfs = linear_to_dbfs(left_rms);
        let right_rms_dbfs = linear_to_dbfs(right_rms);
        let combined_dbfs = linear_to_dbfs(combined_rms);

        let phase_correlation = if left_sq > 1e-12 && right_sq > 1e-12 {
            (cross_lr / (left_sq.sqrt() * right_sq.sqrt())).clamp(-1.0, 1.0) as f32
        } else {
            1.0
        };

        let integrated_lufs = sanitize_lufs(self.ebur.loudness_global());
        let short_term_lufs = sanitize_lufs(self.ebur.loudness_shortterm());

        let tp_l = self.ebur.prev_true_peak(0).unwrap_or(0.0);
        let tp_r = self.ebur.prev_true_peak(1).unwrap_or(0.0);
        let instant_true_peak_dbtp = linear_to_dbfs(tp_l.max(tp_r)).clamp(-96.0, 24.0);
        const TRUE_PEAK_DECAY_DB_PER_SEC: f32 = 12.0;
        let block_secs = frame_count as f32 / CANONICAL_SAMPLE_RATE as f32;
        let decay_db = TRUE_PEAK_DECAY_DB_PER_SEC * block_secs;
        self.decayed_true_peak_dbtp =
            instant_true_peak_dbtp.max((self.decayed_true_peak_dbtp - decay_db).max(-96.0));
        let true_peak_dbtp = self.decayed_true_peak_dbtp;

        // Grid-locked 1.0 Hz Goertzel AC mains hum detector (exact 50/60/100/120 Hz)
        // Requires razor-sharp frequency lock, constant amplitude (+-3.5 dB), and 5s persistence
        // so waiting-screen BGM, synth pads, and bass drones never false-alarm.
        if self.hum_decim_ring.len() == HUM_WIN_SAMPLES {
            self.hum_sample_ms += block_ms;
            if self.hum_sample_ms >= 500 {
                let hum_step_ms = self.hum_sample_ms;
                self.hum_sample_ms = 0;
                if let Some((f, peak_db, level_dbfs)) = self.detect_narrowband_hum() {
                    let same_freq = self
                        .hum_candidate_hz
                        .map(|prev| (prev - f).abs() < 1.0)
                        .unwrap_or(true);
                    let stable_level = self
                        .hum_ref_dbfs
                        .map(|prev| (prev - level_dbfs).abs() <= 3.5)
                        .unwrap_or(true);
                    if same_freq && stable_level {
                        self.hum_ms += hum_step_ms;
                        // Slowly track reference level while staying locked
                        self.hum_ref_dbfs = Some(match self.hum_ref_dbfs {
                            Some(prev) => prev * 0.8 + level_dbfs * 0.2,
                            None => level_dbfs,
                        });
                    } else {
                        self.hum_ms = hum_step_ms;
                        self.hum_ref_dbfs = Some(level_dbfs);
                    }
                    self.hum_candidate_hz = Some(f);
                    if self.hum_ms >= HUM_PERSIST_MS {
                        self.last_hum_freq = Some(f);
                        self.last_hum_peak_db = peak_db;
                    } else {
                        self.last_hum_freq = None;
                        self.last_hum_peak_db = 0.0;
                    }
                } else {
                    self.hum_ms = 0;
                    self.hum_candidate_hz = None;
                    self.hum_ref_dbfs = None;
                    self.last_hum_freq = None;
                    self.last_hum_peak_db = 0.0;
                }
            }
        }

        let mut faults = Vec::new();

        // Digital clipping check: instantaneous True Peak >= -0.1 dBTP or >=3 consecutive clipped samples
        if instant_true_peak_dbtp >= -0.1 || max_consec_clip >= 3 {
            faults.push(FaultKind::AudioClipping);
        }

        // Micro-silence (< -85 dBFS for 30-500ms) and dead air (< -65 dBFS for 5s)
        if combined_dbfs < -85.0 {
            self.micro_silence_ms += block_ms;
            self.dead_air_ms += block_ms;
        } else if combined_dbfs < -65.0 {
            self.micro_silence_ms = 0;
            self.dead_air_ms += block_ms;
        } else {
            if (30..=500).contains(&self.micro_silence_ms) {
                faults.push(FaultKind::AudioMicroSilence);
            }
            self.micro_silence_ms = 0;
            self.dead_air_ms = 0;
        }
        if self.dead_air_ms >= 5000 {
            faults.push(FaultKind::AudioDeadAir);
        }

        // Missing channel check: one side < -60 dBFS while other > -35 dBFS for 3s
        let one_missing = (left_rms_dbfs < -60.0 && right_rms_dbfs > -35.0)
            || (right_rms_dbfs < -60.0 && left_rms_dbfs > -35.0);
        if one_missing {
            self.missing_channel_ms += block_ms;
        } else {
            self.missing_channel_ms = 0;
        }
        if self.missing_channel_ms >= 3000 {
            faults.push(FaultKind::AudioMissingChannel);
        }

        // Phase inversion check: < -0.2 for 3s
        if phase_correlation < -0.2 && combined_dbfs > -50.0 {
            self.phase_inv_ms += block_ms;
        } else {
            self.phase_inv_ms = 0;
        }
        if self.phase_inv_ms >= 3000 {
            faults.push(FaultKind::AudioPhaseInversion);
        }

        if self.hum_ms >= HUM_PERSIST_MS {
            faults.push(FaultKind::AudioMainsHum);
        }

        // Average (integrated) LUFS target check: target +- tolerance for 10s (ignore during silence)
        if combined_dbfs > -55.0 && integrated_lufs > -60.0 {
            let diff = (integrated_lufs - self.target_lufs).abs();
            if diff > self.lufs_tolerance {
                self.lufs_bad_ms += block_ms;
            } else {
                self.lufs_bad_ms = 0;
            }
            if self.lufs_bad_ms >= 10_000 {
                faults.push(FaultKind::LufsOutOfRange);
            }
        } else {
            self.lufs_bad_ms = 0;
        }

        // Intra-group wrong-bus audio check against Group Master rolling loudness (+/-30s lag tolerance)
        const WRONG_BUS_SAMPLE_MS: u64 = 500;
        const WRONG_BUS_HIST_CAP: usize = 180;
        const WRONG_BUS_WINDOW: usize = 30;
        const WRONG_BUS_MAX_LAG: usize = 60;

        if let Some(master_lufs) = group_master_lufs {
            self.wrong_bus_sample_ms += block_ms;
            if self.wrong_bus_sample_ms >= WRONG_BUS_SAMPLE_MS {
                self.wrong_bus_sample_ms = 0;
                if self.master_lufs_history.len() >= WRONG_BUS_HIST_CAP {
                    self.master_lufs_history.pop_front();
                }
                if self.self_lufs_history.len() >= WRONG_BUS_HIST_CAP {
                    self.self_lufs_history.pop_front();
                }
                self.master_lufs_history.push_back(master_lufs);
                self.self_lufs_history.push_back(short_term_lufs);

                if self.master_lufs_history.len() >= WRONG_BUS_WINDOW
                    && self.self_lufs_history.len() >= WRONG_BUS_WINDOW
                {
                    self.last_wrong_bus_corr = max_lag_pearson_corr(
                        &mut self.master_lufs_history,
                        &mut self.self_lufs_history,
                        WRONG_BUS_WINDOW,
                        WRONG_BUS_MAX_LAG,
                    );
                }
            }

            if self.master_lufs_history.len() >= WRONG_BUS_WINDOW
                && self.self_lufs_history.len() >= WRONG_BUS_WINDOW
            {
                if self.last_wrong_bus_corr < 0.15 {
                    self.wrong_bus_ms += block_ms;
                } else {
                    self.wrong_bus_ms = 0;
                }
                if self.wrong_bus_ms >= 15_000 {
                    faults.push(FaultKind::WrongBusAudio);
                }
            }
        } else {
            self.wrong_bus_ms = 0;
            self.wrong_bus_sample_ms = 0;
            self.last_wrong_bus_corr = 1.0;
            self.master_lufs_history.clear();
            self.self_lufs_history.clear();
        }

        let telemetry = AudioTelemetry {
            integrated_lufs,
            short_term_lufs,
            true_peak_dbtp,
            left_rms_dbfs,
            right_rms_dbfs,
            phase_correlation,
            hum_peak_db: self.last_hum_peak_db,
            hum_freq_hz: self.last_hum_freq,
        };

        (telemetry, faults)
    }

    /// Evaluates exact 50/60/100/120 Hz AC mains grid frequencies with 1.0 Hz Goertzel resolution.
    /// Returns `Some((freq_hz, prominence_db, level_dbfs))` only when a razor-sharp tone is locked
    /// directly onto a mains grid frequency (rejecting nearby musical bass notes like 49/55/61.7/98/123.5 Hz).
    fn detect_narrowband_hum(&mut self) -> Option<(f32, f32, f32)> {
        for (dst, (&sample, &w)) in self
            .hum_win_buf
            .iter_mut()
            .zip(self.hum_decim_ring.iter().zip(self.hum_hann.iter()))
        {
            *dst = sample * w;
        }

        const MAINS_TARGETS: [f32; 4] = [50.0, 60.0, 100.0, 120.0];
        let mut best: Option<(f32, f32, f32)> = None;

        for f0 in MAINS_TARGETS {
            let p0 = goertzel_dbfs(&self.hum_win_buf, f0, HUM_SAMPLE_RATE);
            if p0 < -45.0 {
                continue;
            }
            // Inner +-1.5 Hz selectivity guards: on a 1.0s Hann window, a true grid-locked tone at f0
            // sits >15 dB above f0 +- 1.5 Hz, whereas musical notes (e.g. G1=49, A1=55, B1=61.7 Hz)
            // spill into or dominate the guard bins and are immediately rejected.
            let p_in_lo = goertzel_dbfs(&self.hum_win_buf, f0 - 1.5, HUM_SAMPLE_RATE);
            let p_in_hi = goertzel_dbfs(&self.hum_win_buf, f0 + 1.5, HUM_SAMPLE_RATE);
            if (p0 - p_in_lo) < 10.0 || (p0 - p_in_hi) < 10.0 {
                continue;
            }

            // Outer shoulders (+-3.5 / +-5.0 Hz) measure the local low-frequency noise/music floor.
            let shoulder_avg = 0.25
                * (goertzel_dbfs(&self.hum_win_buf, f0 - 5.0, HUM_SAMPLE_RATE)
                    + goertzel_dbfs(&self.hum_win_buf, f0 - 3.5, HUM_SAMPLE_RATE)
                    + goertzel_dbfs(&self.hum_win_buf, f0 + 3.5, HUM_SAMPLE_RATE)
                    + goertzel_dbfs(&self.hum_win_buf, f0 + 5.0, HUM_SAMPLE_RATE));
            let prom = p0 - shoulder_avg;
            if prom >= 24.0 && best.map(|(_, bp, _)| prom > bp).unwrap_or(true) {
                best = Some((f0, prom, p0));
            }
        }

        best
    }
}

fn goertzel_dbfs(windowed: &[f32], freq_hz: f32, sample_rate: f32) -> f32 {
    let omega = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
    let cos_w = omega.cos();
    let sin_w = omega.sin();
    let coeff = 2.0 * cos_w;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    for &x in windowed {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let real = s1 - s2 * cos_w;
    let imag = s2 * sin_w;
    let mag = (real * real + imag * imag).sqrt();
    // Normalize by N * 0.5 (Hann coherent gain = 0.5, single-sided sine factor = 2) -> peak amplitude.
    let amp = (4.0 * mag) / (windowed.len().max(1) as f32);
    20.0 * (amp + 1e-9).log10()
}

fn sanitize_lufs(val: Result<f64, ebur128::Error>) -> f32 {
    match val {
        Ok(v) if v.is_finite() => v.clamp(-70.0, 0.0) as f32,
        _ => -70.0,
    }
}

fn linear_to_dbfs(val: f64) -> f32 {
    if !val.is_finite() || val <= 1e-6 {
        -120.0
    } else {
        (20.0 * val.log10()) as f32
    }
}

fn pearson_slice(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len()) as f32;
    if n < 2.0 {
        return 1.0;
    }
    let mean_a = a.iter().sum::<f32>() / n;
    let mean_b = b.iter().sum::<f32>() / n;
    let mut num = 0.0f32;
    let mut da = 0.0f32;
    let mut db = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let vx = x - mean_a;
        let vy = y - mean_b;
        num += vx * vy;
        da += vx * vx;
        db += vy * vy;
    }
    let denom = (da * db).sqrt();
    if denom < 1e-6 {
        1.0
    } else {
        (num / denom).clamp(-1.0, 1.0)
    }
}

fn max_lag_pearson_corr(
    master: &mut VecDeque<f32>,
    self_hist: &mut VecDeque<f32>,
    window: usize,
    max_lag: usize,
) -> f32 {
    let m = master.make_contiguous();
    let s = self_hist.make_contiguous();
    let len = m.len().min(s.len());
    if len < window {
        return 1.0;
    }
    let avail_lag = (len - window).min(max_lag);
    let mut best = pearson_slice(&m[len - window..len], &s[len - window..len]);
    for lag in 1..=avail_lag {
        let lead_corr = pearson_slice(&m[len - window - lag..len - lag], &s[len - window..len]);
        let trail_corr = pearson_slice(&m[len - window..len], &s[len - window - lag..len - lag]);
        best = best.max(lead_corr).max(trail_corr);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dead_air_and_clipping_detection() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Youtube).unwrap();
        let silence = vec![0.0f32; 48_000 * 2]; // 1s chunk
        for _ in 0..5 {
            let (_, faults) = dsp.process_f32_interleaved(&silence, None);
            let _ = faults;
        }
        let (_, faults) = dsp.process_f32_interleaved(&silence, None);
        assert!(faults.contains(&FaultKind::AudioDeadAir));

        let mut clip_buf = vec![0.2f32; 4800 * 2];
        clip_buf[..10].fill(1.0);
        let (_, clip_faults) = dsp.process_f32_interleaved(&clip_buf, None);
        assert!(clip_faults.contains(&FaultKind::AudioClipping));
    }

    #[test]
    fn test_mains_hum_detection() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Twitch).unwrap();
        let mut tone = Vec::with_capacity(48_000 * 2);
        for i in 0..48_000 {
            let s = 0.15 * (2.0 * std::f32::consts::PI * 50.0 * (i as f32 / 48_000.0)).sin();
            tone.push(s);
            tone.push(s);
        }
        // 5 continuous seconds of constant-amplitude 50.0 Hz hum must trigger AudioMainsHum
        let mut last = (AudioTelemetry::default(), Vec::new());
        for _ in 0..5 {
            last = dsp.process_f32_interleaved(&tone, None);
        }
        let (tel, faults) = last;
        assert!(tel.hum_peak_db >= 24.0, "got {}", tel.hum_peak_db);
        assert_eq!(tel.hum_freq_hz, Some(50.0));
        assert!(faults.contains(&FaultKind::AudioMainsHum));
    }

    #[test]
    fn test_waiting_screen_bgm_bass_drones_do_not_trigger_hum() {
        // Musical bass notes near 50/60/100/120 Hz (G1=49, A1=55, B1=61.74, G2=98, B2=123.47 Hz)
        // sustained for 7 seconds must NEVER false-alarm as AC mains hum.
        for &note_hz in &[49.0f32, 55.0, 61.74, 98.0, 123.47] {
            let mut dsp = AudioDspAnalyzer::new(Platform::Youtube).unwrap();
            let mut tone = Vec::with_capacity(48_000 * 2);
            for i in 0..48_000 {
                let s = 0.25
                    * (2.0 * std::f32::consts::PI * note_hz * (i as f32 / 48_000.0)).sin();
                tone.push(s);
                tone.push(s);
            }
            for _ in 0..7 {
                let (tel, faults) = dsp.process_f32_interleaved(&tone, None);
                assert!(
                    !faults.contains(&FaultKind::AudioMainsHum),
                    "Musical bass note {note_hz} Hz false-flagged AudioMainsHum ({:?})",
                    tel.hum_freq_hz
                );
            }
        }
    }

    #[test]
    fn test_healthy_music_and_speech_do_not_trigger_hum() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Youtube).unwrap();
        // Simulated healthy program: speech/music harmonics (165/220/330/440 Hz) + broadband noise, no 50/60 Hz hum
        for block in 0..30 {
            let mut buf = Vec::with_capacity(4800 * 2);
            for i in 0..4800 {
                let t = (block as f32 * 0.1) + (i as f32 / 48_000.0);
                let carrier = 0.35 * (2.0 * std::f32::consts::PI * 220.0 * t).sin()
                    + 0.25 * (2.0 * std::f32::consts::PI * 330.0 * t).sin()
                    + 0.15 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
                    + 0.12 * (2.0 * std::f32::consts::PI * 165.0 * t).sin();
                let s = carrier * (0.7 + 0.3 * (2.0 * std::f32::consts::PI * 2.5 * t).sin());
                buf.push(s);
                buf.push(s * 0.96);
            }
            let (tel, faults) = dsp.process_f32_interleaved(&buf, None);
            assert!(
                tel.hum_freq_hz.is_none(),
                "Healthy audio false-flagged mains hum: {:?}",
                tel.hum_freq_hz
            );
            assert!(!faults.contains(&FaultKind::AudioMainsHum));
        }
    }

    #[test]
    fn test_transient_low_tone_does_not_trigger_hum() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Twitch).unwrap();
        let mut bass_thump = Vec::with_capacity(4800 * 2);
        for i in 0..4800 {
            let t = i as f32 / 48_000.0;
            let s = 0.25 * (2.0 * std::f32::consts::PI * 55.0 * t).sin();
            bass_thump.push(s);
            bass_thump.push(s);
        }
        // 500ms of sub-bass followed by normal midtone audio must stay clean (< 2000ms persistence gate)
        for _ in 0..5 {
            let (tel, faults) = dsp.process_f32_interleaved(&bass_thump, None);
            assert!(tel.hum_freq_hz.is_none());
            assert!(!faults.contains(&FaultKind::AudioMainsHum));
        }
        let mut clean = Vec::with_capacity(4800 * 2);
        for i in 0..4800 {
            let s = 0.2 * (2.0 * std::f32::consts::PI * 440.0 * (i as f32 / 48_000.0)).sin();
            clean.push(s);
            clean.push(s);
        }
        let (tel, faults) = dsp.process_f32_interleaved(&clean, None);
        assert!(tel.hum_freq_hz.is_none());
        assert!(!faults.contains(&FaultKind::AudioMainsHum));
    }

    #[test]
    fn test_average_lufs_integration_and_reset() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Youtube).unwrap();
        let make_sine_1s = |amp: f32| -> Vec<f32> {
            let mut buf = Vec::with_capacity(48_000 * 2);
            for i in 0..48_000 {
                let s = amp * (2.0 * std::f32::consts::PI * 1000.0 * (i as f32 / 48_000.0)).sin();
                buf.push(s);
                buf.push(s);
            }
            buf
        };

        let loud = make_sine_1s(0.5);
        let (t_loud, _) = dsp.process_f32_interleaved(&loud, None);
        assert!(
            t_loud.integrated_lufs > -12.0 && t_loud.integrated_lufs < -4.0,
            "Expected loud integrated average around -6..-9 LUFS, got {}",
            t_loud.integrated_lufs
        );

        dsp.reset_average_lufs();
        // A 100ms silent block (< 400ms gating window) immediately reports reset floor -70.0
        let tiny_silence = vec![0.0f32; 4800 * 2];
        let (t_reset, _) = dsp.process_f32_interleaved(&tiny_silence, None);
        assert_eq!(t_reset.integrated_lufs, -70.0);

        // Feeding quieter post-reset audio converges purely to the quieter level, forgetting pre-reset loud audio
        let quiet = make_sine_1s(0.05);
        let (t_quiet, _) = dsp.process_f32_interleaved(&quiet, None);
        assert!(
            t_quiet.integrated_lufs < t_loud.integrated_lufs - 15.0,
            "Post-reset integrated LUFS ({}) should reflect only quieter audio, vs pre-reset ({})",
            t_quiet.integrated_lufs,
            t_loud.integrated_lufs
        );
    }

    #[test]
    fn test_custom_lufs_target_override() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Youtube).unwrap();
        let mut buf = Vec::with_capacity(48_000 * 2);
        for i in 0..48_000 {
            let s = 0.25 * (2.0 * std::f32::consts::PI * 1000.0 * (i as f32 / 48_000.0)).sin();
            buf.push(s);
            buf.push(s);
        }
        // ~-15 dBFS sine: within default YouTube (-14 +-2.5 LUFS) -> no LUFS alert
        for _ in 0..11 {
            let (_, faults) = dsp.process_f32_interleaved(&buf, None);
            assert!(!faults.contains(&FaultKind::LufsOutOfRange));
        }

        // Tighten custom user target to -23.0 +-2.0 LUFS -> must flag LufsOutOfRange after 10s
        dsp.set_lufs_target(-23.0, 2.0);
        let mut saw_lufs_fault = false;
        for _ in 0..11 {
            let (_, faults) = dsp.process_f32_interleaved(&buf, None);
            if faults.contains(&FaultKind::LufsOutOfRange) {
                saw_lufs_fault = true;
            }
        }
        assert!(saw_lufs_fault, "Expected LufsOutOfRange under custom -23 LUFS target");

        // Updating back near measured loudness immediately resets bad accumulator
        dsp.set_lufs_target(-15.0, 2.5);
        let (_, faults_after) = dsp.process_f32_interleaved(&buf, None);
        assert!(!faults_after.contains(&FaultKind::LufsOutOfRange));

        // Momentary quieter passage that swings short-term LUFS below target must NOT warn
        // while cumulative integrated Avg LUFS remains within tolerance.
        let mut quiet_dip = Vec::with_capacity(48_000 * 2);
        for i in 0..48_000 {
            let s = 0.05 * (2.0 * std::f32::consts::PI * 1000.0 * (i as f32 / 48_000.0)).sin();
            quiet_dip.push(s);
            quiet_dip.push(s);
        }
        for _ in 0..10 {
            let (tel, faults_dip) = dsp.process_f32_interleaved(&quiet_dip, None);
            if (tel.integrated_lufs - (-15.0)).abs() <= 2.5 {
                assert!(
                    !faults_dip.contains(&FaultKind::LufsOutOfRange),
                    "Integrated Avg LUFS ({}) is in spec; short-term dip ({}) must not trigger warning",
                    tel.integrated_lufs,
                    tel.short_term_lufs
                );
            }
        }
    }

    #[test]
    fn test_wrong_bus_tolerates_encoding_lag_up_to_30s() {
        let mut master = VecDeque::new();
        let mut lagged_same = VecDeque::new();
        // 160 samples at 2 Hz = 80s history; `lagged_same` lags Master by 44 samples (+22s HLS latency).
        for i in 0..160usize {
            let sig = |t: usize| -18.0 + ((t as f32) * 0.35).sin() * 5.0 + ((t as f32) * 0.13).cos() * 2.5;
            master.push_back(sig(i + 44));
            lagged_same.push_back(sig(i));
        }

        let same_corr = max_lag_pearson_corr(&mut master, &mut lagged_same, 30, 60);
        assert!(
            same_corr > 0.90,
            "Expected high correlation despite +22s encoding lag, got {same_corr}"
        );

        // Uncorrelated opposing ramps stay below 0.15 across all +/-30s lags -> WrongBusAudio fires.
        let mut up = VecDeque::new();
        let mut down = VecDeque::new();
        for i in 0..160usize {
            up.push_back(-30.0 + (i as f32) * 0.1);
            down.push_back(-10.0 - (i as f32) * 0.1);
        }
        let opp_corr = max_lag_pearson_corr(&mut up, &mut down, 30, 60);
        assert!(
            opp_corr < 0.15,
            "Expected wrong-bus anti-correlated streams < 0.15, got {opp_corr}"
        );
    }

    #[test]
    fn test_realtime_true_peak_auto_decays_and_does_not_latch_clipping() {
        let mut dsp = AudioDspAnalyzer::new(Platform::Youtube).expect("dsp init");
        // 100ms block at 48kHz stereo = 4800 frames
        let mut loud = Vec::with_capacity(4800 * 2);
        for i in 0..4800 {
            let s = (2.0 * std::f32::consts::PI * 1000.0 * (i as f32 / 48_000.0)).sin();
            loud.push(s);
            loud.push(s);
        }
        let (tel_loud, faults_loud) = dsp.process_f32_interleaved(&loud, None);
        assert!(
            tel_loud.true_peak_dbtp >= -0.5,
            "Expected instantaneous true peak near 0 dBTP, got {}",
            tel_loud.true_peak_dbtp
        );
        assert!(faults_loud.contains(&FaultKind::AudioClipping));

        // Follow with 1.0s of quiet (-26 dBFS) audio in 100ms blocks:
        // clipping fault must clear immediately and true peak must decay smoothly (~12 dB/s).
        let mut quiet = Vec::with_capacity(4800 * 2);
        for i in 0..4800 {
            let s = 0.05 * (2.0 * std::f32::consts::PI * 1000.0 * (i as f32 / 48_000.0)).sin();
            quiet.push(s);
            quiet.push(s);
        }
        let mut last_tp = tel_loud.true_peak_dbtp;
        for _ in 0..10 {
            let (tel_q, faults_q) = dsp.process_f32_interleaved(&quiet, None);
            assert!(
                !faults_q.contains(&FaultKind::AudioClipping),
                "AudioClipping must not latch after signal drops"
            );
            assert!(
                tel_q.true_peak_dbtp < last_tp,
                "Expected true peak to decay each block ({} -> {})",
                last_tp,
                tel_q.true_peak_dbtp
            );
            last_tp = tel_q.true_peak_dbtp;
        }
        assert!(
            last_tp < -10.0,
            "Expected ~12 dB decay after 1s of quiet audio, got {last_tp}"
        );
    }
}
