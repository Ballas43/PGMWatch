use crate::grid::{
    apply_overrides_to_samples, filter_samples_by_window, prepare_accounting_samples,
    slice_matches, summarize_grid,
};
use crate::models::{
    ChannelConfig, ChannelRuntimeSnapshot, GridSample, IncidentRecord, ManualOverride,
    MatchMarker, MetricSource, MetricStatus, ShowPreset,
};
use rust_xlsxwriter::{Color, Format, Workbook};
use std::collections::HashMap;

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
        ws.write_string_with_format(0, 0, format!("PGMWatch Broadcast Accounting — {}", show.title), &title_fmt)?;
        ws.write_string(1, 0, format!("Show ID: {}", show.show_id))?;
        if show.scheduled_start_at.is_some() || show.scheduled_end_at.is_some() {
            let start_s = show
                .scheduled_start_at
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "Open".to_string());
            let end_s = show
                .scheduled_end_at
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "Open".to_string());
            ws.write_string(2, 0, format!("Broadcast Window: {start_s} -> {end_s}"))?;
        }

        ws.write_string_with_format(3, 0, "Global Metric", &header_fmt)?;
        ws.write_string_with_format(3, 1, "Value", &header_fmt)?;

        ws.write_string(4, 0, "Combined Global Peak Concurrent Viewers (PCV)")?;
        ws.write_number_with_format(4, 1, summary.combined_pcv as f64, &num_fmt)?;
        ws.write_string(5, 0, "Combined Average Concurrent Viewers (ACV)")?;
        ws.write_number_with_format(5, 1, summary.combined_acv, &dec_fmt)?;
        ws.write_string(6, 0, "Total Hours Watched (60s UTC Grid)")?;
        ws.write_number_with_format(6, 1, summary.hours_watched, &dec_fmt)?;
        ws.write_string(7, 0, "Tracked Grid Minutes")?;
        ws.write_number_with_format(7, 1, summary.total_minutes as f64, &num_fmt)?;

        ws.write_string_with_format(9, 0, "Stream Group", &header_fmt)?;
        ws.write_string_with_format(9, 1, "Group Tag", &header_fmt)?;
        ws.write_string_with_format(9, 2, "Group PCV", &header_fmt)?;
        ws.write_string_with_format(9, 3, "Group ACV", &header_fmt)?;
        ws.write_string_with_format(9, 4, "Hours Watched", &header_fmt)?;

        for (idx, grp) in show.groups.iter().enumerate() {
            let row = (10 + idx) as u32;
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
            "Segment ID",
            "Segment Label",
            "Started UTC",
            "Ended UTC",
            "Minutes",
            "Segment PCV",
            "Segment ACV",
            "Hours Watched",
        ];
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &header_fmt)?;
        }
        for (idx, ms) in match_slices.iter().enumerate() {
            let r = (idx + 1) as u32;
            ws.write_string(r, 0, &ms.match_marker.id)?;
            ws.write_string(r, 1, &ms.match_marker.label)?;
            ws.write_string(r, 2, ms.match_marker.started_at.to_rfc3339())?;
            ws.write_string(
                r,
                3,
                ms.match_marker
                    .ended_at
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "LIVE".to_string()),
            )?;
            ws.write_number_with_format(r, 4, ms.duration_minutes as f64, &num_fmt)?;
            ws.write_number_with_format(r, 5, ms.pcv as f64, &num_fmt)?;
            ws.write_number_with_format(r, 6, ms.acv, &dec_fmt)?;
            ws.write_number_with_format(r, 7, ms.hours_watched, &dec_fmt)?;
        }
        ws.autofit();
    }

    // Tab 3: Platform Accounting
    {
        let ws = wb.add_worksheet().set_name("Platform Accounting")?;
        let headers = [
            "Channel ID",
            "Channel Name",
            "Group ID",
            "Platform",
            "Tier",
            "CCV Status",
            "Current / Override CCV",
            "Peak CCV",
            "Coverage %",
            "Source",
            "Total Views",
            "Likes",
            "Shares",
            "Gifts",
            "Extras JSON",
        ];
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &header_fmt)?;
        }

        let mut ch_peak: HashMap<String, i64> = HashMap::new();
        let mut ch_source: HashMap<String, MetricSource> = HashMap::new();
        let mut ch_extras: HashMap<String, String> = HashMap::new();
        for s in &effective_samples {
            if let Some(v) = s.ccv {
                let p = ch_peak.entry(s.channel_id.clone()).or_insert(0);
                *p = (*p).max(v);
            }
            ch_source.insert(s.channel_id.clone(), s.source);
            if !s.extras_json.is_empty()
                && let Ok(json) = serde_json::to_string(&s.extras_json)
            {
                ch_extras.insert(s.channel_id.clone(), json);
            }
        }

        for (idx, ch) in channels.iter().enumerate() {
            let r = (idx + 1) as u32;
            ws.write_string(r, 0, &ch.config.id)?;
            ws.write_string(r, 1, &ch.config.name)?;
            ws.write_string(r, 2, &ch.config.group_id)?;
            ws.write_string(r, 3, ch.config.platform.display_name())?;
            ws.write_string(r, 4, ch.effective_tier.as_str())?;
            ws.write_string(r, 5, format!("{:?}", ch.ccv.status).to_uppercase())?;
            match ch.ccv.value {
                Some(v) => ws.write_number_with_format(r, 6, v, &num_fmt)?,
                None => ws.write_string(
                    r,
                    6,
                    match ch.ccv.status {
                        MetricStatus::Na => "N/A",
                        MetricStatus::Unavailable => "UNAVAILABLE",
                        MetricStatus::Value => "0",
                    },
                )?,
            };
            ws.write_number_with_format(
                r,
                7,
                *ch_peak.get(&ch.config.id).unwrap_or(&0) as f64,
                &num_fmt,
            )?;
            ws.write_number_with_format(r, 8, ch.ccv.coverage_pct as f64, &dec_fmt)?;
            let src = ch_source
                .get(&ch.config.id)
                .copied()
                .unwrap_or(ch.ccv.source);
            ws.write_string(
                r,
                9,
                if ch.config.exclude_from_accounting {
                    "EXCLUDED_INTERNAL_QC"
                } else {
                    match src {
                        MetricSource::AutoViewer => "AUTO_VIEWER",
                        MetricSource::ManualStudio => "MANUAL_STUDIO",
                    }
                },
            )?;
            write_tristate_cell(ws, r, 10, &ch.total_views, &num_fmt)?;
            write_tristate_cell(ws, r, 11, &ch.likes, &num_fmt)?;
            write_tristate_cell(ws, r, 12, &ch.shares, &num_fmt)?;
            write_tristate_cell(ws, r, 13, &ch.gifts_count, &num_fmt)?;
            ws.write_string(
                r,
                14,
                ch_extras.get(&ch.config.id).map(String::as_str).unwrap_or("{}"),
            )?;
        }
        ws.autofit();
    }

    // Tab 4: Health & Incident Log
    {
        let ws = wb.add_worksheet().set_name("Health & Incident Log")?;
        let headers = [
            "Incident ID",
            "Channel",
            "Group",
            "Fault Type",
            "Severity",
            "Started UTC",
            "Resolved UTC",
            "Duration (s)",
            "Acknowledged",
            "Evidence Clip",
            "Detail",
        ];
        for (c, h) in headers.iter().enumerate() {
            ws.write_string_with_format(0, c as u16, *h, &header_fmt)?;
        }
        for (idx, inc) in incidents.iter().enumerate() {
            let r = (idx + 1) as u32;
            ws.write_string(r, 0, &inc.id)?;
            ws.write_string(r, 1, &inc.channel_name)?;
            ws.write_string(r, 2, &inc.group_id)?;
            ws.write_string(r, 3, inc.fault.label())?;
            ws.write_string(r, 4, format!("{:?}", inc.severity).to_uppercase())?;
            ws.write_string(r, 5, inc.started_at.to_rfc3339())?;
            ws.write_string(
                r,
                6,
                inc.resolved_at
                    .map(|d| d.to_rfc3339())
                    .unwrap_or_else(|| "ACTIVE".to_string()),
            )?;
            ws.write_number_with_format(r, 7, inc.duration_secs, &dec_fmt)?;
            ws.write_string(r, 8, if inc.acknowledged { "YES" } else { "NO" })?;
            ws.write_string(
                r,
                9,
                inc.clip_filename.as_deref().unwrap_or(""),
            )?;
            ws.write_string(r, 10, &inc.detail)?;
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
            ws.write_string(row, col, "N/A")?;
        }
        MetricStatus::Unavailable => {
            ws.write_string(row, col, "UNAVAILABLE")?;
        }
    }
    Ok(())
}
