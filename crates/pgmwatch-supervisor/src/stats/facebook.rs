use super::{record_http_rx_bytes, unavailable_stats, ChannelStatsUpdate};
use pgmwatch_core::models::TriStateMetric;
use std::collections::HashMap;

pub(super) fn extract_json_field_number(haystack: &str, key: &str) -> Option<f64> {
    for pattern in [format!("\"{key}\""), format!("\\\"{key}\\\"")] {
        let mut offset = 0;
        while let Some(rel) = haystack[offset..].find(&pattern) {
            let after_key = offset + rel + pattern.len();
            let rest = &haystack[after_key..];
            let trimmed_ws = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
            if let Some(after_colon) = trimmed_ws.strip_prefix(':') {
                let val_part = after_colon.trim_start_matches(|c: char| c.is_ascii_whitespace());
                // Support both direct numeric values (`"reaction_count":33`) and nested
                // Facebook/GraphQL objects (`"reaction_count":{"count":33}`).
                let candidate = if let Some(inner) = val_part.strip_prefix('{') {
                    let window = &inner[..inner.floor_char_boundary(inner.len().min(96))];
                    if let Some(num) = extract_direct_count_in_object(window) {
                        return Some(num);
                    }
                    ""
                } else {
                    val_part.trim_start_matches('"')
                };
                let digits: String = candidate
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == ',')
                    .filter(|c| c.is_ascii_digit())
                    .collect();
                if !digits.is_empty()
                    && let Ok(num) = digits.parse::<f64>()
                {
                    return Some(num);
                }
            }
            offset = after_key;
        }
    }
    None
}

fn extract_direct_count_in_object(window: &str) -> Option<f64> {
    for key in ["\"count\"", "\\\"count\\\""] {
        if let Some(idx) = window.find(key) {
            let rest = window[idx + key.len()..].trim_start();
            if let Some(after_colon) = rest.strip_prefix(':') {
                let val_part = after_colon.trim_start().trim_start_matches('"');
                let digits: String = val_part
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == ',')
                    .filter(|c| c.is_ascii_digit())
                    .collect();
                if !digits.is_empty()
                    && let Ok(num) = digits.parse::<f64>()
                {
                    return Some(num);
                }
            }
        }
    }
    None
}

fn extract_facebook_video_id(url: &str) -> Option<&str> {
    let lower = url.to_ascii_lowercase();
    if lower.contains("/events/") {
        return None;
    }
    if let Some(pos) = lower.find("/videos/") {
        let rest = &url[pos + "/videos/".len()..];
        let digits = rest
            .split(['/', '?', '#', '&'])
            .find(|seg| seg.len() >= 6 && seg.chars().all(|c| c.is_ascii_digit()));
        if digits.is_some() {
            return digits;
        }
    }
    if let Some(qpos) = url.find('?') {
        for pair in url[qpos + 1..].split('&') {
            if let Some(val) = pair.strip_prefix("v=") {
                let clean = val.split('#').next().unwrap_or("");
                if clean.len() >= 6 && clean.chars().all(|c| c.is_ascii_digit()) {
                    return Some(clean);
                }
            }
        }
    }
    None
}

fn window_belongs_to_other_video(window: &str, target_video_id: &str) -> bool {
    for pat in ["\"id\":\"", "\\\"id\\\":\\\""] {
        let mut off = 0;
        while let Some(rel) = window[off..].find(pat) {
            let start = off + rel + pat.len();
            let rest = &window[start..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 8 && digits != target_video_id {
                return true;
            }
            off = start;
        }
    }
    false
}

fn extract_facebook_field_number(
    haystack: &str,
    key: &str,
    target_video_id: Option<&str>,
) -> Option<f64> {
    let Some(tid) = target_video_id else {
        return extract_json_field_number(haystack, key);
    };

    let mut fallback: Option<f64> = None;
    for pattern in [format!("\"{key}\""), format!("\\\"{key}\\\"")] {
        let mut offset = 0;
        while let Some(rel) = haystack[offset..].find(&pattern) {
            let abs_pos = offset + rel;
            let after_key = abs_pos + pattern.len();
            let rest = &haystack[after_key..];
            let trimmed_ws = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
            if let Some(after_colon) = trimmed_ws.strip_prefix(':') {
                let val_part = after_colon.trim_start_matches(|c: char| c.is_ascii_whitespace());
                let parsed = if let Some(inner) = val_part.strip_prefix('{') {
                    let obj_win = &inner[..inner.floor_char_boundary(inner.len().min(96))];
                    extract_direct_count_in_object(obj_win)
                } else {
                    let digits: String = val_part
                        .trim_start_matches('"')
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == ',')
                        .filter(|c| c.is_ascii_digit())
                        .collect();
                    if digits.is_empty() {
                        None
                    } else {
                        digits.parse::<f64>().ok()
                    }
                };
                if let Some(num) = parsed {
                    let w_start = haystack.floor_char_boundary(abs_pos.saturating_sub(800));
                    let w_end = haystack.ceil_char_boundary((after_key + 800).min(haystack.len()));
                    let ctx = &haystack[w_start..w_end];
                    if ctx.contains(tid) {
                        return Some(num);
                    }
                    let tight_start = haystack.floor_char_boundary(abs_pos.saturating_sub(250));
                    let tight_end = haystack.ceil_char_boundary((after_key + 350).min(haystack.len()));
                    let tight_ctx = &haystack[tight_start..tight_end];
                    if !window_belongs_to_other_video(tight_ctx, tid) && fallback.is_none() {
                        fallback = Some(num);
                    }
                }
            }
            offset = after_key;
        }
    }
    fallback
}

pub(super) fn parse_facebook_html_stats(channel_id: &str, url: &str, html: &str) -> Option<ChannelStatsUpdate> {
    let target_vid = extract_facebook_video_id(url);
    let ccv = [
        "liveViewerCount",
        "live_viewer_count_read_only",
        "live_viewer_count",
        "concurrent_view_count",
        "viewer_count",
    ]
    .iter()
    .find_map(|key| extract_facebook_field_number(html, key, target_vid))?;

    let total_views = ["play_count", "video_view_count", "views"]
        .iter()
        .find_map(|key| extract_facebook_field_number(html, key, target_vid))
        .map(|v| TriStateMetric::value(v, 90.0))
        .unwrap_or_else(TriStateMetric::na);

    let likes = ["reaction_count", "like_count", "likers_count"]
        .iter()
        .find_map(|key| extract_facebook_field_number(html, key, target_vid))
        .map(|v| TriStateMetric::value(v, 90.0))
        .unwrap_or_else(TriStateMetric::na);

    let shares = ["share_count"]
        .iter()
        .find_map(|key| extract_facebook_field_number(html, key, target_vid))
        .map(|v| TriStateMetric::value(v, 90.0))
        .unwrap_or_else(TriStateMetric::na);

    let mut extras = HashMap::new();
    if let Some(comments) = ["comment_count"]
        .iter()
        .find_map(|key| extract_facebook_field_number(html, key, target_vid))
    {
        extras.insert("comment_count".into(), serde_json::json!(comments.round() as i64));
    }

    Some(ChannelStatsUpdate {
        channel_id: channel_id.to_string(),
        ccv: TriStateMetric::value(ccv, 95.0),
        total_views,
        likes,
        chat_rate_per_min: TriStateMetric::na(),
        shares,
        gifts_count: TriStateMetric::na(),
        extras,
    })
}

async fn fetch_facebook_html_once(
    client: &reqwest::Client,
    url: &str,
    cookie_header: Option<&str>,
) -> Result<String, String> {
    let mut req = client
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .header("Sec-Fetch-Mode", "navigate");
    if let Some(cookie) = cookie_header.filter(|s| !s.is_empty()) {
        req = req.header("Cookie", cookie);
    }
    let resp = req
        .send()
        .await
        .map_err(|_| "Facebook stats request failed".to_string())?;

    if !resp.status().is_success() {
        return Err(format!(
            "Facebook public page returned HTTP {}",
            resp.status().as_u16()
        ));
    }

    let content_len = resp.content_length();
    let html = resp
        .text()
        .await
        .map_err(|_| "Facebook HTML read failed".to_string())?;
    record_http_rx_bytes(content_len, html.len());
    Ok(html)
}

pub(super) async fn fetch_facebook_public_stats(
    client: &reqwest::Client,
    channel_id: &str,
    url: &str,
    cookie_header: Option<&str>,
) -> ChannelStatsUpdate {
    let has_cookie = cookie_header.is_some_and(|s| !s.is_empty());
    // Facebook `/events/` pages embed `live_viewer_count_read_only` and `play_count` in anonymous
    // public SSR HTML, whereas logged-in Comet SSR often lazy-loads event video stats via JS.
    // Try the best order first, then fall back to the other mode when cookies are configured.
    let attempts: Vec<Option<&str>> = if has_cookie {
        if url.to_ascii_lowercase().contains("/events/") {
            vec![None, cookie_header]
        } else {
            vec![cookie_header, None]
        }
    } else {
        vec![None]
    };

    let mut last_err: Option<String> = None;
    for cookie_opt in attempts {
        match fetch_facebook_html_once(client, url, cookie_opt).await {
            Ok(html) => {
                if let Some(parsed) = parse_facebook_html_stats(channel_id, url, &html) {
                    return parsed;
                }
                last_err = Some(
                    "Facebook public page viewer count unavailable (login or private event)"
                        .to_string(),
                );
            }
            Err(e) => {
                last_err = Some(e);
            }
        }
    }

    unavailable_stats(
        channel_id,
        last_err
            .as_deref()
            .unwrap_or("Facebook public page viewer count unavailable (login or private event)"),
    )
}

pub(super) fn extract_json_number_near(haystack: &str, needle: &str) -> Option<f64> {
    let idx = haystack.find(needle)?;
    let rest = &haystack[idx + needle.len()..];
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<f64>().ok()
}
