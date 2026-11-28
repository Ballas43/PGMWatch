use chrono::{DateTime, Duration, Utc};
use pgmwatch_core::models::{MetricSource, MetricStatus, Platform, TriStateMetric};
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct ChannelStatsUpdate {
    #[allow(dead_code)]
    pub channel_id: String,
    pub ccv: TriStateMetric,
    pub total_views: TriStateMetric,
    pub likes: TriStateMetric,
    pub chat_rate_per_min: TriStateMetric,
    pub shares: TriStateMetric,
    pub gifts_count: TriStateMetric,
    pub extras: HashMap<String, serde_json::Value>,
}

pub struct ChatComplaintSensor {
    events: HashMap<String, VecDeque<(DateTime<Utc>, String)>>,
}

impl Default for ChatComplaintSensor {
    fn default() -> Self {
        Self::new()
    }
}
 
pub struct ChatComplaintRule<'a> {
    pub keywords: &'a [String],
    pub window_secs: u64,
    pub threshold_unique: usize,
}

pub struct PlatformStatsCaches<'a> {
    pub tiktok: &'a HashMap<String, ChannelStatsUpdate>,
    pub chat_rates: &'a HashMap<String, f64>,
    pub twitch_gifts: &'a HashMap<String, f64>,
    pub mode_b: &'a HashMap<String, ChannelStatsUpdate>,
    pub facebook_cookie: Option<&'a str>,
}

/// Reads a Netscape `cookies.txt` file and builds a `Cookie: k=v; ...` header for matching domain suffix.
pub fn load_cookie_header_for_domain(cookies_path: &Path, domain_suffix: &str) -> Option<String> {
    let content = std::fs::read_to_string(cookies_path).ok()?;
    let suffix = domain_suffix.trim_start_matches('.').to_ascii_lowercase();
    let mut pairs = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 7 {
            continue;
        }
        let dom = cols[0].trim_start_matches('.').to_ascii_lowercase();
        if dom == suffix || dom.ends_with(&format!(".{suffix}")) {
            let name = cols[5].trim();
            let val = cols[6].trim();
            if !name.is_empty() && !val.is_empty() {
                pairs.push(format!("{name}={val}"));
            }
        }
    }
    if pairs.is_empty() {
        None
    } else {
        Some(pairs.join("; "))
    }
}

/// Extracts a single named cookie value (e.g. `sessionid` for TikTok) from a Netscape `cookies.txt`.
pub fn extract_cookie_value(cookies_path: &Path, domain_suffix: &str, cookie_name: &str) -> Option<String> {
    let content = std::fs::read_to_string(cookies_path).ok()?;
    let suffix = domain_suffix.trim_start_matches('.').to_ascii_lowercase();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let cols: Vec<&str> = trimmed.split('\t').collect();
        if cols.len() < 7 {
            continue;
        }
        let dom = cols[0].trim_start_matches('.').to_ascii_lowercase();
        if (dom == suffix || dom.ends_with(&format!(".{suffix}")))
            && cols[5].trim().eq_ignore_ascii_case(cookie_name)
        {
            let val = cols[6].trim();
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

impl ChatComplaintSensor {
    pub fn new() -> Self {
        Self {
            events: HashMap::new(),
        }
    }

    pub fn ingest_message(
        &mut self,
        group_id: &str,
        viewer_id: &str,
        text: &str,
        rule: &ChatComplaintRule<'_>,
        now: DateTime<Utc>,
    ) -> bool {
        let lower = text.to_ascii_lowercase();
        let matched = rule
            .keywords
            .iter()
            .any(|kw| lower.contains(&kw.to_ascii_lowercase()));
        if !matched {
            return false;
        }

        let q = self.events.entry(group_id.to_string()).or_default();
        q.push_back((now, viewer_id.to_string()));
        let cutoff = now - Duration::seconds(rule.window_secs as i64);
        while let Some((ts, _)) = q.front() {
            if *ts < cutoff {
                q.pop_front();
            } else {
                break;
            }
        }

        let uniq: HashSet<&str> = q.iter().map(|(_, u)| u.as_str()).collect();
        uniq.len() >= rule.threshold_unique
    }
}

pub async fn collect_platform_stats(
    client: &reqwest::Client,
    channel_id: &str,
    url: &str,
    platform: Platform,
    tick: u64,
    caches: &PlatformStatsCaches<'_>,
) -> ChannelStatsUpdate {
    if url.starts_with("sim://") {
        return synthesize_sim_stats(channel_id, platform, tick);
    }

    let chat_rate = caches.chat_rates.get(channel_id).copied();
    let twitch_gift = caches.twitch_gifts.get(channel_id).copied();

    match platform {
        Platform::Youtube => fetch_youtube_public_stats(client, channel_id, url, chat_rate).await,
        Platform::Twitch => {
            fetch_twitch_public_stats(client, channel_id, url, chat_rate, twitch_gift).await
        }
        Platform::Facebook => {
            let pub_stats =
                fetch_facebook_public_stats(client, channel_id, url, caches.facebook_cookie).await;
            if pub_stats.ccv.status == MetricStatus::Value {
                pub_stats
            } else if let Some(cached) = caches.mode_b.get(channel_id) {
                cached.clone()
            } else {
                pub_stats
            }
        }
        Platform::Tiktok => {
            if let Some(cached) = caches
                .tiktok
                .get(channel_id)
                .filter(|u| u.ccv.status == MetricStatus::Value)
            {
                cached.clone()
            } else {
                let http_fb = fetch_tiktok_public_stats(client, channel_id, url).await;
                if http_fb.ccv.status == MetricStatus::Value {
                    http_fb
                } else {
                    caches.tiktok.get(channel_id).cloned().unwrap_or(http_fb)
                }
            }
        }
        Platform::Bigo | Platform::Custom => caches
            .mode_b
            .get(channel_id)
            .cloned()
            .unwrap_or_else(|| {
                unavailable_stats(
                    channel_id,
                    "Awaiting Ghost Browser Mode B DOM scrape or manual override",
                )
            }),
    }
}

fn synthesize_sim_stats(channel_id: &str, platform: Platform, tick: u64) -> ChannelStatsUpdate {
    let base: f64 = match platform {
        Platform::Youtube => 18_400.0,
        Platform::Twitch => 11_250.0,
        Platform::Tiktok => 6_800.0,
        Platform::Facebook => 4_100.0,
        Platform::Bigo => 2_200.0,
        Platform::Custom => 1_500.0,
    };
    let wave = ((tick as f64) * 0.17 + channel_id.len() as f64).sin() * 0.09;
    let ccv_val = (base * (1.0 + wave)).round().max(100.0);

    let mut extras = HashMap::new();
    extras.insert("mode".into(), serde_json::json!("sim_viewer"));

    ChannelStatsUpdate {
        channel_id: channel_id.to_string(),
        ccv: TriStateMetric {
            status: MetricStatus::Value,
            value: Some(ccv_val),
            coverage_pct: 100.0,
            source: MetricSource::AutoViewer,
            note: None,
        },
        total_views: if matches!(platform, Platform::Twitch) {
            TriStateMetric::na()
        } else {
            TriStateMetric::value(ccv_val * 4.2 + (tick as f64 * 45.0), 100.0)
        },
        likes: if matches!(platform, Platform::Twitch) {
            TriStateMetric::na()
        } else {
            TriStateMetric::value(ccv_val * 0.35 + (tick as f64 * 12.0), 96.0)
        },
        chat_rate_per_min: TriStateMetric::value((ccv_val * 0.018).round(), 92.0),
        shares: if matches!(platform, Platform::Tiktok | Platform::Facebook) {
            TriStateMetric::value((ccv_val * 0.04).round(), 90.0)
        } else {
            TriStateMetric::na()
        },
        gifts_count: if matches!(platform, Platform::Tiktok | Platform::Bigo) {
            TriStateMetric::value((tick as f64 * 3.0).round(), 94.0)
        } else {
            TriStateMetric::na()
        },
        extras,
    }
}

fn unavailable_stats(channel_id: &str, reason: &str) -> ChannelStatsUpdate {
    ChannelStatsUpdate {
        channel_id: channel_id.to_string(),
        ccv: TriStateMetric::unavailable(reason),
        total_views: TriStateMetric::na(),
        likes: TriStateMetric::na(),
        chat_rate_per_min: TriStateMetric::unavailable(reason),
        shares: TriStateMetric::na(),
        gifts_count: TriStateMetric::na(),
        extras: HashMap::new(),
    }
}

fn parse_youtube_html_likes(html: &str) -> Option<f64> {
    if let Some(v) = extract_json_number_near(html, "\"likeCount\":\"")
        .or_else(|| extract_json_field_number(html, "likeCount"))
    {
        return Some(v);
    }
    if let Some(v) = extract_json_number_near(html, "like this video along with ") {
        return Some(v);
    }
    if let Some(v) = extract_json_number_near(
        html,
        "\"defaultText\":{\"accessibility\":{\"accessibilityData\":{\"label\":\"",
    ) {
        return Some(v);
    }
    // When a YouTube livestream has 0 likes, YouTube renders the like button ViewModel /
    // `unset_like_count_entity_key` without a "like this video along with N other" label.
    if html.contains("\"likeCountEntity\"")
        || html.contains("\"likeButtonViewModel\"")
        || html.contains("\"segmentedLikeDislikeButtonViewModel\"")
    {
        return Some(0.0);
    }
    None
}

async fn fetch_youtube_public_stats(
    client: &reqwest::Client,
    channel_id: &str,
    url: &str,
    chat_rate: Option<f64>,
) -> ChannelStatsUpdate {
    let Ok(resp) = client
        .get(url)
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
    else {
        return unavailable_stats(channel_id, "YouTube page fetch failed");
    };
    let Ok(html) = resp.text().await else {
        return unavailable_stats(channel_id, "YouTube response unreadable");
    };

    let ccv = extract_json_number_near(&html, "\"concurrentViewers\":\"")
        .or_else(|| extract_json_number_near(&html, "\"viewCount\":{\"runs\":[{\"text\":\""));
    let views = extract_json_number_near(&html, "\"viewCount\":\"");
    let likes = parse_youtube_html_likes(&html);
    let has_live_chat = html.contains("\"liveChatRenderer\"");

    ChannelStatsUpdate {
        channel_id: channel_id.to_string(),
        ccv: match ccv {
            Some(v) => TriStateMetric::value(v, 100.0),
            None => TriStateMetric::unavailable("YouTube live CCV not exposed in page HTML"),
        },
        total_views: match views {
            Some(v) => TriStateMetric::value(v, 100.0),
            None => TriStateMetric::unavailable("YouTube views unavailable"),
        },
        likes: match likes {
            Some(v) => TriStateMetric::value(v, 100.0),
            None => TriStateMetric::unavailable("YouTube likes unavailable"),
        },
        chat_rate_per_min: match chat_rate {
            Some(v) => TriStateMetric::value(v, 100.0),
            None if has_live_chat => TriStateMetric::value(0.0, 100.0),
            None => TriStateMetric::na(),
        },
        shares: TriStateMetric::na(),
        gifts_count: TriStateMetric::na(),
        extras: HashMap::new(),
    }
}

fn parse_tiktok_public_ccv(html: &str) -> Option<f64> {
    for key in [
        "\"userCount\"",
        "\"user_count\"",
        "\"viewerCount\"",
        "\"viewer_count\"",
    ] {
        let mut rest = html;
        while let Some(idx) = rest.find(key) {
            let after_key = &rest[idx + key.len()..];
            let trimmed = after_key.trim_start();
            if let Some(after_colon) = trimmed.strip_prefix(':') {
                let num_start = after_colon.trim_start();
                let digits: String = num_start
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let Ok(val) = digits.parse::<f64>()
                    && val.is_finite()
                {
                    return Some(val);
                }
            }
            rest = after_key;
        }
    }
    None
}

async fn fetch_tiktok_public_stats(
    client: &reqwest::Client,
    channel_id: &str,
    url: &str,
) -> ChannelStatsUpdate {
    let Some(username) = extract_tiktok_username(url) else {
        return unavailable_stats(channel_id, "TikTok sidecar awaiting room connection");
    };
    let live_url = format!("https://www.tiktok.com/@{username}/live");
    let html = match client
        .get(&live_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
        _ => return unavailable_stats(channel_id, "TikTok sidecar reconnecting"),
    };

    if let Some(ccv) = parse_tiktok_public_ccv(&html) {
        let total_views = ["totalUser", "total_user", "enterCount", "enter_count"]
            .iter()
            .find_map(|k| extract_json_field_number(&html, k))
            .filter(|&v| v > 0.0)
            .map(|v| TriStateMetric::value(v, 100.0))
            .unwrap_or_else(|| TriStateMetric::unavailable("TikTok total views awaiting sidecar"));
        let likes = ["likeCount", "like_count"]
            .iter()
            .find_map(|k| extract_json_field_number(&html, k))
            .filter(|&v| v > 0.0)
            .map(|v| TriStateMetric::value(v, 100.0))
            .unwrap_or_else(|| TriStateMetric::unavailable("TikTok likes awaiting sidecar"));
        let shares = ["shareCount", "share_count"]
            .iter()
            .find_map(|k| extract_json_field_number(&html, k))
            .filter(|&v| v > 0.0)
            .map(|v| TriStateMetric::value(v, 100.0))
            .unwrap_or_else(|| TriStateMetric::unavailable("TikTok shares awaiting sidecar"));
        ChannelStatsUpdate {
            channel_id: channel_id.to_string(),
            ccv: TriStateMetric::value(ccv, 100.0),
            total_views,
            likes,
            chat_rate_per_min: TriStateMetric::unavailable("TikTok sidecar reconnecting"),
            shares,
            gifts_count: TriStateMetric::unavailable("TikTok gifts awaiting sidecar"),
            extras: HashMap::new(),
        }
    } else {
        unavailable_stats(channel_id, "TikTok sidecar awaiting room connection")
    }
}

pub(crate) fn extract_tiktok_username(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = reqwest::Url::parse(&normalized).ok()?;
    let segments: Vec<&str> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .collect();
    if let Some(at_seg) = segments.iter().find(|s| s.starts_with('@')) {
        let user = at_seg.trim_start_matches('@').trim();
        if !user.is_empty() {
            return Some(user.to_string());
        }
    }
    let first = segments.first()?.trim_start_matches('@').trim();
    if first.is_empty()
        || matches!(
            first.to_ascii_lowercase().as_str(),
            "t" | "v" | "live" | "video" | "embed" | "share" | "explore" | "following"
        )
    {
        return None;
    }
    Some(first.to_string())
}

pub(crate) fn extract_twitch_login(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let parsed = reqwest::Url::parse(&normalized).ok()?;
    let segments: Vec<&str> = parsed
        .path_segments()?
        .filter(|s| !s.is_empty())
        .collect();
    let candidate = match segments.first().copied() {
        Some("popout" | "embed") => segments.get(1).copied()?,
        Some(seg) => seg,
        None => return None,
    };
    let clean = candidate.trim_start_matches('@').to_ascii_lowercase();
    if clean.is_empty()
        || matches!(
            clean.as_str(),
            "directory" | "videos" | "p" | "downloads" | "turbo" | "settings" | "login" | "signup"
        )
    {
        return None;
    }
    Some(clean)
}

async fn fetch_twitch_public_stats(
    client: &reqwest::Client,
    channel_id: &str,
    url: &str,
    chat_rate: Option<f64>,
    gifts_count: Option<f64>,
) -> ChannelStatsUpdate {
    let Some(login) = extract_twitch_login(url) else {
        return unavailable_stats(channel_id, "Invalid Twitch login URL");
    };

    let gql_body = serde_json::json!({
        "query": "query($login:String!){user(login:$login){stream{viewersCount title game{name}}}}",
        "variables": { "login": login }
    });

    let Ok(resp) = client
        .post("https://gql.twitch.tv/gql")
        .header("Client-ID", "kimne78kx3ncx6brgo4mv6wki5h1ko")
        .json(&gql_body)
        .send()
        .await
    else {
        return unavailable_stats(channel_id, "Twitch GQL request failed");
    };

    let Ok(val) = resp.json::<serde_json::Value>().await else {
        return unavailable_stats(channel_id, "Twitch GQL parse error");
    };

    if let Some(viewers) = val.pointer("/data/user/stream/viewersCount").and_then(|v| v.as_f64()) {
        let mut extras = HashMap::new();
        if let Some(title) = val.pointer("/data/user/stream/title").and_then(|v| v.as_str()) {
            extras.insert("title".into(), serde_json::json!(title));
        }
        ChannelStatsUpdate {
            channel_id: channel_id.to_string(),
            ccv: TriStateMetric::value(viewers, 100.0),
            total_views: TriStateMetric::na(),
            likes: TriStateMetric::na(),
            chat_rate_per_min: chat_rate
                .map(|v| TriStateMetric::value(v, 100.0))
                .unwrap_or_else(|| TriStateMetric::unavailable("Live chat stream connecting")),
            shares: TriStateMetric::na(),
            gifts_count: gifts_count
                .map(|v| TriStateMetric::value(v, 100.0))
                .unwrap_or_else(|| TriStateMetric::unavailable("Live chat stream connecting")),
            extras,
        }
    } else {
        unavailable_stats(channel_id, "Twitch stream offline or GQL blocked")
    }
}

fn extract_json_field_number(haystack: &str, key: &str) -> Option<f64> {
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
                    let window = &inner[..inner.len().min(96)];
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
                    let obj_win = &inner[..inner.len().min(96)];
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

fn parse_facebook_html_stats(channel_id: &str, url: &str, html: &str) -> Option<ChannelStatsUpdate> {
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

    resp.text()
        .await
        .map_err(|_| "Facebook HTML read failed".to_string())
}

async fn fetch_facebook_public_stats(
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

fn extract_json_number_near(haystack: &str, needle: &str) -> Option<f64> {
    let idx = haystack.find(needle)?;
    let rest = &haystack[idx + needle.len()..];
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<f64>().ok()
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SidecarEvent {
    Stats {
        channel_id: String,
        ccv: Option<f64>,
        #[serde(default)]
        total_views: Option<f64>,
        likes: Option<f64>,
        #[serde(default)]
        shares: Option<f64>,
        gifts: Option<f64>,
        chat_count: Option<f64>,
    },
    Chat {
        channel_id: String,
        viewer_id: String,
        text: String,
    },
    Error {
        channel_id: String,
        message: String,
    },
}

pub struct TikTokSidecarClient {
    stdin: Option<tokio::process::ChildStdin>,
    child: Option<tokio::process::Child>,
}

impl TikTokSidecarClient {
    pub fn spawn(
        node_exe: &Path,
        script_path: &Path,
        on_event: tokio::sync::mpsc::Sender<SidecarEvent>,
        job_guard: Option<&crate::win32::WindowsJobGuard>,
    ) -> Self {
        if !node_exe.exists() || !script_path.exists() {
            tracing::info!("TikTok sidecar skipped (node.exe or sidecar script not staged yet)");
            return Self { stdin: None, child: None };
        }
        use pgmwatch_core::process::BackgroundCommandExt;
        let mut cmd = Command::new(node_exe);
        cmd.no_window()
            .kill_on_drop(true)
            .arg(script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                if let (Some(guard), Some(pid)) = (job_guard, child.id())
                    && let Err(err) = guard.assign_pid(pid)
                {
                    tracing::debug!("Failed assigning TikTok sidecar PID to job guard: {err}");
                }
                let stdin = child.stdin.take();
                if let Some(stderr) = child.stderr.take() {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stderr).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let trimmed = line.trim();
                            if !trimmed.is_empty() {
                                tracing::warn!("tiktok-sidecar: {trimmed}");
                            }
                        }
                    });
                }
                if let Some(stdout) = child.stdout.take() {
                    tokio::spawn(async move {
                        let mut lines = BufReader::new(stdout).lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            if let Ok(ev) = serde_json::from_str::<SidecarEvent>(&line) {
                                let _ = on_event.send(ev).await;
                            }
                        }
                    });
                }
                Self {
                    stdin,
                    child: Some(child),
                }
            }
            Err(err) => {
                tracing::warn!("Failed to spawn TikTok sidecar: {err}");
                Self { stdin: None, child: None }
            }
        }
    }

    pub async fn stop(&mut self) {
        self.stdin = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    pub async fn subscribe(&mut self, channel_id: &str, username: &str, session_id: Option<&str>) {
        if let Some(ref mut stdin) = self.stdin {
            let line = serde_json::json!({
                "cmd": "subscribe",
                "channel_id": channel_id,
                "username": username,
                "session_id": session_id
            })
            .to_string();
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
            let _ = stdin.flush().await;
        }
    }

    pub async fn unsubscribe(&mut self, channel_id: &str) {
        if let Some(ref mut stdin) = self.stdin {
            let line = serde_json::json!({
                "cmd": "unsubscribe",
                "channel_id": channel_id
            })
            .to_string();
            let _ = stdin.write_all(line.as_bytes()).await;
            let _ = stdin.write_all(b"\n").await;
            let _ = stdin.flush().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_twitch_login_with_live_suffix_and_queries() {
        assert_eq!(
            extract_twitch_login("https://www.twitch.tv/hok_esports/live").as_deref(),
            Some("hok_esports")
        );
        assert_eq!(
            extract_twitch_login("https://twitch.tv/ESL_Dota2?referrer=raid").as_deref(),
            Some("esl_dota2")
        );
        assert_eq!(
            extract_twitch_login("https://www.twitch.tv/popout/hok_esports/chat").as_deref(),
            Some("hok_esports")
        );
        assert_eq!(extract_twitch_login("https://www.twitch.tv/directory/game/Honor"), None);
    }

    #[test]
    fn parses_facebook_public_viewer_and_play_count() {
        let html = r#"<script>{"live_viewer_count":null,"live_viewer_count_read_only":2,"play_count":154,"reaction_count":33,"share_count":7,"comment_count":19}</script>"#;
        let parsed = parse_facebook_html_stats(
            "ch-fb-main",
            "https://www.facebook.com/events/1702525898135304/",
            html,
        )
        .expect("FB ccv should parse");
        assert_eq!(parsed.ccv.status, MetricStatus::Value);
        assert_eq!(parsed.ccv.value, Some(2.0));
        assert_eq!(parsed.total_views.status, MetricStatus::Value);
        assert_eq!(parsed.total_views.value, Some(154.0));
        assert_eq!(parsed.likes.value, Some(33.0));
        assert_eq!(parsed.shares.value, Some(7.0));
        assert_eq!(parsed.chat_rate_per_min.status, MetricStatus::Na);
        assert_eq!(parsed.extras.get("comment_count"), Some(&serde_json::json!(19)));

        let escaped = r#"{\"viewer_count\": 42, \"video_view_count\": 901, \"reaction_count\": {\"count\": 128}, \"share_count\": {\"count\": 14}}"#;
        let parsed_escaped = parse_facebook_html_stats(
            "ch-fb-2",
            "https://www.facebook.com/events/1702525898135304/",
            escaped,
        )
        .expect("escaped FB JSON should parse");
        assert_eq!(parsed_escaped.ccv.value, Some(42.0));
        assert_eq!(parsed_escaped.total_views.value, Some(901.0));
        assert_eq!(parsed_escaped.likes.value, Some(128.0));
        assert_eq!(parsed_escaped.shares.value, Some(14.0));

        // Sidebar recommended video belonging to a different video ID must NOT be attributed to target /videos/<id>
        let sidebar_only = r#"{"video":{"is_live_streaming":true,"live_viewer_count_read_only":3,"id":"1013575908466414"}}"#;
        assert!(
            parse_facebook_html_stats(
                "ch-fb-my",
                "https://www.facebook.com/hokmyofficial/videos/1880057129693239",
                sidebar_only,
            )
            .is_none()
        );

        // Comet /videos/ pages expose live viewers under camelCase `liveViewerCount` near the target video ID
        let comet_video = r#"{"video":{"id":"1880057129693239","liveViewerCount":157,"reaction_count":{"count":12}}}"#;
        let parsed_comet = parse_facebook_html_stats(
            "ch-fb-my",
            "https://www.facebook.com/hokmyofficial/videos/1880057129693239",
            comet_video,
        )
        .expect("Comet liveViewerCount should parse");
        assert_eq!(parsed_comet.ccv.value, Some(157.0));
        assert_eq!(parsed_comet.likes.value, Some(12.0));
    }

    #[test]
    fn parses_youtube_accessibility_likes_and_zero_likes() {
        let with_likes = r#"{"likeCountEntity":{"key":"unset_like_count_entity_key"},"accessibilityText":"like this video along with 1,272 other people"}"#;
        assert_eq!(parse_youtube_html_likes(with_likes), Some(1272.0));

        let zero_likes = r#"{"likeCountEntity":{"key":"unset_like_count_entity_key"}}"#;
        assert_eq!(parse_youtube_html_likes(zero_likes), Some(0.0));

        assert_eq!(parse_youtube_html_likes("<html>no player</html>"), None);
    }

    #[test]
    fn extracts_tiktok_username_from_live_and_profile_urls() {
        assert_eq!(
            extract_tiktok_username("https://www.tiktok.com/@hokglobal/live").as_deref(),
            Some("hokglobal")
        );
        assert_eq!(
            extract_tiktok_username("https://tiktok.com/@my_channel?lang=en").as_deref(),
            Some("my_channel")
        );
        assert_eq!(
            extract_tiktok_username("https://www.tiktok.com/hokglobal/live").as_deref(),
            Some("hokglobal")
        );
        assert_eq!(extract_tiktok_username("https://www.tiktok.com/explore"), None);
        assert_eq!(
            parse_tiktok_public_ccv(r#"{"liveRoom":{"liveRoomStats":{"userCount":147}}}"#),
            Some(147.0)
        );
        assert_eq!(
            parse_tiktok_public_ccv(r#"{"roomInfo":{"user_count":72}}"#),
            Some(72.0)
        );
        assert_eq!(parse_tiktok_public_ccv("<html>offline</html>"), None);
    }
}
