use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Client;
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
const RELAY_IDLE_EVICT_TTL: Duration = Duration::from_secs(120);

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
    last_accessed: Instant,
}


mod hls_proxy;
mod mpd_parser;
mod xml;
#[cfg(test)]
mod tests;

pub use hls_proxy::{should_proxy_hls_manifest, trim_hls_dvr_playlist};
pub use mpd_parser::pick_dash_video_audio_rep_ids;
use mpd_parser::{format_hls_v7_playlist, is_dash_mpd_ad_break, parse_dash_representation_to_segments};

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

    /// Prunes idle relay tokens (unused for >120s) for `source_url` without evicting active tokens
    /// still being polled by another channel that shares the same stream URL.
    pub async fn clear_source(&self, source_url: &str) {
        let mut by_src = self.by_source.lock().await;
        let Some(tokens) = by_src.get_mut(source_url) else {
            return;
        };
        let mut keep = Vec::with_capacity(tokens.len());
        let mut evict = Vec::new();
        {
            let entries = self.entries.read().await;
            for tok in tokens.drain(..) {
                if let Some(arc) = entries.get(&tok) {
                    let idle = arc.lock().await.last_accessed.elapsed() > RELAY_IDLE_EVICT_TTL;
                    if idle {
                        evict.push(tok);
                    } else {
                        keep.push(tok);
                    }
                }
            }
        }
        if keep.is_empty() {
            by_src.remove(source_url);
        } else {
            *tokens = keep;
        }
        drop(by_src);
        if !evict.is_empty() {
            let mut entries = self.entries.write().await;
            for tok in evict {
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
            last_accessed: Instant::now(),
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
            last_accessed: Instant::now(),
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
            let mut e = entry_arc.lock().await;
            e.last_accessed = Instant::now();
            (e.kind.clone(), e.headers.clone())
        };

        match kind {
            RelayKind::HlsProxy { playlist_url } => {
                let cache_key = format!("hls-trimmed:{playlist_url}");
                {
                    let cache = self.mpd_cache.lock().await;
                    if let Some(hit) = cache.get(&cache_key)
                        && hit.fetched_at.elapsed() < HLS_PROXY_CACHE_TTL
                    {
                        return Ok(hit.xml.clone());
                    }
                }
                let raw_m3u8 = self
                    .fetch_manifest_cached(&playlist_url, &headers, HLS_PROXY_CACHE_TTL)
                    .await?;
                let trimmed = trim_hls_dvr_playlist(
                    &playlist_url,
                    &raw_m3u8,
                    HLS_DVR_KEEP_SEGMENTS,
                );
                self.mpd_cache.lock().await.insert(
                    cache_key,
                    CachedMpd {
                        xml: trimmed.clone(),
                        fetched_at: Instant::now(),
                    },
                );
                Ok(trimmed)
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
