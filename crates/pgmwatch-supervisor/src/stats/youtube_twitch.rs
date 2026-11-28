use super::{record_http_rx_bytes, unavailable_stats, ChannelStatsUpdate};
use super::facebook::{extract_json_field_number, extract_json_number_near};
use pgmwatch_core::models::TriStateMetric;
use std::collections::HashMap;

pub(super) fn parse_youtube_html_likes(html: &str) -> Option<f64> {
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

pub(super) async fn fetch_youtube_public_stats(
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
    let content_len = resp.content_length();
    let Ok(html) = resp.text().await else {
        return unavailable_stats(channel_id, "YouTube response unreadable");
    };
    record_http_rx_bytes(content_len, html.len());

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

pub(super) async fn fetch_twitch_public_stats(
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

