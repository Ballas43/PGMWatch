use crate::grid::{
    apply_overrides_to_samples, filter_samples_by_window, prepare_accounting_samples,
    slice_matches, summarize_grid,
};
use crate::models::{
    AlertState, ChannelConfig, ChannelRuntimeSnapshot, FeedTier, GridSample, IncidentRecord,
    ManualOverride, MatchMarker, MetricSource, MetricStatus, ShowPreset,
};
use chrono::{DateTime, Local, Utc};
use rust_xlsxwriter::{Color, Format, Workbook};
use std::collections::HashMap;

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
        MetricStatus::Na => "Not Tracked",
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

    // Tab 3: Channel Breakdown
    {
        let ws = wb.add_worksheet().set_name("Channel Breakdown")?;
        let headers = [
            "Channel Name",
            "Group",
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

        let mut ch_peak: HashMap<String, i64> = HashMap::new();
        let mut ch_source: HashMap<String, MetricSource> = HashMap::new();
        for s in &effective_samples {
            if let Some(v) = s.ccv {
                let p = ch_peak.entry(s.channel_id.clone()).or_insert(0);
                *p = (*p).max(v);
            }
            ch_source.insert(s.channel_id.clone(), s.source);
        }

        for (idx, ch) in channels.iter().enumerate() {
            let r = (idx + 1) as u32;
            let grp_label = group_names
                .get(ch.config.group_id.as_str())
                .copied()
                .unwrap_or(ch.config.group_id.as_str());
            ws.write_string(r, 0, &ch.config.name)?;
            ws.write_string(r, 1, grp_label)?;
            ws.write_string(r, 2, ch.config.platform.display_name())?;
            ws.write_string(r, 3, tier_label(ch.effective_tier))?;
            ws.write_string(r, 4, status_label(ch.ccv.status))?;
            match ch.ccv.value {
                Some(v) => ws.write_number_with_format(r, 5, v, &num_fmt)?,
                None => ws.write_string(
                    r,
                    5,
                    match ch.ccv.status {
                        MetricStatus::Na => "Not Tracked",
                        MetricStatus::Unavailable => "Unavailable",
                        MetricStatus::Value => "0",
                    },
                )?,
            };
            ws.write_number_with_format(
                r,
                6,
                *ch_peak.get(&ch.config.id).unwrap_or(&0) as f64,
                &num_fmt,
            )?;
            ws.write_number_with_format(r, 7, ch.ccv.coverage_pct as f64, &dec_fmt)?;
            let src = ch_source
                .get(&ch.config.id)
                .copied()
                .unwrap_or(ch.ccv.source);
            ws.write_string(
                r,
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
            write_tristate_cell(ws, r, 9, &ch.total_views, &num_fmt)?;
            write_tristate_cell(ws, r, 10, &ch.likes, &num_fmt)?;
            write_tristate_cell(ws, r, 11, &ch.shares, &num_fmt)?;
            write_tristate_cell(ws, r, 12, &ch.gifts_count, &num_fmt)?;
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
            ws.write_string(row, col, "Not Tracked")?;
        }
        MetricStatus::Unavailable => {
            ws.write_string(row, col, "Unavailable")?;
        }
    }
    Ok(())
}
