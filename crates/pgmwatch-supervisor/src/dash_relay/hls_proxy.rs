use super::{HLS_DVR_TRIM_THRESHOLD};
use reqwest::Url;

/// Returns true for large live DVR HLS manifests (such as YouTube's 3600s DVR `index.m3u8`,
/// which is ~883 KB uncompressed vs ~7.5 KB with gzip) where proxying through our local
/// gzip-enabled, live-edge-trimming relay saves ~99% of manifest bandwidth.
pub fn should_proxy_hls_manifest(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    if let Ok(parsed) = Url::parse(&lower)
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
    {
        return false;
    }
    let path = lower.split(['?', '#']).next().unwrap_or("");
    if !path.ends_with(".m3u8") {
        return false;
    }
    lower.contains("manifest.googlevideo.com")
        || lower.contains("playlist_type/dvr")
        || lower.contains("manifest_duration/")
}


fn resolve_hls_tag_uri(line: &str, base: Option<&Url>) -> String {
    let Some(b) = base else {
        return line.to_string();
    };
    let Some(uri_idx) = line.find("URI=\"") else {
        return line.to_string();
    };
    let val_start = uri_idx + 5;
    let Some(val_end_rel) = line[val_start..].find('"') else {
        return line.to_string();
    };
    let raw_uri = &line[val_start..val_start + val_end_rel];
    if raw_uri.starts_with("http://") || raw_uri.starts_with("https://") {
        return line.to_string();
    }
    let resolved = b
        .join(raw_uri)
        .map(String::from)
        .unwrap_or_else(|_| raw_uri.to_string());
    format!(
        "{}{}{}",
        &line[..val_start],
        resolved,
        &line[val_start + val_end_rel..]
    )
}

fn resolve_all_relative_uris(base: Option<&Url>, body: &str) -> String {
    let Some(b) = base else {
        return body.to_string();
    };
    let mut out = String::with_capacity(body.len() + 256);
    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            if line.starts_with("#EXT-X-MAP:") || line.starts_with("#EXT-X-KEY:") {
                out.push_str(&resolve_hls_tag_uri(line, Some(b)));
            } else {
                out.push_str(line);
            }
        } else if line.starts_with("http://") || line.starts_with("https://") {
            out.push_str(line);
        } else {
            let resolved = b.join(line).map(String::from).unwrap_or_else(|_| line.to_string());
            out.push_str(&resolved);
        }
        out.push('\n');
    }
    out
}

/// Trims oversized live DVR HLS media playlists (e.g., YouTube's 720-segment 1-hour DVR
/// playlist) down to the trailing `keep_segments` live-edge segments while preserving
/// `#EXT-X-MEDIA-SEQUENCE`, `#EXT-X-DISCONTINUITY-SEQUENCE`, `#EXT-X-KEY`, and `#EXT-X-MAP`,
/// and resolving any relative segment URIs against `base_url`.
pub fn trim_hls_dvr_playlist(base_url: &str, body: &str, keep_segments: usize) -> String {
    let keep = keep_segments.max(3);
    let base = Url::parse(base_url).ok();
    if body.contains("#EXT-X-ENDLIST") || body.contains("#EXT-X-STREAM-INF") {
        return resolve_all_relative_uris(base.as_ref(), body);
    }

    let mut header_lines: Vec<&str> = Vec::new();
    let mut media_seq: u64 = 0;
    let mut has_media_seq = false;
    let mut disc_seq: u64 = 0;
    let mut has_disc_seq = false;

    let mut segments: Vec<Vec<&str>> = Vec::new();
    let mut pending_seg_tags: Vec<&str> = Vec::new();
    let mut in_segments = false;

    for raw_line in body.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if !in_segments {
            if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
                if let Ok(v) = rest.trim().parse::<u64>() {
                    media_seq = v;
                    has_media_seq = true;
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("#EXT-X-DISCONTINUITY-SEQUENCE:") {
                if let Ok(v) = rest.trim().parse::<u64>() {
                    disc_seq = v;
                    has_disc_seq = true;
                }
                continue;
            }
            let is_seg_tag = line.starts_with("#EXTINF:")
                || line.starts_with("#EXT-X-PROGRAM-DATE-TIME:")
                || line.starts_with("#EXT-X-DISCONTINUITY")
                || line.starts_with("#EXT-X-BYTERANGE:")
                || line.starts_with("#EXT-X-KEY:")
                || line.starts_with("#EXT-X-MAP:")
                || line.starts_with("#EXT-X-DATERANGE:");
            if is_seg_tag || !line.starts_with('#') {
                in_segments = true;
            } else {
                header_lines.push(line);
                continue;
            }
        }

        if line.starts_with('#') {
            pending_seg_tags.push(line);
        } else {
            pending_seg_tags.push(line);
            segments.push(std::mem::take(&mut pending_seg_tags));
        }
    }

    if segments.len() <= HLS_DVR_TRIM_THRESHOLD || segments.len() <= keep {
        return resolve_all_relative_uris(base.as_ref(), body);
    }

    let drop_count = segments.len() - keep;
    let mut dropped_discontinuities = 0u64;
    let mut last_dropped_key: Option<&str> = None;
    let mut last_dropped_map: Option<&str> = None;
    for seg in &segments[..drop_count] {
        for &l in seg {
            if l == "#EXT-X-DISCONTINUITY" {
                dropped_discontinuities += 1;
            } else if l.starts_with("#EXT-X-KEY:") {
                last_dropped_key = Some(l);
            } else if l.starts_with("#EXT-X-MAP:") {
                last_dropped_map = Some(l);
            }
        }
    }

    let mut out = String::with_capacity(keep * 512 + 512);
    for &h in &header_lines {
        out.push_str(h);
        out.push('\n');
    }
    if has_media_seq || drop_count > 0 {
        out.push_str(&format!(
            "#EXT-X-MEDIA-SEQUENCE:{}\n",
            media_seq.saturating_add(drop_count as u64)
        ));
    }
    if has_disc_seq || dropped_discontinuities > 0 {
        out.push_str(&format!(
            "#EXT-X-DISCONTINUITY-SEQUENCE:{}\n",
            disc_seq.saturating_add(dropped_discontinuities)
        ));
    }
    if let Some(map_tag) = last_dropped_map {
        out.push_str(&resolve_hls_tag_uri(map_tag, base.as_ref()));
        out.push('\n');
    }
    if let Some(key_tag) = last_dropped_key {
        out.push_str(&resolve_hls_tag_uri(key_tag, base.as_ref()));
        out.push('\n');
    }
    // Only call Url::join on the kept trailing segments (e.g. 12 instead of 720).
    for seg in &segments[drop_count..] {
        for &l in seg {
            if l.starts_with('#') {
                if l.starts_with("#EXT-X-MAP:") || l.starts_with("#EXT-X-KEY:") {
                    out.push_str(&resolve_hls_tag_uri(l, base.as_ref()));
                } else {
                    out.push_str(l);
                }
            } else if l.starts_with("http://") || l.starts_with("https://") {
                out.push_str(l);
            } else if let Some(ref b) = base {
                let resolved = b.join(l).map(String::from).unwrap_or_else(|_| l.to_string());
                out.push_str(&resolved);
            } else {
                out.push_str(l);
            }
            out.push('\n');
        }
    }
    for &tail in &pending_seg_tags {
        out.push_str(tail);
        out.push('\n');
    }
    out
}

