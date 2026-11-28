use chrono::{DateTime, Duration, Utc};
use pgmwatch_core::models::{MetricSource, MetricStatus, Platform, TriStateMetric};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

mod facebook;
mod tiktok;
mod youtube_twitch;
#[cfg(test)]
mod tests;

use facebook::fetch_facebook_public_stats;
pub(crate) use tiktok::extract_tiktok_username;
pub use tiktok::{SidecarEvent, TikTokSidecarClient};
use tiktok::fetch_tiktok_public_stats;
pub(crate) use youtube_twitch::extract_twitch_login;
use youtube_twitch::{fetch_twitch_public_stats, fetch_youtube_public_stats};

static SUPERVISOR_HTTP_RX_BYTES: AtomicU64 = AtomicU64::new(0);

/// Records wire-level HTTP receive bytes from supervisor background tasks (DashRelay manifests,
/// YouTube/Facebook/TikTok HTML scrapes, and live chat polling) into the governor's WAN counter.
pub fn record_http_rx_bytes(content_length: Option<u64>, body_len: usize) {
    let wire_bytes = match content_length {
        Some(n) if n > 0 => n,
        // Chunked gzip responses omit Content-Length; HTML/JSON/HLS typically compress ~4:1.
        _ => (body_len as u64 / 4).max(body_len.min(512) as u64),
    };
    SUPERVISOR_HTTP_RX_BYTES.fetch_add(wire_bytes, Ordering::Relaxed);
}

pub fn take_http_rx_bytes() -> u64 {
    SUPERVISOR_HTTP_RX_BYTES.swap(0, Ordering::Relaxed)
}

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

