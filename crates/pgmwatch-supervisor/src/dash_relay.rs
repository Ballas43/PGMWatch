use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Url};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

const MPD_CACHE_TTL: Duration = Duration::from_millis(800);
const HLS_PROXY_CACHE_TTL: Duration = Duration::from_millis(1800);
const HLS_DVR_TRIM_THRESHOLD: usize = 16;
const HLS_DVR_KEEP_SEGMENTS: usize = 12;
const MAX_SEGMENTS_PER_REPRESENTATION: usize = 256;

#[derive(Debug, Clone)]
enum RelayKind {
    DashMpd {
        mpd_url: String,
        format_id: String,
        last_times: Vec<u64>,
        media_sequence: u64,
    },
    HlsProxy {
        playlist_url: String,
    },
}

#[derive(Debug, Clone)]
struct RelayEntry {
    kind: RelayKind,
    headers: Vec<(String, String)>,
}

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

#[derive(Debug, Clone)]
struct CachedMpd {
    xml: String,
    fetched_at: Instant,
}

pub struct DashRelayState {
    local_port: u16,
    client: Client,
    entries: RwLock<HashMap<String, Arc<Mutex<RelayEntry>>>>,
    by_source: Mutex<HashMap<String, Vec<String>>>,
    mpd_cache: Mutex<HashMap<String, CachedMpd>>,
}

impl DashRelayState {
    pub fn new(local_port: u16) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(8))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            local_port,
            client,
            entries: RwLock::new(HashMap::new()),
            by_source: Mutex::new(HashMap::new()),
            mpd_cache: Mutex::new(HashMap::new()),
        }
    }

    pub async fn clear_source(&self, source_url: &str) {
        let old_tokens = self.by_source.lock().await.remove(source_url);
        if let Some(tokens) = old_tokens {
            let mut entries = self.entries.write().await;
            for tok in tokens {
                entries.remove(&tok);
            }
        }
    }

    pub async fn register(
        &self,
        source_url: &str,
        mpd_url: String,
        format_id: String,
        headers: Vec<(String, String)>,
    ) -> String {
        let token = Uuid::new_v4().simple().to_string();
        let entry = Arc::new(Mutex::new(RelayEntry {
            kind: RelayKind::DashMpd {
                mpd_url,
                format_id,
                last_times: Vec::new(),
                media_sequence: 0,
            },
            headers,
        }));
        self.entries.write().await.insert(token.clone(), entry);
        self.by_source
            .lock()
            .await
            .entry(source_url.to_string())
            .or_default()
            .push(token.clone());
        format!(
            "http://127.0.0.1:{}/api/internal/dash-hls/{}.m3u8",
            self.local_port, token
        )
    }

    pub async fn register_hls_proxy(
        &self,
        source_url: &str,
        playlist_url: String,
        headers: Vec<(String, String)>,
    ) -> String {
        let token = Uuid::new_v4().simple().to_string();
        let entry = Arc::new(Mutex::new(RelayEntry {
            kind: RelayKind::HlsProxy { playlist_url },
            headers,
        }));
        self.entries.write().await.insert(token.clone(), entry);
        self.by_source
            .lock()
            .await
            .entry(source_url.to_string())
            .or_default()
            .push(token.clone());
        format!(
            "http://127.0.0.1:{}/api/internal/dash-hls/{}.m3u8",
            self.local_port, token
        )
    }

    pub async fn render_hls(&self, raw_token: &str) -> Result<String> {
        let token = raw_token.strip_suffix(".m3u8").unwrap_or(raw_token);
        if token.is_empty()
            || !token
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(anyhow!("Invalid DASH relay token"));
        }

        let entry_arc = self
            .entries
            .read()
            .await
            .get(token)
            .cloned()
            .ok_or_else(|| anyhow!("Unknown or expired DASH relay token"))?;

        let (kind, headers) = {
            let e = entry_arc.lock().await;
            (e.kind.clone(), e.headers.clone())
        };

        match kind {
            RelayKind::HlsProxy { playlist_url } => {
                let raw_m3u8 = self
                    .fetch_manifest_cached(&playlist_url, &headers, HLS_PROXY_CACHE_TTL)
                    .await?;
                Ok(trim_hls_dvr_playlist(
                    &playlist_url,
                    &raw_m3u8,
                    HLS_DVR_KEEP_SEGMENTS,
                ))
            }
            RelayKind::DashMpd {
                mpd_url,
                format_id,
                ..
            } => {
                let xml = self
                    .fetch_manifest_cached(&mpd_url, &headers, MPD_CACHE_TTL)
                    .await?;
                let parsed = parse_dash_representation_to_segments(&xml, &mpd_url, &format_id)?;

                let mut entry = entry_arc.lock().await;
                let seg_times: Vec<u64> = parsed.segments.iter().map(|s| s.start_tick).collect();
                let media_seq = advance_media_sequence(&mut entry, &seg_times);
                drop(entry);

                let ad_break = is_dash_mpd_ad_break(&xml);
                Ok(format_hls_v7_playlist(&parsed, media_seq, ad_break))
            }
        }
    }

    async fn fetch_manifest_cached(
        &self,
        manifest_url: &str,
        headers: &[(String, String)],
        ttl: Duration,
    ) -> Result<String> {
        {
            let cache = self.mpd_cache.lock().await;
            if let Some(hit) = cache.get(manifest_url)
                && hit.fetched_at.elapsed() < ttl
            {
                return Ok(hit.xml.clone());
            }
        }

        let mut req_headers = HeaderMap::new();
        for (k, v) in headers {
            let lower = k.to_ascii_lowercase();
            // Strip Range and Accept-Encoding so reqwest negotiates gzip cleanly
            // (Google's manifest.googlevideo.com disables gzip when Range is sent).
            if matches!(
                lower.as_str(),
                "host" | "content-length" | "connection" | "accept-encoding" | "range"
            ) {
                continue;
            }
            if let (Ok(name), Ok(val)) = (
                HeaderName::from_bytes(k.as_bytes()),
                HeaderValue::from_str(v),
            ) {
                req_headers.insert(name, val);
            }
        }
        if !req_headers.contains_key(reqwest::header::USER_AGENT) {
            req_headers.insert(
                reqwest::header::USER_AGENT,
                HeaderValue::from_static(
                    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
                ),
            );
        }

        let resp = self
            .client
            .get(manifest_url)
            .headers(req_headers)
            .send()
            .await
            .with_context(|| format!("Failed fetching manifest {manifest_url}"))?;

        if !resp.status().is_success() {
            return Err(anyhow!("Manifest fetch returned HTTP {}", resp.status()));
        }

        let content_len = resp.content_length();
        let body = resp.text().await.context("Failed reading manifest body")?;
        crate::stats::record_http_rx_bytes(content_len, body.len());
        {
            let mut cache = self.mpd_cache.lock().await;
            if cache.len() > 128 {
                cache.retain(|_, v| v.fetched_at.elapsed() < Duration::from_secs(10));
            }
            cache.insert(
                manifest_url.to_string(),
                CachedMpd {
                    xml: body.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }
        Ok(body)
    }
}

/// Trims oversized live DVR HLS media playlists (e.g., YouTube's 720-segment 1-hour DVR
/// playlist) down to the trailing `keep_segments` live-edge segments while preserving
/// `#EXT-X-MEDIA-SEQUENCE`, `#EXT-X-DISCONTINUITY-SEQUENCE`, `#EXT-X-KEY`, and `#EXT-X-MAP`,
/// and resolving any relative segment URIs against `base_url`.
pub fn trim_hls_dvr_playlist(base_url: &str, body: &str, keep_segments: usize) -> String {
    let keep = keep_segments.max(3);
    if body.contains("#EXT-X-ENDLIST") || body.contains("#EXT-X-STREAM-INF") {
        return body.to_string();
    }

    let base = Url::parse(base_url).ok();
    let mut header_lines: Vec<String> = Vec::new();
    let mut media_seq: u64 = 0;
    let mut has_media_seq = false;
    let mut disc_seq: u64 = 0;
    let mut has_disc_seq = false;

    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut pending_seg_tags: Vec<String> = Vec::new();
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
                header_lines.push(line.to_string());
                continue;
            }
        }

        if line.starts_with('#') {
            pending_seg_tags.push(line.to_string());
        } else {
            let resolved_uri = if let Some(ref b) = base {
                b.join(line).map(String::from).unwrap_or_else(|_| line.to_string())
            } else {
                line.to_string()
            };
            pending_seg_tags.push(resolved_uri);
            segments.push(std::mem::take(&mut pending_seg_tags));
        }
    }

    if segments.len() <= HLS_DVR_TRIM_THRESHOLD || segments.len() <= keep {
        return body.to_string();
    }

    let drop_count = segments.len() - keep;
    let mut dropped_discontinuities = 0u64;
    let mut last_dropped_key: Option<String> = None;
    let mut last_dropped_map: Option<String> = None;
    for seg in &segments[..drop_count] {
        for l in seg {
            if l == "#EXT-X-DISCONTINUITY" {
                dropped_discontinuities += 1;
            } else if l.starts_with("#EXT-X-KEY:") {
                last_dropped_key = Some(l.clone());
            } else if l.starts_with("#EXT-X-MAP:") {
                last_dropped_map = Some(l.clone());
            }
        }
    }

    let mut out = String::with_capacity(keep * 512 + 512);
    for h in &header_lines {
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
        out.push_str(&map_tag);
        out.push('\n');
    }
    if let Some(key_tag) = last_dropped_key {
        out.push_str(&key_tag);
        out.push('\n');
    }
    for seg in &segments[drop_count..] {
        for l in seg {
            out.push_str(l);
            out.push('\n');
        }
    }
    for tail in &pending_seg_tags {
        out.push_str(tail);
        out.push('\n');
    }
    out
}

fn advance_media_sequence(entry: &mut RelayEntry, seg_times: &[u64]) -> u64 {
    let RelayKind::DashMpd {
        last_times,
        media_sequence,
        ..
    } = &mut entry.kind
    else {
        return 0;
    };
    if let Some(&first_t) = seg_times.first() {
        if !last_times.is_empty() {
            if let Some(pos) = last_times.iter().position(|&t| t == first_t) {
                *media_sequence = media_sequence.saturating_add(pos as u64);
            } else if let Some(&last_prev) = last_times.last()
                && first_t > last_prev
            {
                *media_sequence = media_sequence.saturating_add(last_times.len() as u64);
            }
        }
        *last_times = seg_times.to_vec();
    }
    *media_sequence
}

#[derive(Debug, Clone)]
struct DashSegment {
    start_tick: u64,
    duration_secs: f64,
    url: String,
}

#[derive(Debug, Clone)]
struct ParsedDashPlaylist {
    target_duration: u64,
    init_url: String,
    segments: Vec<DashSegment>,
}

pub(crate) fn is_dash_mpd_ad_break(xml: &str) -> bool {
    let lower = xml.to_ascii_lowercase();
    if lower.contains("<eventstream")
        && (lower.contains("urn:scte:scte35")
            || lower.contains("scte35")
            || (lower.contains("urn:mpeg:dash:event:2012")
                && (lower.contains("splice_insert")
                    || lower.contains("ad_break")
                    || lower.contains("cue-out")
                    || lower.contains("adbreak"))))
    {
        return true;
    }
    for seg in lower.split("<period").skip(1) {
        let tag_head = seg.split('>').next().unwrap_or("");
        if let Some(pos) = tag_head.find("id=") {
            let after = &tag_head[pos + 3..];
            let id_val = after
                .trim_start_matches(['"', '\''])
                .split(['"', '\'', ' '])
                .next()
                .unwrap_or("");
            if id_val.contains("ad-")
                || id_val.contains("_ad_")
                || id_val.contains("adbreak")
                || id_val.contains("preroll")
                || id_val.contains("midroll")
            {
                return true;
            }
        }
    }
    false
}

fn format_hls_v7_playlist(parsed: &ParsedDashPlaylist, media_sequence: u64, ad_break: bool) -> String {
    let mut lines = Vec::with_capacity(7 + parsed.segments.len() * 2);
    lines.push("#EXTM3U".to_string());
    lines.push("#EXT-X-VERSION:7".to_string());
    lines.push(format!("#EXT-X-TARGETDURATION:{}", parsed.target_duration));
    lines.push(format!("#EXT-X-MEDIA-SEQUENCE:{}", media_sequence));
    if ad_break {
        lines.push("#EXT-X-CUE-OUT:30".to_string());
    }
    lines.push(format!(
        "#EXT-X-MAP:URI=\"{}\"",
        parsed.init_url.replace('"', "%22")
    ));
    for seg in &parsed.segments {
        lines.push(format!("#EXTINF:{:.3},", seg.duration_secs));
        lines.push(seg.url.clone());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Picks the best `(video_rep_id, audio_rep_id)` directly from a DASH MPD XML document
/// for fallback resolvers (such as Facebook Comet SSR `dash_manifest_urls`) that bypass yt-dlp.
pub fn pick_dash_video_audio_rep_ids(
    xml: &str,
    max_video_height: Option<u32>,
    audio_only: bool,
) -> (Option<String>, Option<String>) {
    let Some(root) = parse_xml_root(xml) else {
        return (None, None);
    };
    let Ok(dummy_base) = Url::parse("https://dash.local/manifest.mpd") else {
        return (None, None);
    };
    let mut candidates: Vec<RepEntry<'_>> = Vec::new();
    let periods = root.children_named("Period");
    if periods.is_empty() {
        let _ = collect_representations(&root, None, &dummy_base, &mut candidates);
    } else {
        for period in periods {
            let _ = collect_representations(period, None, &dummy_base, &mut candidates);
        }
    }

    let mut videos: Vec<(&str, u32, u64)> = Vec::new();
    let mut audios: Vec<(&str, u64)> = Vec::new();

    for c in &candidates {
        if c.rep_id.is_empty() {
            continue;
        }
        let mime = c
            .rep
            .attrs
            .get("mimeType")
            .or_else(|| c.adapt.and_then(|a| a.attrs.get("mimeType")))
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let bw: u64 = c
            .rep
            .attrs
            .get("bandwidth")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let height: u32 = c
            .rep
            .attrs
            .get("height")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if mime.starts_with("audio/") || (height == 0 && c.rep_id.contains("-a")) {
            audios.push((c.rep_id, bw));
        } else if mime.starts_with("video/") || height > 0 {
            videos.push((c.rep_id, height, bw));
        }
    }

    let best_audio = audios
        .iter()
        .max_by_key(|(_, bw)| *bw)
        .map(|(id, _)| (*id).to_string());

    if audio_only {
        return (best_audio, None);
    }

    let best_video = if let Some(cap_h) = max_video_height {
        videos
            .iter()
            .filter(|(_, h, _)| *h > 0 && *h <= cap_h)
            .max_by_key(|(_, h, bw)| (*h, *bw))
            .or_else(|| videos.iter().min_by_key(|(_, h, bw)| (*h, *bw)))
            .map(|(id, _, _)| (*id).to_string())
    } else {
        videos
            .iter()
            .max_by_key(|(_, h, bw)| (*h, *bw))
            .map(|(id, _, _)| (*id).to_string())
    };

    (best_video, best_audio)
}

fn parse_dash_representation_to_segments(
    xml: &str,
    mpd_url: &str,
    format_id: &str,
) -> Result<ParsedDashPlaylist> {
    let root = parse_xml_root(xml).ok_or_else(|| anyhow!("Empty or invalid MPD XML"))?;
    let mpd_base = Url::parse(mpd_url).with_context(|| format!("Invalid MPD URL: {mpd_url}"))?;
    let mpd_resolved = resolve_base_url(&mpd_base, &root)?;

    let mut candidates: Vec<RepEntry<'_>> = Vec::new();
    let periods = root.children_named("Period");
    if periods.is_empty() {
        collect_representations(&root, None, &mpd_resolved, &mut candidates)?;
    } else {
        for period in periods {
            let period_base = resolve_base_url(&mpd_resolved, period)?;
            collect_representations(period, None, &period_base, &mut candidates)?;
        }
    }

    if candidates.is_empty() {
        return Err(anyhow!("No DASH <Representation> elements found in MPD"));
    }

    let chosen = pick_representation(&candidates, format_id).ok_or_else(|| {
        let avail: Vec<&str> = candidates.iter().map(|c| c.rep_id).collect();
        anyhow!(
            "DASH representation '{}' not found in MPD (available: {})",
            format_id,
            avail.join(", ")
        )
    })?;

    let rep_tmpl = chosen.rep.find_child("SegmentTemplate");
    let adapt_tmpl = chosen
        .adapt
        .and_then(|a| a.find_child("SegmentTemplate"));
    if rep_tmpl.is_none() && adapt_tmpl.is_none() {
        return Err(anyhow!(
            "DASH representation '{}' has no <SegmentTemplate>",
            chosen.rep_id
        ));
    }

    let attr = |key: &str| -> Option<&str> {
        rep_tmpl
            .and_then(|t| t.attrs.get(key).map(|s| s.as_str()))
            .or_else(|| adapt_tmpl.and_then(|t| t.attrs.get(key).map(|s| s.as_str())))
    };

    let timescale: u64 = attr("timescale")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(1);
    let start_number: u64 = attr("startNumber")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);

    let init_tmpl = attr("initialization")
        .ok_or_else(|| anyhow!("DASH <SegmentTemplate> missing initialization attribute"))?;
    let media_tmpl = attr("media")
        .ok_or_else(|| anyhow!("DASH <SegmentTemplate> missing media attribute"))?;

    let init_rel = replace_rep_id(init_tmpl, chosen.rep_id);
    let init_url = chosen
        .base
        .join(&init_rel)
        .with_context(|| format!("Failed resolving DASH init URL '{init_rel}'"))?
        .to_string();

    let timeline = rep_tmpl
        .and_then(|t| t.find_child("SegmentTimeline"))
        .or_else(|| adapt_tmpl.and_then(|t| t.find_child("SegmentTimeline")))
        .ok_or_else(|| anyhow!("DASH <SegmentTemplate> missing <SegmentTimeline>"))?;

    let mut segments = Vec::new();
    let mut cur_t: u64 = 0;
    let mut seg_number: u64 = start_number;
    let mut max_dur: f64 = 1.0;

    for s in timeline.children_named("S") {
        if let Some(t_str) = s.attrs.get("t")
            && let Ok(parsed_t) = t_str.parse::<u64>()
        {
            cur_t = parsed_t;
        }
        let d: u64 = s
            .attrs
            .get("d")
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .ok_or_else(|| anyhow!("DASH <SegmentTimeline><S> missing valid duration 'd'"))?;
        let repeat_extra: usize = s
            .attrs
            .get("r")
            .and_then(|v| v.parse::<i64>().ok())
            .map(|r| r.max(0) as usize)
            .unwrap_or(0)
            .min(MAX_SEGMENTS_PER_REPRESENTATION);

        let dur_secs = d as f64 / timescale as f64;
        if dur_secs > max_dur {
            max_dur = dur_secs;
        }

        for _ in 0..=repeat_extra {
            if segments.len() >= MAX_SEGMENTS_PER_REPRESENTATION {
                break;
            }
            let seg_rel = replace_rep_id(media_tmpl, chosen.rep_id)
                .replace("$Time$", &cur_t.to_string())
                .replace("$Number$", &seg_number.to_string());
            let seg_abs = chosen
                .base
                .join(&seg_rel)
                .with_context(|| format!("Failed resolving DASH segment URL '{seg_rel}'"))?
                .to_string();
            segments.push(DashSegment {
                start_tick: cur_t,
                duration_secs: dur_secs,
                url: seg_abs,
            });
            cur_t = cur_t.saturating_add(d);
            seg_number = seg_number.saturating_add(1);
        }
    }

    if segments.is_empty() {
        return Err(anyhow!("DASH <SegmentTimeline> produced 0 segments"));
    }

    Ok(ParsedDashPlaylist {
        target_duration: max_dur.ceil().max(1.0) as u64,
        init_url,
        segments,
    })
}

fn collect_representations<'a>(
    container: &'a XmlNode,
    enclosing_adapt: Option<&'a XmlNode>,
    base: &Url,
    out: &mut Vec<impl From<(&'a str, &'a XmlNode, Option<&'a XmlNode>, Url)>>,
) -> Result<()> {
    for adapt in container.children_named("AdaptationSet") {
        let adapt_base = resolve_base_url(base, adapt)?;
        for rep in adapt.children_named("Representation") {
            let rep_base = resolve_base_url(&adapt_base, rep)?;
            let rep_id = rep.attrs.get("id").map(|s| s.as_str()).unwrap_or("");
            out.push((rep_id, rep, Some(adapt), rep_base).into());
        }
    }
    for rep in container.children_named("Representation") {
        let rep_base = resolve_base_url(base, rep)?;
        let rep_id = rep.attrs.get("id").map(|s| s.as_str()).unwrap_or("");
        out.push((rep_id, rep, enclosing_adapt, rep_base).into());
    }
    Ok(())
}

// Local conversion helper without exposing private Candidate outside parse_dash_representation_to_segments
struct RepEntry<'a> {
    rep_id: &'a str,
    rep: &'a XmlNode,
    adapt: Option<&'a XmlNode>,
    base: Url,
}

impl<'a> From<(&'a str, &'a XmlNode, Option<&'a XmlNode>, Url)> for RepEntry<'a> {
    fn from(v: (&'a str, &'a XmlNode, Option<&'a XmlNode>, Url)) -> Self {
        Self {
            rep_id: v.0,
            rep: v.1,
            adapt: v.2,
            base: v.3,
        }
    }
}

fn pick_representation<'a, T>(
    candidates: &'a [T],
    format_id: &str,
) -> Option<&'a T>
where
    T: HasRepId,
{
    if !format_id.is_empty() {
        if let Some(exact) = candidates.iter().find(|c| c.rep_id() == format_id) {
            return Some(exact);
        }
        // yt-dlp appends "-<n>" on duplicate Representation ids (e.g. "dash-lp-pst-v-2")
        if let Some(matched) = candidates
            .iter()
            .filter(|c| {
                let id = c.rep_id();
                !id.is_empty()
                    && format_id.starts_with(id)
                    && format_id.as_bytes().get(id.len()) == Some(&b'-')
                    && format_id[id.len() + 1..]
                        .chars()
                        .all(|ch| ch.is_ascii_digit())
            })
            .max_by_key(|c| c.rep_id().len())
        {
            return Some(matched);
        }
        return None;
    }
    candidates.first()
}

trait HasRepId {
    fn rep_id(&self) -> &str;
}

impl<'a> HasRepId for RepEntry<'a> {
    fn rep_id(&self) -> &str {
        self.rep_id
    }
}

fn replace_rep_id(template: &str, rep_id: &str) -> String {
    template.replace("$RepresentationID$", rep_id)
}

fn resolve_base_url(current: &Url, node: &XmlNode) -> Result<Url> {
    if let Some(base_el) = node.find_child("BaseURL") {
        let raw = base_el.text.trim();
        if !raw.is_empty() {
            return current
                .join(raw)
                .with_context(|| format!("Invalid <BaseURL> '{raw}'"));
        }
    }
    Ok(current.clone())
}

#[derive(Debug, Clone)]
struct XmlNode {
    tag: String,
    attrs: HashMap<String, String>,
    text: String,
    children: Vec<XmlNode>,
}

impl XmlNode {
    fn find_child(&self, local_name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.tag == local_name)
    }

    fn children_named(&self, local_name: &str) -> Vec<&XmlNode> {
        self.children
            .iter()
            .filter(|c| c.tag == local_name)
            .collect()
    }
}

fn local_tag_name(raw: &str) -> String {
    raw.rsplit_once(':')
        .map_or(raw, |(_, local)| local)
        .to_string()
}

fn parse_xml_root(input: &str) -> Option<XmlNode> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    let mut stack: Vec<XmlNode> = Vec::new();
    let mut roots: Vec<XmlNode> = Vec::new();

    while i < len {
        if let Some(rel_lt) = input[i..].find('<') {
            let text_slice = &input[i..i + rel_lt];
            if !text_slice.trim().is_empty()
                && let Some(top) = stack.last_mut()
            {
                top.text.push_str(&unescape_xml(text_slice));
            }
            i += rel_lt;
        } else {
            break;
        }

        if input[i..].starts_with("<!--") {
            if let Some(end) = input[i + 4..].find("-->") {
                i += 4 + end + 3;
                continue;
            }
            break;
        }
        if input[i..].starts_with("<![CDATA[") {
            if let Some(end) = input[i + 9..].find("]]>") {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&input[i + 9..i + 9 + end]);
                }
                i += 9 + end + 3;
                continue;
            }
            break;
        }
        if input[i..].starts_with("<?") {
            if let Some(end) = input[i + 2..].find("?>") {
                i += 2 + end + 2;
                continue;
            }
            break;
        }
        if input[i..].starts_with("<!") {
            if let Some(end) = input[i + 2..].find('>') {
                i += 2 + end + 1;
                continue;
            }
            break;
        }

        let tag_end = find_tag_close(input, i + 1)?;
        let raw_tag_body = input[i + 1..tag_end].trim();
        i = tag_end + 1;

        if raw_tag_body.is_empty() {
            continue;
        }

        if let Some(close_name) = raw_tag_body.strip_prefix('/') {
            let local = local_tag_name(close_name.trim());
            while let Some(popped) = stack.pop() {
                let matches = popped.tag == local;
                if let Some(parent) = stack.last_mut() {
                    parent.children.push(popped);
                } else {
                    roots.push(popped);
                }
                if matches {
                    break;
                }
            }
            continue;
        }

        let self_closing = raw_tag_body.ends_with('/');
        let inner = if self_closing {
            raw_tag_body[..raw_tag_body.len() - 1].trim_end()
        } else {
            raw_tag_body
        };

        let (tag_name, attr_str) = split_tag_and_attrs(inner);
        let node = XmlNode {
            tag: local_tag_name(tag_name),
            attrs: parse_xml_attrs(attr_str),
            text: String::new(),
            children: Vec::new(),
        };

        if self_closing {
            if let Some(parent) = stack.last_mut() {
                parent.children.push(node);
            } else {
                roots.push(node);
            }
        } else {
            stack.push(node);
        }
    }

    while let Some(popped) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(popped);
        } else {
            roots.push(popped);
        }
    }

    roots.into_iter().next()
}

fn find_tag_close(s: &str, start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    for (idx, b) in s.as_bytes()[start..].iter().enumerate() {
        match (quote, *b) {
            (None, b'"') | (None, b'\'') => quote = Some(*b),
            (Some(q), c) if q == c => quote = None,
            (None, b'>') => return Some(start + idx),
            _ => {}
        }
    }
    None
}

fn split_tag_and_attrs(inner: &str) -> (&str, &str) {
    match inner.find(char::is_whitespace) {
        Some(idx) => (&inner[..idx], inner[idx..].trim_start()),
        None => (inner, ""),
    }
}

fn parse_xml_attrs(mut s: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            break;
        }
        let Some(eq_idx) = s.find('=') else {
            break;
        };
        let key = s[..eq_idx].trim();
        let after_eq = s[eq_idx + 1..].trim_start();
        let Some(first_byte) = after_eq.as_bytes().first().copied() else {
            break;
        };
        if first_byte != b'"' && first_byte != b'\'' {
            s = after_eq.find(char::is_whitespace).map_or("", |w| &after_eq[w..]);
            continue;
        }
        let quote_char = first_byte as char;
        let rest = &after_eq[1..];
        let Some(close_idx) = rest.find(quote_char) else {
            break;
        };
        let raw_val = &rest[..close_idx];
        let local_key = local_tag_name(key);
        attrs.insert(local_key, unescape_xml(raw_val));
        s = &rest[close_idx + 1..];
    }
    attrs
}

fn unescape_xml(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        if let Some(semi) = tail.find(';') {
            let entity = &tail[1..semi];
            let replacement = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => {
                    entity[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(ch) = replacement {
                out.push(ch);
                rest = &tail[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[amp + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_FB_MPD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<MPD xmlns="urn:mpeg:dash:schema:mpd:2011" type="dynamic">
  <Period id="0">
    <AdaptationSet mimeType="video/mp4">
      <Representation id="dash-lp-pst-v" bandwidth="2500000" width="1280" height="720">
        <SegmentTemplate timescale="1000" initialization="../dash-lp-pst-v/init.mp4?a=1&amp;b=2" media="../dash-lp-pst-v/$Time$.m4v?a=1&amp;b=2">
          <SegmentTimeline>
            <S t="10000" d="2000" r="2"/>
            <S d="2000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
    <AdaptationSet mimeType="audio/mp4">
      <Representation id="dash-lp-md-a" bandwidth="128000">
        <SegmentTemplate timescale="1000" initialization="../dash-lp-md-a/init.mp4?tok=x&amp;y=1" media="../dash-lp-md-a/$Time$.m4a?tok=x&amp;y=1">
          <SegmentTimeline>
            <S t="10000" d="2000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;

    #[test]
    fn parses_facebook_dynamic_mpd_relative_urls_and_entities() {
        let mpd_url = "https://video.xx.fbcdn.net/v/t66/live-dash/1161684949951264.mpd";
        let parsed = parse_dash_representation_to_segments(SAMPLE_FB_MPD, mpd_url, "dash-lp-pst-v-2")
            .expect("should match dash-lp-pst-v from yt-dlp format_id dash-lp-pst-v-2");

        assert_eq!(
            parsed.init_url,
            "https://video.xx.fbcdn.net/v/t66/dash-lp-pst-v/init.mp4?a=1&b=2"
        );
        assert_eq!(parsed.target_duration, 2);
        assert_eq!(parsed.segments.len(), 4);
        assert_eq!(parsed.segments[0].start_tick, 10000);
        assert_eq!(
            parsed.segments[0].url,
            "https://video.xx.fbcdn.net/v/t66/dash-lp-pst-v/10000.m4v?a=1&b=2"
        );
        assert_eq!(parsed.segments[2].start_tick, 14000);
        assert_eq!(parsed.segments[3].start_tick, 16000);

        let m3u8 = format_hls_v7_playlist(&parsed, 7, false);
        assert!(m3u8.contains("#EXT-X-VERSION:7\n"));
        assert!(m3u8.contains("#EXT-X-MEDIA-SEQUENCE:7\n"));
        assert!(m3u8.contains("#EXT-X-MAP:URI=\"https://video.xx.fbcdn.net/v/t66/dash-lp-pst-v/init.mp4?a=1&b=2\"\n"));
        assert!(m3u8.contains("#EXTINF:2.000,\n"));
    }

    #[test]
    fn parses_audio_representation_exact_and_resolves_base_url() {
        let mpd = r#"<MPD>
  <BaseURL>https://cdn.example.com/live/sub/</BaseURL>
  <Period>
    <AdaptationSet>
      <Representation id="dash-lp-md-a">
        <SegmentTemplate timescale="48000" initialization="init.mp4" media="$Time$.m4a">
          <SegmentTimeline>
            <S t="480000" d="96000"/>
          </SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        let parsed = parse_dash_representation_to_segments(
            mpd,
            "https://other.example.com/manifest.mpd",
            "dash-lp-md-a",
        )
        .expect("exact rep id should match");
        assert_eq!(parsed.init_url, "https://cdn.example.com/live/sub/init.mp4");
        assert_eq!(parsed.segments[0].url, "https://cdn.example.com/live/sub/480000.m4a");
        assert!((parsed.segments[0].duration_secs - 2.0).abs() < 1e-6);
    }

    #[test]
    fn advances_media_sequence_monotonically_across_sliding_windows() {
        let mut entry = RelayEntry {
            kind: RelayKind::DashMpd {
                mpd_url: "https://example.com/live.mpd".into(),
                format_id: "dash-lp-pst-v".into(),
                last_times: Vec::new(),
                media_sequence: 0,
            },
            headers: vec![],
        };
        assert_eq!(advance_media_sequence(&mut entry, &[100, 200, 300, 400]), 0);
        // Same window (cache hit) keeps sequence 0
        assert_eq!(advance_media_sequence(&mut entry, &[100, 200, 300, 400]), 0);
        // Window slid forward by 2 segments
        assert_eq!(advance_media_sequence(&mut entry, &[300, 400, 500, 600]), 2);
        // Disjoint forward jump advances cleanly
        assert_eq!(advance_media_sequence(&mut entry, &[900, 1000, 1100]), 6);
    }

    #[test]
    fn identifies_youtube_dvr_hls_manifests_for_gzip_proxy() {
        assert!(should_proxy_hls_manifest(
            "https://manifest.googlevideo.com/api/manifest/hls_playlist/playlist_type/DVR/playlist/index.m3u8"
        ));
        assert!(should_proxy_hls_manifest(
            "https://cdn.example.com/live/manifest_duration/3600/index.m3u8"
        ));
        assert!(!should_proxy_hls_manifest(
            "http://127.0.0.1:8787/api/internal/dash-hls/abc123.m3u8"
        ));
        assert!(!should_proxy_hls_manifest(
            "https://pull-flv.tiktokcdn.com/stage/stream-123.flv"
        ));
    }

    #[test]
    fn trims_large_live_dvr_hls_playlist_to_live_edge() {
        let mut raw = String::from(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:5\n#EXT-X-MEDIA-SEQUENCE:2400\n#EXT-X-DISCONTINUITY-SEQUENCE:1\n",
        );
        for i in 0..25 {
            if i == 5 {
                raw.push_str("#EXT-X-DISCONTINUITY\n");
            }
            raw.push_str(&format!("#EXTINF:5.005,\nseg_{i}.ts\n"));
        }
        let trimmed = trim_hls_dvr_playlist(
            "https://manifest.googlevideo.com/api/manifest/hls_playlist/index.m3u8",
            &raw,
            12,
        );
        assert!(trimmed.contains("#EXT-X-MEDIA-SEQUENCE:2413\n"));
        assert!(trimmed.contains("#EXT-X-DISCONTINUITY-SEQUENCE:2\n"));
        assert!(!trimmed.contains("seg_12.ts"));
        assert!(trimmed.contains("https://manifest.googlevideo.com/api/manifest/hls_playlist/seg_13.ts"));
        assert!(trimmed.contains("https://manifest.googlevideo.com/api/manifest/hls_playlist/seg_24.ts"));
        assert_eq!(trimmed.matches("#EXTINF:").count(), 12);
    }

    #[test]
    fn leaves_short_or_vod_hls_playlists_unchanged() {
        let vod = "#EXTM3U\n#EXT-X-TARGETDURATION:5\n#EXTINF:5.0,\nseg0.ts\n#EXT-X-ENDLIST\n";
        assert_eq!(
            trim_hls_dvr_playlist("https://example.com/vod.m3u8", vod, 12),
            vod
        );
    }

    #[test]
    fn emits_hls_cue_out_when_dash_mpd_contains_scte35_ad_event() {
        let mpd = r#"<MPD>
  <BaseURL>https://cdn.example.com/live/</BaseURL>
  <Period id="main">
    <EventStream schemeIdUri="urn:scte:scte35:2013:xml"><Event id="10"/></EventStream>
    <AdaptationSet>
      <Representation id="v1">
        <SegmentTemplate timescale="1000" initialization="init.mp4" media="$Time$.m4v">
          <SegmentTimeline><S t="1000" d="2000"/></SegmentTimeline>
        </SegmentTemplate>
      </Representation>
    </AdaptationSet>
  </Period>
</MPD>"#;
        assert!(is_dash_mpd_ad_break(mpd));
        let parsed = parse_dash_representation_to_segments(mpd, "https://cdn.example.com/live.mpd", "v1").unwrap();
        let m3u8 = format_hls_v7_playlist(&parsed, 1, true);
        assert!(m3u8.contains("#EXT-X-CUE-OUT:30\n"));
        let clean = format_hls_v7_playlist(&parsed, 1, false);
        assert!(!clean.contains("#EXT-X-CUE-OUT"));
    }
}
