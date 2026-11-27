use crate::grid::{
    apply_overrides_to_samples, filter_samples_by_window, prepare_accounting_samples,
    slice_matches, summarize_grid,
};
use crate::models::{
    AlertState, ChannelConfig, ChannelRuntimeSnapshot, FeedTier, GridSample, IncidentRecord,
    ManualOverride, MatchMarker, MetricSource, MetricStatus, ShowPreset,
};
use chrono::{DateTime, Local, Utc};
use rust_xlsxwriter::{Color, Format, FormatBorder, Workbook};
use std::collections::{HashMap, HashSet};

fn format_local_dt(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

fn format_local_hm(dt: DateTime<Utc>) -> String {
    dt.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

fn local_tz_label() -> String {
    let offset = Local::now().format("%:z").to_string();
    format!("Local Time (UTC{offset})")
}

fn tier_label(tier: FeedTier) -> &'static str {
    match tier {
        FeedTier::Master => "Primary (HD)",
        FeedTier::Simulcast => "Standard (SD)",
        FeedTier::Light => "Audio-Only",
    }
}

fn status_label(status: MetricStatus) -> &'static str {
    match status {
        MetricStatus::Value => "Live",
        MetricStatus::Unavailable => "Unavailable",
        MetricStatus::Na => "Not Supported",
    }
}

fn severity_label(state: AlertState) -> &'static str {
    match state {
        AlertState::Critical => "Critical",
        AlertState::Warning => "Warning",
        AlertState::Acknowledged => "Acknowledged",
        AlertState::AdBreak => "Ad Break",
        AlertState::Offline => "Offline",
        AlertState::LoginGated => "Login Required",
        AlertState::ExtractorErr => "Stream Error",
        AlertState::Reacquiring => "Reconnecting",
        AlertState::Healthy => "Normal",
    }
}

pub fn build_accounting_workbook(
    show: &ShowPreset,
    channels: &[ChannelRuntimeSnapshot],
    samples: &[GridSample],
    matches: &[MatchMarker],
    incidents: &[IncidentRecord],
    overrides: &[ManualOverride],
) -> anyhow::Result<Vec<u8>> {
    let channel_cfgs: Vec<ChannelConfig> = channels.iter().map(|c| c.config.clone()).collect();
    let effective_samples = filter_samples_by_window(
        prepare_accounting_samples(&apply_overrides_to_samples(samples, overrides), &channel_cfgs),
        show.scheduled_start_at,
        show.scheduled_end_at,
    );
    let summary = summarize_grid(&effective_samples);
    let match_slices = slice_matches(&effective_samples, matches);
    let tz_label = local_tz_label();
    let group_names: HashMap<&str, &str> = show
        .groups
        .iter()
        .map(|g| (g.id.as_str(), g.name.as_str()))
        .collect();

    let mut wb = Workbook::new();
    let header_fmt = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0x19191B))
        .set_font_color(Color::RGB(0xEDEDED));
    let title_fmt = Format::new().set_bold().set_font_size(14);
    let num_fmt = Format::new().set_num_format("#,##0");
    let dec_fmt = Format::new().set_num_format("#,##0.00");

    // Tab 1: Executive Summary
    {
        let ws = wb.add_worksheet().set_name("Executive Summary")?;
        ws.write_string_with_format(0, 0, format!("Broadcast Viewership Report — {}", show.title), &title_fmt)?;
        ws.write_string(1, 0, format!("Timezone: {tz_label}"))?;
        if show.scheduled_start_at.is_some() || show.scheduled_end_at.is_some() {
            let start_s = show
                .scheduled_start_at
                .map(format_local_hm)
                .unwrap_or_else(|| "Open".to_string());
            let end_s = show
                .scheduled_end_at
                .map(format_local_hm)
                .unwrap_or_else(|| "Open".to_string());
            ws.write_string(2, 0, format!("Broadcast Window: {start_s} to {end_s}"))?;
        }

        ws.write_string_with_format(4, 0, "Summary Metric", &header_fmt)?;
        ws.write_string_with_format(4, 1, "Value", &header_fmt)?;

        ws.write_string(5, 0, "Peak Concurrent Viewers (All Channels Combined)")?;
        ws.write_number_with_format(5, 1, summary.combined_pcv as f64, &num_fmt)?;
        ws.write_string(6, 0, "Average Concurrent Viewers")?;
        ws.write_number_with_format(6, 1, summary.combined_acv, &dec_fmt)?;
        ws.write_string(7, 0, "Total Hours Watched")?;
        ws.write_number_with_format(7, 1, summary.hours_watched, &dec_fmt)?;
        ws.write_string(8, 0, "Broadcast Duration (Minutes)")?;
        ws.write_number_with_format(8, 1, summary.total_minutes as f64, &num_fmt)?;

        ws.write_string_with_format(10, 0, "Stream Group", &header_fmt)?;
        ws.write_string_with_format(10, 1, "Language / Region", &header_fmt)?;
        ws.write_string_with_format(10, 2, "Peak Viewers", &header_fmt)?;
        ws.write_string_with_format(10, 3, "Average Viewers", &header_fmt)?;
        ws.write_string_with_format(10, 4, "Hours Watched", &header_fmt)?;

        for (idx, grp) in show.groups.iter().enumerate() {
            let row = (11 + idx) as u32;
            ws.write_string(row, 0, &grp.name)?;
            ws.write_string(row, 1, &grp.language_code)?;
            ws.write_number_with_format(
                row,
                2,
                *summary.group_pcv.get(&grp.id).unwrap_or(&0) as f64,
                &num_fmt,
            )?;
            ws.write_number_with_format(
                row,
                3,
                *summary.group_acv.get(&grp.id).unwrap_or(&0.0),
                &dec_fmt,
            )?;
            ws.write_number_with_format(
                row,
                4,
                *summary.group_hours_watched.get(&grp.id).unwrap_or(&0.0),
                &dec_fmt,
            )?;
        }
        ws.autofit();
    }

    // Tab 2: Segment Breakdown
    {
        let ws = wb.add_worksheet().set_name("Segment Breakdown")?;
        let headers = [
            "Segment",
            "Start Time (Local)",
            "End Time (Local)",
            "Duration (Minutes)",
            "Peak Viewers",
            "Average Viewers",
            "Hours Watched",
        ];
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &header_fmt)?;
        }
        for (idx, ms) in match_slices.iter().enumerate() {
            let r = (idx + 1) as u32;
            ws.write_string(r, 0, &ms.match_marker.label)?;
            ws.write_string(r, 1, format_local_dt(ms.match_marker.started_at))?;
            ws.write_string(
                r,
                2,
                ms.match_marker
                    .ended_at
                    .map(format_local_dt)
                    .unwrap_or_else(|| "Ongoing".to_string()),
            )?;
            ws.write_number_with_format(r, 3, ms.duration_minutes as f64, &num_fmt)?;
            ws.write_number_with_format(r, 4, ms.pcv as f64, &num_fmt)?;
            ws.write_number_with_format(r, 5, ms.acv, &dec_fmt)?;
            ws.write_number_with_format(r, 6, ms.hours_watched, &dec_fmt)?;
        }
        ws.autofit();
    }

    // Tab 3: Channel Breakdown (grouped by Category with banners, subtotals, and spacer rows)
    {
        let ws = wb.add_worksheet().set_name("Channel Breakdown")?;
        let headers = [
            "Channel Name",
            "Category",
            "Platform",
            "Monitoring Mode",
            "Viewer Data Status",
            "Latest Viewers",
            "Peak Viewers",
            "Data Coverage (%)",
            "Data Source",
            "Total Views",
            "Likes",
            "Shares",
            "Gifts",
        ];
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &header_fmt)?;
        }

        let category_fmt = Format::new()
            .set_bold()
            .set_background_color(Color::RGB(0x2B303B))
            .set_font_color(Color::RGB(0xF2F4F8))
            .set_border_top(FormatBorder::Medium);
        let subtotal_label_fmt = Format::new()
            .set_bold()
            .set_background_color(Color::RGB(0xE8ECF2))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);
        let subtotal_num_fmt = Format::new()
            .set_bold()
            .set_num_format("#,##0")
            .set_background_color(Color::RGB(0xE8ECF2))
            .set_border_top(FormatBorder::Thin)
            .set_border_bottom(FormatBorder::Double);
        let grand_label_fmt = Format::new()
            .set_bold()
            .set_background_color(Color::RGB(0x19191B))
            .set_font_color(Color::RGB(0xEDEDED));
        let grand_num_fmt = Format::new()
            .set_bold()
            .set_num_format("#,##0")
            .set_background_color(Color::RGB(0x19191B))
            .set_font_color(Color::RGB(0xEDEDED));

        let mut ch_peak: HashMap<String, i64> = HashMap::new();
        let mut ch_source: HashMap<String, MetricSource> = HashMap::new();
        for s in &effective_samples {
            if let Some(v) = s.ccv {
                let p = ch_peak.entry(s.channel_id.clone()).or_insert(0);
                *p = (*p).max(v);
            }
            ch_source.insert(s.channel_id.clone(), s.source);
        }

        // Group channels in show.groups order, followed by any ungrouped channels.
        let mut grouped: Vec<(Option<&str>, String, Vec<&ChannelRuntimeSnapshot>)> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for g in &show.groups {
            let items: Vec<&ChannelRuntimeSnapshot> = channels
                .iter()
                .filter(|c| c.config.group_id == g.id)
                .collect();
            if !items.is_empty() {
                for ch in &items {
                    seen.insert(ch.config.id.as_str());
                }
                grouped.push((
                    Some(g.id.as_str()),
                    format!("{} — {}", g.language_code, g.name),
                    items,
                ));
            }
        }
        let ungrouped: Vec<&ChannelRuntimeSnapshot> = channels
            .iter()
            .filter(|c| !seen.contains(c.config.id.as_str()))
            .collect();
        if !ungrouped.is_empty() {
            grouped.push((None, "Ungrouped".to_string(), ungrouped));
        }

        let mut row: u32 = 1;
        let mut grand_latest = 0.0f64;
        let mut grand_views = 0.0f64;
        let mut grand_likes = 0.0f64;
        let mut grand_shares = 0.0f64;
        let mut grand_gifts = 0.0f64;

        for (grp_idx, (grp_id, grp_title, items)) in grouped.iter().enumerate() {
            if grp_idx > 0 {
                // Blank spacer row for clear visual separation between categories
                row += 1;
            }
            let banner = format!(
                "{} ({} {})",
                grp_title,
                items.len(),
                if items.len() == 1 { "channel" } else { "channels" }
            );
            ws.merge_range(row, 0, row, (headers.len() - 1) as u16, &banner, &category_fmt)?;
            row += 1;

            let mut cat_latest = 0.0f64;
            let mut cat_views = 0.0f64;
            let mut cat_likes = 0.0f64;
            let mut cat_shares = 0.0f64;
            let mut cat_gifts = 0.0f64;

            for ch in items {
                let grp_label = group_names
                    .get(ch.config.group_id.as_str())
                    .copied()
                    .unwrap_or(ch.config.group_id.as_str());
                ws.write_string(row, 0, &ch.config.name)?;
                ws.write_string(row, 1, grp_label)?;
                ws.write_string(row, 2, ch.config.platform.display_name())?;
                ws.write_string(row, 3, tier_label(ch.effective_tier))?;
                ws.write_string(row, 4, status_label(ch.ccv.status))?;
                match ch.ccv.value {
                    Some(v) if ch.ccv.status == MetricStatus::Value => {
                        ws.write_number_with_format(row, 5, v, &num_fmt)?;
                        if !ch.config.exclude_from_accounting {
                            cat_latest += v;
                        }
                    }
                    _ => {
                        ws.write_string(
                            row,
                            5,
                            match ch.ccv.status {
                                MetricStatus::Na => "Not Supported",
                                MetricStatus::Unavailable => "Unavailable",
                                MetricStatus::Value => "0",
                            },
                        )?;
                    }
                }
                ws.write_number_with_format(
                    row,
                    6,
                    *ch_peak.get(&ch.config.id).unwrap_or(&0) as f64,
                    &num_fmt,
                )?;
                ws.write_number_with_format(row, 7, ch.ccv.coverage_pct as f64, &dec_fmt)?;
                let src = ch_source
                    .get(&ch.config.id)
                    .copied()
                    .unwrap_or(ch.ccv.source);
                ws.write_string(
                    row,
                    8,
                    if ch.config.exclude_from_accounting {
                        "Excluded (Internal QC)"
                    } else {
                        match src {
                            MetricSource::AutoViewer => "Automatic",
                            MetricSource::ManualStudio => "Manual Override",
                        }
                    },
                )?;
                write_tristate_cell(ws, row, 9, &ch.total_views, &num_fmt)?;
                write_tristate_cell(ws, row, 10, &ch.likes, &num_fmt)?;
                write_tristate_cell(ws, row, 11, &ch.shares, &num_fmt)?;
                write_tristate_cell(ws, row, 12, &ch.gifts_count, &num_fmt)?;

                if !ch.config.exclude_from_accounting {
                    if ch.total_views.status == MetricStatus::Value {
                        cat_views += ch.total_views.value.unwrap_or(0.0);
                    }
                    if ch.likes.status == MetricStatus::Value {
                        cat_likes += ch.likes.value.unwrap_or(0.0);
                    }
                    if ch.shares.status == MetricStatus::Value {
                        cat_shares += ch.shares.value.unwrap_or(0.0);
                    }
                    if ch.gifts_count.status == MetricStatus::Value {
                        cat_gifts += ch.gifts_count.value.unwrap_or(0.0);
                    }
                }
                row += 1;
            }

            // Category Subtotal Row
            let cat_peak = match grp_id {
                Some(id) => summary.group_pcv.get(*id).copied().unwrap_or(0) as f64,
                None => items
                    .iter()
                    .filter(|c| !c.config.exclude_from_accounting)
                    .map(|c| *ch_peak.get(&c.config.id).unwrap_or(&0) as f64)
                    .sum(),
            };
            ws.write_string_with_format(row, 0, format!("Subtotal — {grp_title}"), &subtotal_label_fmt)?;
            for col in [1u16, 2, 3, 4, 7, 8] {
                ws.write_string_with_format(row, col, "", &subtotal_label_fmt)?;
            }
            ws.write_number_with_format(row, 5, cat_latest, &subtotal_num_fmt)?;
            ws.write_number_with_format(row, 6, cat_peak, &subtotal_num_fmt)?;
            ws.write_number_with_format(row, 9, cat_views, &subtotal_num_fmt)?;
            ws.write_number_with_format(row, 10, cat_likes, &subtotal_num_fmt)?;
            ws.write_number_with_format(row, 11, cat_shares, &subtotal_num_fmt)?;
            ws.write_number_with_format(row, 12, cat_gifts, &subtotal_num_fmt)?;
            row += 1;

            grand_latest += cat_latest;
            grand_views += cat_views;
            grand_likes += cat_likes;
            grand_shares += cat_shares;
            grand_gifts += cat_gifts;
        }

        if !grouped.is_empty() {
            row += 1;
            ws.write_string_with_format(row, 0, "Grand Total (All Included Channels)", &grand_label_fmt)?;
            for col in [1u16, 2, 3, 4, 7, 8] {
                ws.write_string_with_format(row, col, "", &grand_label_fmt)?;
            }
            ws.write_number_with_format(row, 5, grand_latest, &grand_num_fmt)?;
            ws.write_number_with_format(row, 6, summary.combined_pcv as f64, &grand_num_fmt)?;
            ws.write_number_with_format(row, 9, grand_views, &grand_num_fmt)?;
            ws.write_number_with_format(row, 10, grand_likes, &grand_num_fmt)?;
            ws.write_number_with_format(row, 11, grand_shares, &grand_num_fmt)?;
            ws.write_number_with_format(row, 12, grand_gifts, &grand_num_fmt)?;
        }

        ws.autofit();
    }

    // Tab 4: Incident Log
    {
        let ws = wb.add_worksheet().set_name("Incident Log")?;
        let headers = [
            "Channel",
            "Group",
            "Issue",
            "Severity",
            "Started (Local)",
            "Resolved (Local)",
            "Duration (Seconds)",
            "Acknowledged",
            "Evidence Clip",
            "Details",
        ];
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &header_fmt)?;
        }
        for (idx, inc) in incidents.iter().enumerate() {
            let r = (idx + 1) as u32;
            let grp_label = group_names
                .get(inc.group_id.as_str())
                .copied()
                .unwrap_or(inc.group_id.as_str());
            ws.write_string(r, 0, &inc.channel_name)?;
            ws.write_string(r, 1, grp_label)?;
            ws.write_string(r, 2, inc.fault.label())?;
            ws.write_string(r, 3, severity_label(inc.severity))?;
            ws.write_string(r, 4, format_local_dt(inc.started_at))?;
            ws.write_string(
                r,
                5,
                inc.resolved_at
                    .map(format_local_dt)
                    .unwrap_or_else(|| "Active".to_string()),
            )?;
            ws.write_number_with_format(r, 6, inc.duration_secs, &dec_fmt)?;
            ws.write_string(r, 7, if inc.acknowledged { "Yes" } else { "No" })?;
            ws.write_string(r, 8, inc.clip_filename.as_deref().unwrap_or(""))?;
            ws.write_string(r, 9, &inc.detail)?;
        }
        ws.autofit();
    }

    Ok(wb.save_to_buffer()?)
}

fn write_tristate_cell(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col: u16,
    metric: &crate::models::TriStateMetric,
    fmt: &Format,
) -> anyhow::Result<()> {
    match metric.status {
        MetricStatus::Value => {
            ws.write_number_with_format(row, col, metric.value.unwrap_or(0.0), fmt)?;
        }
        MetricStatus::Na => {
            ws.write_string(row, col, "Not Supported")?;
        }
        MetricStatus::Unavailable => {
            ws.write_string(row, col, "Unavailable")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AudioTelemetry, LanguageGroupConfig, Platform, TriStateMetric, VideoTelemetry,
    };

    #[test]
    fn builds_grouped_channel_breakdown_workbook_across_categories() {
        let show = ShowPreset {
            show_id: "show-1".into(),
            title: "Test Broadcast".into(),
            created_at: Utc::now(),
            scheduled_start_at: None,
            scheduled_end_at: None,
            groups: vec![
                LanguageGroupConfig {
                    id: "grp-id".into(),
                    name: "Indonesian Main".into(),
                    language_code: "ID".into(),
                    intermission_active: false,
                    complaint_keywords: vec![],
                },
                LanguageGroupConfig {
                    id: "grp-en".into(),
                    name: "English Main".into(),
                    language_code: "EN".into(),
                    intermission_active: false,
                    complaint_keywords: vec![],
                },
            ],
            channels: vec![],
        };

        let mk_ch = |id: &str, grp: &str, name: &str, ccv: f64, excluded: bool| {
            ChannelRuntimeSnapshot {
                config: ChannelConfig {
                    id: id.into(),
                    group_id: grp.into(),
                    name: name.into(),
                    url: format!("https://youtube.com/watch?v={id}"),
                    platform: Platform::Youtube,
                    tier: FeedTier::Master,
                    priority: 1,
                    orientation_vertical: false,
                    enabled: true,
                    target_lufs: None,
                    lufs_tolerance: None,
                    broadcast_delay_secs: 0,
                    exclude_from_accounting: excluded,
                    live_streams_only: true,
                },
                effective_tier: FeedTier::Master,
                alert_state: AlertState::Healthy,
                active_faults: vec![],
                last_status_reason: None,
                audio: AudioTelemetry::default(),
                video: VideoTelemetry::default(),
                ccv: TriStateMetric::value(ccv, 100.0),
                total_views: TriStateMetric::value(ccv * 10.0, 100.0),
                likes: TriStateMetric::value(100.0, 100.0),
                chat_rate_per_min: TriStateMetric::na(),
                shares: TriStateMetric::na(),
                gifts_count: TriStateMetric::na(),
                uptime_pct: 100.0,
                thumbnail_b64: None,
                thumbnail_updated_at: None,
                pfl_active: false,
                video_shed_resource: false,
                resolved_via: Some("yt-dlp".into()),
                effective_target_lufs: -14.0,
                effective_lufs_tolerance: 2.5,
                updated_at: Utc::now(),
            }
        };

        let channels = vec![
            mk_ch("ch-1", "grp-id", "ID YouTube", 1200.0, false),
            mk_ch("ch-2", "grp-id", "ID Internal QC", 50.0, true),
            mk_ch("ch-3", "grp-en", "EN YouTube", 800.0, false),
        ];

        let bytes = build_accounting_workbook(&show, &channels, &[], &[], &[], &[])
            .expect("workbook build should succeed");
        // Valid ZIP/XLSX magic header ("PK")
        assert!(bytes.len() > 2000);
        assert_eq!(&bytes[0..2], b"PK");
    }
}
