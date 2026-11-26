use crate::models::{ChannelConfig, GridSample, ManualOverride, MatchMarker, MetricSource, MetricStatus};
use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub fn floor_to_minute_utc(dt: DateTime<Utc>) -> DateTime<Utc> {
    dt.with_second(0)
        .and_then(|d| d.with_nanosecond(0))
        .unwrap_or(dt)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridPoint {
    pub bucket_utc: DateTime<Utc>,
    pub combined_ccv: i64,
    pub per_group_ccv: HashMap<String, i64>,
    pub per_channel_ccv: HashMap<String, i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountingSummary {
    pub total_minutes: usize,
    pub combined_pcv: i64,
    pub combined_acv: f64,
    pub hours_watched: f64,
    pub group_pcv: HashMap<String, i64>,
    pub group_acv: HashMap<String, f64>,
    pub group_hours_watched: HashMap<String, f64>,
    pub timeline: Vec<GridPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchAccounting {
    pub match_marker: MatchMarker,
    pub duration_minutes: usize,
    pub pcv: i64,
    pub acv: f64,
    pub hours_watched: f64,
    pub group_pcv: HashMap<String, i64>,
}

pub fn apply_overrides_to_samples(
    samples: &[GridSample],
    overrides: &[ManualOverride],
) -> Vec<GridSample> {
    let mut out = samples.to_vec();
    for ov in overrides {
        if ov.metric_name.eq_ignore_ascii_case("ccv") {
            for sample in &mut out {
                let bucket_match = match ov.bucket_utc {
                    Some(b) => floor_to_minute_utc(b) == floor_to_minute_utc(sample.bucket_utc),
                    None => true,
                };
                if sample.channel_id == ov.channel_id && bucket_match {
                    sample.ccv = Some(ov.value.round() as i64);
                    sample.ccv_status = MetricStatus::Value;
                    sample.source = MetricSource::ManualStudio;
                    sample.coverage_pct = 100.0;
                }
            }
        }
    }
    out
}

/// Drop accounting-excluded internal QC feeds and shift delayed feeds back to program time.
pub fn prepare_accounting_samples(
    samples: &[GridSample],
    channels: &[ChannelConfig],
) -> Vec<GridSample> {
    let channel_opts: HashMap<&str, (i64, bool)> = channels
        .iter()
        .map(|c| {
            (
                c.id.as_str(),
                (i64::from(c.broadcast_delay_secs), c.exclude_from_accounting),
            )
        })
        .collect();

    samples
        .iter()
        .filter_map(|s| {
            let (delay_secs, exclude) = channel_opts
                .get(s.channel_id.as_str())
                .copied()
                .unwrap_or((0, false));
            if exclude {
                return None;
            }
            let mut copy = s.clone();
            if delay_secs > 0 {
                copy.bucket_utc =
                    floor_to_minute_utc(copy.bucket_utc - Duration::seconds(delay_secs));
            }
            Some(copy)
        })
        .collect()
}

pub fn summarize_grid(samples: &[GridSample]) -> AccountingSummary {
    let mut by_bucket: BTreeMap<DateTime<Utc>, Vec<&GridSample>> = BTreeMap::new();
    for s in samples {
        by_bucket
            .entry(floor_to_minute_utc(s.bucket_utc))
            .or_default()
            .push(s);
    }

    let mut timeline = Vec::with_capacity(by_bucket.len());
    let mut combined_pcv = 0i64;
    let mut combined_sum = 0i64;
    let mut group_pcv: HashMap<String, i64> = HashMap::new();
    let mut group_sums: HashMap<String, i64> = HashMap::new();
    let mut group_counts: HashMap<String, usize> = HashMap::new();

    for (bucket_utc, items) in &by_bucket {
        let mut combined_ccv = 0i64;
        let mut per_group_ccv: HashMap<String, i64> = HashMap::new();
        let mut per_channel_ccv: HashMap<String, i64> = HashMap::new();

        for item in items {
            // Only VALUE status counts toward totals; NA is excluded from averages; UNAVAILABLE contributes 0 unless overridden
            if item.ccv_status == MetricStatus::Value
                && let Some(v) = item.ccv
            {
                combined_ccv += v;
                *per_group_ccv.entry(item.group_id.clone()).or_insert(0) += v;
                per_channel_ccv.insert(item.channel_id.clone(), v);
            }
        }

        combined_pcv = combined_pcv.max(combined_ccv);
        combined_sum += combined_ccv;

        for (gid, gval) in &per_group_ccv {
            let e = group_pcv.entry(gid.clone()).or_insert(0);
            *e = (*e).max(*gval);
            *group_sums.entry(gid.clone()).or_insert(0) += *gval;
            *group_counts.entry(gid.clone()).or_insert(0) += 1;
        }

        timeline.push(GridPoint {
            bucket_utc: *bucket_utc,
            combined_ccv,
            per_group_ccv,
            per_channel_ccv,
        });
    }

    let total_minutes = timeline.len();
    let combined_acv = if total_minutes == 0 {
        0.0
    } else {
        combined_sum as f64 / total_minutes as f64
    };
    let hours_watched = combined_sum as f64 / 60.0;

    let mut group_acv = HashMap::new();
    let mut group_hours_watched = HashMap::new();
    for (gid, sum) in &group_sums {
        let cnt = *group_counts.get(gid).unwrap_or(&1).max(&1);
        group_acv.insert(gid.clone(), *sum as f64 / cnt as f64);
        group_hours_watched.insert(gid.clone(), *sum as f64 / 60.0);
    }

    AccountingSummary {
        total_minutes,
        combined_pcv,
        combined_acv,
        hours_watched,
        group_pcv,
        group_acv,
        group_hours_watched,
        timeline,
    }
}

pub fn slice_matches(samples: &[GridSample], matches: &[MatchMarker]) -> Vec<MatchAccounting> {
    matches
        .iter()
        .map(|m| {
            let end = m.ended_at.unwrap_or_else(Utc::now);
            let filtered: Vec<GridSample> = samples
                .iter()
                .filter(|s| s.bucket_utc >= floor_to_minute_utc(m.started_at) && s.bucket_utc <= end)
                .cloned()
                .collect();
            let sum = summarize_grid(&filtered);
            MatchAccounting {
                match_marker: m.clone(),
                duration_minutes: sum.total_minutes,
                pcv: sum.combined_pcv,
                acv: sum.combined_acv,
                hours_watched: sum.hours_watched,
                group_pcv: sum.group_pcv,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Platform;
    use chrono::TimeZone;

    #[test]
    fn test_60s_grid_and_manual_override() {
        let t1 = Utc.with_ymd_and_hms(2026, 11, 21, 14, 0, 12).unwrap();
        let t2 = Utc.with_ymd_and_hms(2026, 11, 21, 14, 1, 45).unwrap();
        let samples = vec![
            GridSample {
                bucket_utc: floor_to_minute_utc(t1),
                channel_id: "yt-1".into(),
                group_id: "id-pgm".into(),
                platform: Platform::Youtube,
                ccv: Some(12000),
                ccv_status: MetricStatus::Value,
                source: MetricSource::AutoViewer,
                views: Some(50000),
                likes: Some(1400),
                chat_msgs: Some(230),
                shares: None,
                gifts: None,
                coverage_pct: 100.0,
                extras_json: HashMap::new(),
            },
            GridSample {
                bucket_utc: floor_to_minute_utc(t1),
                channel_id: "tt-1".into(),
                group_id: "id-pgm".into(),
                platform: Platform::Tiktok,
                ccv: None,
                ccv_status: MetricStatus::Unavailable,
                source: MetricSource::AutoViewer,
                views: None,
                likes: None,
                chat_msgs: None,
                shares: None,
                gifts: None,
                coverage_pct: 0.0,
                extras_json: HashMap::new(),
            },
            GridSample {
                bucket_utc: floor_to_minute_utc(t2),
                channel_id: "yt-1".into(),
                group_id: "id-pgm".into(),
                platform: Platform::Youtube,
                ccv: Some(15000),
                ccv_status: MetricStatus::Value,
                source: MetricSource::AutoViewer,
                views: Some(61000),
                likes: Some(1650),
                chat_msgs: Some(310),
                shares: None,
                gifts: None,
                coverage_pct: 100.0,
                extras_json: HashMap::new(),
            },
        ];

        let overrides = vec![ManualOverride {
            id: "ov-1".into(),
            channel_id: "tt-1".into(),
            metric_name: "ccv".into(),
            bucket_utc: Some(t1),
            value: 8000.0,
            operator_note: "Studio dashboard read".into(),
            updated_at: t1,
        }];

        let merged = apply_overrides_to_samples(&samples, &overrides);
        let summary = summarize_grid(&merged);
        assert_eq!(summary.combined_pcv, 20000);
        assert_eq!(summary.total_minutes, 2);
        assert!((summary.hours_watched - (35000.0 / 60.0)).abs() < 0.01);
    }

    #[test]
    fn test_delay_aligned_match_and_exclude_from_accounting() {
        let m_start = Utc.with_ymd_and_hms(2026, 11, 25, 18, 0, 0).unwrap();
        let m_end = Utc.with_ymd_and_hms(2026, 11, 25, 18, 30, 0).unwrap();
        let channels = vec![
            ChannelConfig {
                id: "qc-unlisted".into(),
                group_id: "g".into(),
                name: "Studio QC".into(),
                url: "rtmp://localhost/live/qc".into(),
                platform: Platform::Custom,
                tier: crate::models::FeedTier::Master,
                priority: 1,
                orientation_vertical: false,
                enabled: true,
                target_lufs: None,
                lufs_tolerance: None,
                broadcast_delay_secs: 0,
                exclude_from_accounting: true,
                live_streams_only: true,
            },
            ChannelConfig {
                id: "pub-delayed".into(),
                group_id: "g".into(),
                name: "Public +10m".into(),
                url: "https://youtube.com/watch?v=x".into(),
                platform: Platform::Youtube,
                tier: crate::models::FeedTier::Simulcast,
                priority: 2,
                orientation_vertical: false,
                enabled: true,
                target_lufs: None,
                lufs_tolerance: None,
                broadcast_delay_secs: 600,
                exclude_from_accounting: false,
                live_streams_only: true,
            },
        ];

        let samples = vec![
            GridSample {
                bucket_utc: m_start,
                channel_id: "qc-unlisted".into(),
                group_id: "g".into(),
                platform: Platform::Custom,
                ccv: Some(999),
                ccv_status: MetricStatus::Value,
                source: MetricSource::AutoViewer,
                views: None,
                likes: None,
                chat_msgs: None,
                shares: None,
                gifts: None,
                coverage_pct: 100.0,
                extras_json: HashMap::new(),
            },
            GridSample {
                // Recorded 10 min after live climax due to +600s broadcast delay
                bucket_utc: Utc.with_ymd_and_hms(2026, 11, 25, 18, 38, 0).unwrap(),
                channel_id: "pub-delayed".into(),
                group_id: "g".into(),
                platform: Platform::Youtube,
                ccv: Some(5400),
                ccv_status: MetricStatus::Value,
                source: MetricSource::AutoViewer,
                views: None,
                likes: None,
                chat_msgs: None,
                shares: None,
                gifts: None,
                coverage_pct: 100.0,
                extras_json: HashMap::new(),
            },
        ];

        let aligned = prepare_accounting_samples(&samples, &channels);
        assert_eq!(aligned.len(), 1);
        assert_eq!(
            aligned[0].bucket_utc,
            Utc.with_ymd_and_hms(2026, 11, 25, 18, 28, 0).unwrap()
        );

        let slices = slice_matches(
            &aligned,
            &[MatchMarker {
                id: "m1".into(),
                show_id: "s1".into(),
                label: "Segment 1".into(),
                started_at: m_start,
                ended_at: Some(m_end),
            }],
        );
        assert_eq!(slices[0].pcv, 5400);
    }
}
