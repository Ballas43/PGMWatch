use crate::dash_relay::DashRelayState;
use crate::ghost_browser::{detect_msedge_path, export_cookies_from_auth_profile, sniff_media_via_edge_cdp};
use crate::win32::WindowsJobGuard;
use extractors::valid_bind_ip;
use pgmwatch_core::config::ExtractionConfig;
use pgmwatch_core::models::{FeedTier, Platform};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};

const RESOLVED_CACHE_TTL: Duration = Duration::from_secs(20 * 60);

pub fn auth_profile_slug(platform: Platform) -> &'static str {
    match platform {
        Platform::Youtube => "youtube_auth",
        Platform::Twitch => "twitch_auth",
        Platform::Facebook => "facebook_auth",
        Platform::Tiktok => "tiktok_auth",
        Platform::Bigo => "bigo_auth",
        Platform::Custom => "custom_auth",
    }
}

pub fn auth_cookies_slug(platform: Platform) -> &'static str {
    match platform {
        Platform::Youtube => "youtube_cookies.txt",
        Platform::Twitch => "twitch_cookies.txt",
        Platform::Facebook => "facebook_cookies.txt",
        Platform::Tiktok => "tiktok_cookies.txt",
        Platform::Bigo => "bigo_cookies.txt",
        Platform::Custom => "custom_cookies.txt",
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedMedia {
    pub stream_url: String,
    pub audio_url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub via: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("Login required or age-restricted stream: {0}")]
    LoginGated(String),
    #[error("Stream offline or unavailable: {0}")]
    Offline(String),
    #[error("Extractor error: {0}")]
    ExtractorFailed(String),
}

type ResolverCache = HashMap<(String, FeedTier, bool), (ResolvedMedia, Instant)>;

pub struct UrlResolver {
    semaphore: Arc<Semaphore>,
    cfg: RwLock<ExtractionConfig>,
    bind_ip: RwLock<String>,
    bin_dir: PathBuf,
    profiles_dir: PathBuf,
    job_guard: Arc<WindowsJobGuard>,
    dash_relay: Arc<DashRelayState>,
    http: reqwest::Client,
    cache: Mutex<ResolverCache>,
    cookie_sync_gate: Mutex<HashMap<Platform, Instant>>,
}


mod extractors;
mod live_check;
#[cfg(test)]
mod tests;

use extractors::{run_streamlink, run_yt_dlp, select_streamlink_quality, select_ytdlp_format};
pub(crate) use live_check::{classify_dash_manifest_vod, is_direct_stream_url};
use live_check::{direct_via_label, fetch_manifest_text, verify_resolved_is_live};

impl UrlResolver {
    pub fn new(
        cfg: ExtractionConfig,
        bin_dir: PathBuf,
        profiles_dir: PathBuf,
        job_guard: Arc<WindowsJobGuard>,
        dash_relay: Arc<DashRelayState>,
    ) -> Self {
        let permits = cfg.max_concurrent_extractors.max(1);
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(4))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            semaphore: Arc::new(Semaphore::new(permits)),
            cfg: RwLock::new(cfg),
            bind_ip: RwLock::new(String::new()),
            bin_dir,
            profiles_dir,
            job_guard,
            dash_relay,
            http,
            cache: Mutex::new(HashMap::new()),
            cookie_sync_gate: Mutex::new(HashMap::new()),
        }
    }

    pub async fn update_config(&self, cfg: ExtractionConfig) {
        *self.cfg.write().await = cfg;
        self.cache.lock().await.clear();
    }

    pub async fn set_bind_ip(&self, ip: &str) {
        let normalized = ip.trim().to_string();
        let mut guard = self.bind_ip.write().await;
        if *guard != normalized {
            *guard = normalized;
            drop(guard);
            self.cache.lock().await.clear();
        }
    }

    pub async fn invalidate_url(&self, url: &str) {
        let mut cache = self.cache.lock().await;
        cache.retain(|(k_url, _, _), _| k_url != url);
    }

    async fn sync_auth_cookies(&self, platform: Platform) -> Result<Option<PathBuf>, String> {
        let auth_profile_dir = self.profiles_dir.join(auth_profile_slug(platform));
        if !auth_profile_dir.join("Default").exists() {
            return Ok(None);
        }
        let msedge_cfg = self.cfg.read().await.msedge_path.clone();
        let Some(msedge) = detect_msedge_path(&msedge_cfg) else {
            return Ok(None);
        };
        let cookies_path = self.profiles_dir.join(auth_cookies_slug(platform));

        // Serialize headless Edge cookie exports so concurrent resolvers (e.g. 6 YouTube
        // channels on startup) never launch multiple Edge processes against the same profile
        // lockfile simultaneously, and reuse freshly synced cookies for 60s.
        let mut last_sync = self.cookie_sync_gate.lock().await;
        if let Some(prev) = last_sync.get(&platform)
            && prev.elapsed() < Duration::from_secs(60)
            && std::fs::metadata(&cookies_path).is_ok_and(|m| m.len() > 0)
        {
            return Ok(Some(cookies_path));
        }

        match export_cookies_from_auth_profile(
            &msedge,
            &auth_profile_dir,
            &cookies_path,
            Arc::clone(&self.job_guard),
        )
        .await
        {
            Ok(count) if count > 0 => {
                last_sync.insert(platform, Instant::now());
                Ok(Some(cookies_path))
            }
            Ok(_) => Ok(None),
            Err(err) => Err(err.to_string()),
        }
    }

    pub async fn resolve(
        &self,
        url: &str,
        platform: Platform,
        tier: FeedTier,
        live_streams_only: bool,
    ) -> Result<ResolvedMedia, ResolveError> {
        let ext_cfg = self.cfg.read().await.clone();
        if is_direct_stream_url(url) {
            let resolved = ResolvedMedia {
                stream_url: url.to_string(),
                audio_url: None,
                headers: Vec::new(),
                via: direct_via_label(url).to_string(),
            };
            verify_resolved_is_live(&self.http, &resolved, live_streams_only, false)
                .await?;
            return Ok(resolved);
        }

        if let Some((cached, ts)) = self
            .cache
            .lock()
            .await
            .get(&(url.to_string(), tier, live_streams_only))
            .cloned()
            && ts.elapsed() < RESOLVED_CACHE_TTL
        {
            return Ok(cached);
        }

        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|e| ResolveError::ExtractorFailed(e.to_string()))?;

        // Re-check cache after acquiring semaphore in case a peer task just populated it.
        if let Some((cached, ts)) = self
            .cache
            .lock()
            .await
            .get(&(url.to_string(), tier, live_streams_only))
            .cloned()
            && ts.elapsed() < RESOLVED_CACHE_TTL
        {
            return Ok(cached);
        }

        let bind_ip_raw = self.bind_ip.read().await.clone();
        let bind_ip = valid_bind_ip(Some(&bind_ip_raw));
        let cookies_path = self.profiles_dir.join(auth_cookies_slug(platform));
        let proactive_auth = ext_cfg.auth_mode.eq_ignore_ascii_case("fallback_auth");
        let mut cookies_file: Option<PathBuf> = if proactive_auth {
            if !cookies_path.exists() {
                if let Ok(Some(synced)) = self.sync_auth_cookies(platform).await {
                    Some(synced)
                } else {
                    None
                }
            } else {
                Some(cookies_path.clone())
            }
        } else {
            None
        };

        // For YouTube and Facebook, prefer yt-dlp first (handles live HLS and dynamic DASH MPDs via DashRelay);
        // for Twitch/TikTok/Bigo/etc., prefer Streamlink primary then yt-dlp.
        let mut last_offline: Option<String> = None;
        let mut confirmed_vod_offline: Option<String> = None;
        let mut login_err: Option<String> = None;

        enum TierTry {
            Resolved(ResolvedMedia),
            ConfirmedVodOffline(String),
            LoginGated(String),
            Offline(String),
            Fatal(ResolveError),
            Next,
        }

        let try_ytdlp = |cookies_file: &mut Option<PathBuf>| {
            let cookies_snapshot = cookies_file.clone();
            async move {
                let Some(yt_exe) = self.find_yt_dlp_exe() else {
                    return (TierTry::Next, cookies_snapshot);
                };
                let node_exe = self.bin_dir.join("node.exe");
                let format_expr = select_ytdlp_format(platform, tier);
                let mut cur_cookies = cookies_snapshot;
                match run_yt_dlp(
                    &yt_exe,
                    &node_exe,
                    url,
                    format_expr,
                    cur_cookies.as_deref(),
                    ext_cfg.enable_bgutil_pot,
                    ext_cfg.bgutil_pot_port,
                    bind_ip,
                    &self.dash_relay,
                    &self.job_guard,
                    live_streams_only,
                )
                .await
                {
                    Ok((resolved, confirmed_live)) => match verify_resolved_is_live(
                        &self.http,
                        &resolved,
                        live_streams_only,
                        confirmed_live,
                    )
                    .await
                    {
                        Ok(()) => (TierTry::Resolved(resolved), cur_cookies),
                        Err(ResolveError::Offline(msg)) => {
                            (TierTry::ConfirmedVodOffline(msg), cur_cookies)
                        }
                        Err(e) => (TierTry::Fatal(e), cur_cookies),
                    },
                    Err(ResolveError::LoginGated(msg)) => {
                        let mut sync_note = None;
                        match self.sync_auth_cookies(platform).await {
                            Ok(Some(synced)) => cur_cookies = Some(synced),
                            Ok(None) => {}
                            Err(err) => sync_note = Some(err),
                        }
                        if let Some(cf) = cur_cookies.as_deref() {
                            match run_yt_dlp(
                                &yt_exe,
                                &node_exe,
                                url,
                                format_expr,
                                Some(cf),
                                ext_cfg.enable_bgutil_pot,
                                ext_cfg.bgutil_pot_port,
                                bind_ip,
                                &self.dash_relay,
                                &self.job_guard,
                                live_streams_only,
                            )
                            .await
                            {
                                Ok((resolved, confirmed_live)) => match verify_resolved_is_live(
                                    &self.http,
                                    &resolved,
                                    live_streams_only,
                                    confirmed_live,
                                )
                                .await
                                {
                                    Ok(()) => return (TierTry::Resolved(resolved), cur_cookies),
                                    Err(e) => return (TierTry::Fatal(e), cur_cookies),
                                },
                                Err(ResolveError::Offline(msg)) => {
                                    return (TierTry::Fatal(ResolveError::Offline(msg)), cur_cookies);
                                }
                                Err(_) => {}
                            }
                        }
                        (TierTry::LoginGated(sync_note.unwrap_or(msg)), cur_cookies)
                    }
                    Err(ResolveError::Offline(msg)) => {
                        if msg.contains("live-only mode active") {
                            (TierTry::ConfirmedVodOffline(msg), cur_cookies)
                        } else {
                            (TierTry::Offline(msg), cur_cookies)
                        }
                    }
                    Err(_) => (TierTry::Next, cur_cookies),
                }
            }
        };

        let try_streamlink = || async {
            let Some(sl_exe) = self.find_streamlink_exe() else {
                return TierTry::Next;
            };
            let quality = select_streamlink_quality(platform, tier);
            match run_streamlink(&sl_exe, url, quality, bind_ip, &self.job_guard).await {
                Ok(resolved) => match verify_resolved_is_live(
                    &self.http,
                    &resolved,
                    live_streams_only,
                    false,
                )
                .await
                {
                    Ok(()) => TierTry::Resolved(resolved),
                    Err(ResolveError::Offline(msg)) => TierTry::ConfirmedVodOffline(msg),
                    Err(e) => TierTry::Fatal(e),
                },
                Err(ResolveError::LoginGated(msg)) => TierTry::LoginGated(msg),
                Err(ResolveError::Offline(msg)) => TierTry::Offline(msg),
                Err(_) => TierTry::Next,
            }
        };

        let result: Result<ResolvedMedia, ResolveError> = async {
            let order = if matches!(platform, Platform::Youtube | Platform::Facebook) {
                [true, false] // yt-dlp first, then Streamlink
            } else {
                [false, true] // Streamlink first, then yt-dlp
            };
            for use_ytdlp in order {
                if confirmed_vod_offline.is_some() {
                    break;
                }
                if !use_ytdlp && platform == Platform::Youtube && login_err.is_some() {
                    break;
                }
                let outcome = if use_ytdlp {
                    let (out, updated_cookies) = try_ytdlp(&mut cookies_file).await;
                    cookies_file = updated_cookies;
                    out
                } else {
                    try_streamlink().await
                };
                match outcome {
                    TierTry::Resolved(resolved) => return Ok(resolved),
                    TierTry::ConfirmedVodOffline(msg) => confirmed_vod_offline = Some(msg),
                    TierTry::LoginGated(msg) => login_err = Some(msg),
                    TierTry::Offline(msg) => {
                        if last_offline.is_none() {
                            last_offline = Some(msg);
                        }
                    }
                    TierTry::Fatal(err) => return Err(err),
                    TierTry::Next => {}
                }
            }

            if let Some(msg) = confirmed_vod_offline {
                return Err(ResolveError::Offline(msg));
            }

            // Facebook Comet SSR fallback: newer Facebook Live pages set `videoDeliveryLegacyFields: null`
            // and place the live `.mpd` in `videoDeliveryResponseFragment.videoDeliveryResponseResult.dash_manifest_urls`,
            // which yt-dlp currently misses ("No video found"). Extract and relay it directly.
            if platform == Platform::Facebook
                && let Some(fb_res) = self
                    .try_resolve_facebook_dash_ssr(url, tier, live_streams_only)
                    .await
            {
                return fb_res;
            }

            if let Some(msg) = login_err {
                return Err(ResolveError::LoginGated(msg));
            }
            if let Some(msg) = last_offline {
                return Err(ResolveError::Offline(msg));
            }

            // Tier 2: Windows Edge CDP Ghost Browser Mode A
            if let Some(msedge) = detect_msedge_path(&ext_cfg.msedge_path) {
                let anon_dir = self.profiles_dir.join("anon_ephemeral");
                if let Ok(sniffed) =
                    sniff_media_via_edge_cdp(&msedge, &anon_dir, url, Arc::clone(&self.job_guard)).await
                {
                    let resolved = ResolvedMedia {
                        stream_url: sniffed.stream_url,
                        audio_url: None,
                        headers: sniffed.headers,
                        via: "edge-cdp".to_string(),
                    };
                    verify_resolved_is_live(&self.http, &resolved, live_streams_only, false)
                        .await?;
                    return Ok(resolved);
                }
            }

            Err(ResolveError::ExtractorFailed(
                "Streamlink, yt-dlp, and Edge CDP could not resolve a playable stream URL".to_string(),
            ))
        }.await;

        let mut result = result;
        if let Ok(ref mut media) = result {
            let proxy_v = crate::dash_relay::should_proxy_hls_manifest(&media.stream_url);
            let proxy_a = media
                .audio_url
                .as_deref()
                .is_some_and(crate::dash_relay::should_proxy_hls_manifest);
            if proxy_v || proxy_a {
                self.dash_relay.clear_source(url).await;
                if proxy_v {
                    media.stream_url = self
                        .dash_relay
                        .register_hls_proxy(url, media.stream_url.clone(), media.headers.clone())
                        .await;
                }
                if proxy_a && let Some(ref au) = media.audio_url {
                    media.audio_url = Some(
                        self.dash_relay
                            .register_hls_proxy(url, au.clone(), media.headers.clone())
                            .await,
                    );
                }
            }
            self.cache.lock().await.insert(
                (url.to_string(), tier, live_streams_only),
                (media.clone(), Instant::now()),
            );
        }
        result
    }

    fn find_streamlink_exe(&self) -> Option<PathBuf> {
        let candidates = [
            self.bin_dir.join("streamlink").join("bin").join("streamlink.exe"),
            self.bin_dir.join("streamlink").join("streamlink.exe"),
        ];
        candidates.into_iter().find(|p| p.exists())
    }

    fn find_yt_dlp_exe(&self) -> Option<PathBuf> {
        let p = self.bin_dir.join("yt-dlp.exe");
        if p.exists() {
            Some(p)
        } else {
            None
        }
    }

    async fn try_resolve_facebook_dash_ssr(
        &self,
        url: &str,
        tier: FeedTier,
        live_streams_only: bool,
    ) -> Option<Result<ResolvedMedia, ResolveError>> {
        let fb_cookies_path = self.profiles_dir.join(auth_cookies_slug(Platform::Facebook));
        let cookie_hdr = crate::stats::load_cookie_header_for_domain(&fb_cookies_path, "facebook.com");
        let attempts: Vec<Option<&str>> = if cookie_hdr.as_deref().is_some_and(|s| !s.is_empty()) {
            vec![cookie_hdr.as_deref(), None]
        } else {
            vec![None]
        };

        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";
        let mut mpd_url: Option<String> = None;
        for cookie_opt in attempts {
            let mut req = self
                .http
                .get(url)
                .header("User-Agent", ua)
                .header(
                    "Accept",
                    "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                )
                .header("Accept-Language", "en-US,en;q=0.9")
                .header("Sec-Fetch-Mode", "navigate");
            if let Some(c) = cookie_opt {
                req = req.header("Cookie", c);
            }
            let Ok(resp) = req.send().await else { continue };
            if !resp.status().is_success() {
                continue;
            }
            let content_len = resp.content_length();
            let Ok(html) = resp.text().await else { continue };
            crate::stats::record_http_rx_bytes(content_len, html.len());
            if let Some(found) = extract_facebook_dash_manifest_url(&html, url) {
                mpd_url = Some(found);
                break;
            }
        }

        let mpd_url = mpd_url?;
        let headers = vec![("User-Agent".to_string(), ua.to_string())];
        let xml = fetch_manifest_text(&self.http, &mpd_url, &headers).await?;
        if live_streams_only
            && let Some(reason) = classify_dash_manifest_vod(&xml)
        {
            return Some(Err(ResolveError::Offline(format!(
                "Stream offline ({reason} — live-only mode active)"
            ))));
        }

        let (max_h, audio_only) = match tier {
            FeedTier::Master => (None, false),
            FeedTier::Simulcast => (Some(480), false),
            FeedTier::Light => (None, true),
        };
        let (vid_id, aud_id) =
            crate::dash_relay::pick_dash_video_audio_rep_ids(&xml, max_h, audio_only);
        let primary_id = vid_id?;

        self.dash_relay.clear_source(url).await;
        let stream_url = self
            .dash_relay
            .register(url, mpd_url.clone(), primary_id, headers.clone())
            .await;
        let audio_url = match aud_id {
            Some(aid) if !audio_only => Some(
                self.dash_relay
                    .register(url, mpd_url, aid, headers.clone())
                    .await,
            ),
            _ => None,
        };

        Some(Ok(ResolvedMedia {
            stream_url,
            audio_url,
            headers,
            via: "fb-dash-ssr".to_string(),
        }))
    }
}

fn extract_fb_video_id_from_url(url: &str) -> Option<&str> {
    let lower = url.to_ascii_lowercase();
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

fn extract_facebook_dash_manifest_url(html: &str, page_url: &str) -> Option<String> {
    let target_vid = extract_fb_video_id_from_url(page_url);
    let mut first_valid: Option<String> = None;

    // Facebook A/B-serves two SSR JSON layouts:
    // 1. Comet: `"dash_manifest_urls":[{"manifest_url":"https:\/\/...mpd..."}]`
    // 2. Legacy: `"dash_manifest_url":"https:\/\/...mpd..."`
    for (anchor_pat, val_key) in [
        ("\"dash_manifest_urls\"", "\"manifest_url\":\""),
        ("\"dash_manifest_url\":\"", "\"dash_manifest_url\":\""),
    ] {
        let mut offset = 0usize;
        while let Some(rel) = html[offset..].find(anchor_pat) {
            let anchor = offset + rel;
            let win_end = html.ceil_char_boundary((anchor + 4096).min(html.len()));
            let window = &html[anchor..win_end];
            if let Some(kpos) = window.find(val_key) {
                let start = kpos + val_key.len();
                let rest = &window[start..];
                if let Some(end) = rest.find('"') {
                    let raw = &rest[..end];
                    let unescaped = raw
                        .replace("\\/", "/")
                        .replace("\\u0026", "&")
                        .replace("&amp;", "&");
                    if unescaped.starts_with("http") && unescaped.contains(".mpd") {
                        if let Some(vid) = target_vid {
                            let ctx_start = html.floor_char_boundary(anchor.saturating_sub(2048));
                            let ctx = &html[ctx_start..win_end];
                            if ctx.contains(vid) || unescaped.contains(vid) {
                                return Some(unescaped);
                            }
                        } else {
                            return Some(unescaped);
                        }
                        if first_valid.is_none() {
                            first_valid = Some(unescaped);
                        }
                    }
                }
            }
            offset = anchor + anchor_pat.len();
        }
    }
    first_valid
}

