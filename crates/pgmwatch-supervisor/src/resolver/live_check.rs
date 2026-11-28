use super::{ResolveError, ResolvedMedia};

pub(crate) fn is_always_live_transport_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("sim://")
        || lower.starts_with("rtmp://")
        || lower.starts_with("rtmps://")
        || lower.starts_with("srt://")
}

pub(crate) fn is_direct_stream_url(url: &str) -> bool {
    if is_always_live_transport_url(url) {
        return true;
    }
    let lower = url.trim().to_ascii_lowercase();
    let before_query = lower.split(['?', '#']).next().unwrap_or("");
    before_query.ends_with(".m3u8")
        || before_query.ends_with(".mpd")
        || before_query.ends_with(".flv")
}

pub(crate) fn is_static_vod_media_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    let before_query = lower.split(['?', '#']).next().unwrap_or("");
    before_query.ends_with(".mp4")
        || before_query.ends_with(".m4v")
        || before_query.ends_with(".webm")
        || before_query.ends_with(".mov")
        || before_query.ends_with(".mkv")
}

/// Evaluates `yt-dlp` JSON live flags (`live_status`, `is_live`, `was_live`).
/// Returns `Ok(true)` when metadata explicitly confirms an active live stream,
/// `Ok(false)` when metadata omits live flags, and `Err(reason)` for replays/VODs.
pub(crate) fn check_ytdlp_live_status(json: &serde_json::Value) -> Result<bool, String> {
    if let Some(status) = json.get("live_status").and_then(|v| v.as_str()) {
        if status.eq_ignore_ascii_case("is_live") {
            return Ok(true);
        }
        if matches!(
            status.to_ascii_lowercase().as_str(),
            "was_live" | "post_live" | "is_upcoming" | "not_live"
        ) {
            return Err(format!(
                "Stream offline ({status} — live-only mode active)"
            ));
        }
    }
    match json.get("is_live").and_then(|v| v.as_bool()) {
        Some(true) => Ok(true),
        Some(false) => Err("Stream offline (not a live broadcast — live-only mode active)".to_string()),
        None => {
            if json.get("was_live").and_then(|v| v.as_bool()) == Some(true) {
                Err("Stream offline (ended broadcast replay — live-only mode active)".to_string())
            } else {
                Ok(false)
            }
        }
    }
}

pub(crate) fn classify_hls_manifest_vod(body: &str) -> Option<&'static str> {
    for raw_line in body.lines() {
        let upper = raw_line.trim().to_ascii_uppercase();
        if upper == "#EXT-X-ENDLIST" {
            return Some("HLS playlist has #EXT-X-ENDLIST (replay/VOD)");
        }
        if upper.starts_with("#EXT-X-PLAYLIST-TYPE:VOD") {
            return Some("HLS playlist marked VOD");
        }
    }
    None
}

pub(crate) fn first_hls_variant_url(master_url: &str, body: &str) -> Option<String> {
    let base = reqwest::Url::parse(master_url).ok()?;
    let mut expect_uri = false;
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.to_ascii_uppercase().starts_with("#EXT-X-STREAM-INF") {
            expect_uri = true;
            continue;
        }
        if expect_uri && !line.starts_with('#') {
            return base.join(line).ok().map(String::from);
        }
    }
    None
}

pub(crate) fn classify_dash_manifest_vod(body: &str) -> Option<&'static str> {
    let lower = body.to_ascii_lowercase();
    let start = lower.find("<mpd")?;
    let end_rel = lower[start..].find('>')?;
    let tag = &lower[start..start + end_rel];
    if tag.contains("type=\"static\"") || tag.contains("type='static'") {
        return Some("DASH MPD is static VOD (not dynamic live)");
    }
    None
}

fn is_loopback_http_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
}

pub(super) async fn fetch_manifest_text(
    http: &reqwest::Client,
    url: &str,
    headers: &[(String, String)],
) -> Option<String> {
    let mut req = http.get(url);
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.text().await.ok()
}

pub(super) async fn verify_resolved_is_live(
    http: &reqwest::Client,
    resolved: &ResolvedMedia,
    live_streams_only: bool,
    metadata_confirmed_live: bool,
) -> Result<(), ResolveError> {
    if !live_streams_only
        || metadata_confirmed_live
        || is_always_live_transport_url(&resolved.stream_url)
    {
        return Ok(());
    }

    if is_static_vod_media_url(&resolved.stream_url) {
        return Err(ResolveError::Offline(
            "Stream offline (static VOD media file — live-only mode active)".to_string(),
        ));
    }

    if is_loopback_http_url(&resolved.stream_url) {
        return Ok(());
    }

    let lower = resolved.stream_url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or("");
    if path.ends_with(".m3u8") {
        if let Some(body) = fetch_manifest_text(http, &resolved.stream_url, &resolved.headers).await
        {
            if let Some(reason) = classify_hls_manifest_vod(&body) {
                return Err(ResolveError::Offline(format!(
                    "Stream offline ({reason} — live-only mode active)"
                )));
            }
            if let Some(variant_url) = first_hls_variant_url(&resolved.stream_url, &body)
                && let Some(variant_body) =
                    fetch_manifest_text(http, &variant_url, &resolved.headers).await
                && let Some(reason) = classify_hls_manifest_vod(&variant_body)
            {
                return Err(ResolveError::Offline(format!(
                    "Stream offline ({reason} — live-only mode active)"
                )));
            }
        }
    } else if path.ends_with(".mpd")
        && let Some(body) =
            fetch_manifest_text(http, &resolved.stream_url, &resolved.headers).await
        && let Some(reason) = classify_dash_manifest_vod(&body)
    {
        return Err(ResolveError::Offline(format!(
            "Stream offline ({reason} — live-only mode active)"
        )));
    }

    Ok(())
}

pub(super) fn direct_via_label(url: &str) -> &'static str {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("sim://") {
        "simulator"
    } else if lower.starts_with("rtmp://") || lower.starts_with("rtmps://") {
        "direct-rtmp"
    } else if lower.starts_with("srt://") {
        "direct-srt"
    } else {
        "direct"
    }
}

