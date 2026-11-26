use crate::dash_relay::DashRelayState;
use crate::ghost_browser::{detect_msedge_path, export_cookies_from_auth_profile, sniff_media_via_edge_cdp};
use crate::win32::WindowsJobGuard;
use pgmwatch_core::config::ExtractionConfig;
use pgmwatch_core::models::{FeedTier, Platform};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;
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
}

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

async fn fetch_manifest_text(
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

async fn verify_resolved_is_live(
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

fn direct_via_label(url: &str) -> &'static str {
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
        match export_cookies_from_auth_profile(
            &msedge,
            &auth_profile_dir,
            &cookies_path,
            Arc::clone(&self.job_guard),
        )
        .await
        {
            Ok(count) if count > 0 => Ok(Some(cookies_path)),
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

        // For YouTube, prefer yt-dlp first (handles both live HLS and DASH/VOD with HTTP headers);
        // for Twitch/TikTok/Facebook/etc., prefer Streamlink primary then yt-dlp.
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
            match run_streamlink(&sl_exe, url, quality, bind_ip).await {
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
            let order = if platform == Platform::Youtube {
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

        if let Ok(ref media) = result {
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
}

fn select_streamlink_quality(platform: Platform, tier: FeedTier) -> &'static str {
    match (platform, tier) {
        (_, FeedTier::Master) => "best",
        // YouTube HLS hard floor: never below 360p (itag 93) so AAC-LC stays >=128kbps
        (Platform::Youtube, FeedTier::Simulcast) => "480p,360p,best",
        (Platform::Youtube, FeedTier::Light) => "audio_only,360p,480p",
        (Platform::Tiktok | Platform::Bigo, _) => "origin,hd,best",
        _ => "480p,360p,best",
    }
}

fn select_ytdlp_format(platform: Platform, tier: FeedTier) -> &'static str {
    match (platform, tier) {
        (Platform::Youtube, FeedTier::Master) => {
            "bestvideo[vcodec^=avc1]+bestaudio[acodec^=mp4a]/bestvideo+bestaudio/best"
        }
        (_, FeedTier::Master) => "bestvideo+bestaudio/best",
        // DASH paired <=480p video + bestaudio; muxed fallback floor >=360p
        (Platform::Youtube | Platform::Facebook, FeedTier::Simulcast) => {
            "bestvideo[height<=480][vcodec^=avc1]+bestaudio[acodec^=mp4a]/bestvideo[height<=480]+bestaudio/best[height>=360]/best"
        }
        (_, FeedTier::Light) => "bestaudio/best[height>=360]/best",
        _ => "best[height<=480]/best",
    }
}

fn sanitize_http_headers(map: Option<&serde_json::Map<String, serde_json::Value>>) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(obj) = map else { return out };
    for (k, v) in obj {
        if out.len() >= 12 {
            break;
        }
        let kl = k.to_ascii_lowercase();
        if !matches!(
            kl.as_str(),
            "user-agent" | "referer" | "origin" | "accept" | "accept-language"
        ) {
            continue;
        }
        if let Some(s) = v.as_str() {
            let clean: String = s.chars().filter(|c| *c != '\r' && *c != '\n' && *c != '\0').take(1024).collect();
            if !clean.is_empty() {
                out.push((k.clone(), clean));
            }
        }
    }
    out
}

fn valid_bind_ip(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && s.parse::<std::net::IpAddr>().is_ok())
}

fn build_streamlink_args(url: &str, quality: &str, bind_ip: Option<&str>) -> Vec<String> {
    let mut args = vec!["--stream-url".to_string()];
    if let Some(ip) = valid_bind_ip(bind_ip) {
        args.push("--interface".to_string());
        args.push(ip.to_string());
    }
    args.push(url.to_string());
    args.push(quality.to_string());
    args
}

async fn run_streamlink(
    exe: &Path,
    url: &str,
    quality: &str,
    bind_ip: Option<&str>,
) -> Result<ResolvedMedia, ResolveError> {
    let args = build_streamlink_args(url, quality, bind_ip);
    use pgmwatch_core::process::BackgroundCommandExt;
    let mut cmd = Command::new(exe);
    cmd.no_window()
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = cmd
        .output()
        .await
        .map_err(|e| ResolveError::ExtractorFailed(e.to_string()))?;

    if output.status.success() {
        let stream_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stream_url.starts_with("http") {
            return Ok(ResolvedMedia {
                stream_url,
                audio_url: None,
                headers: Vec::new(),
                via: "streamlink".to_string(),
            });
        }
    }

    let err = String::from_utf8_lossy(&output.stderr).to_string();
    let lower = err.to_ascii_lowercase();
    if lower.contains("login") || lower.contains("age") {
        Err(ResolveError::LoginGated(err))
    } else if lower.contains("no playable streams") {
        Err(ResolveError::Offline(err))
    } else {
        Err(ResolveError::ExtractorFailed(err))
    }
}

pub(crate) fn build_yt_extractor_args(
    base_client: Option<&str>,
    enable_bgutil_pot: bool,
    bgutil_pot_port: u16,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(base) = base_client.filter(|s| !s.trim().is_empty()) {
        parts.push(base.trim().to_string());
    }
    if enable_bgutil_pot && bgutil_pot_port > 0 {
        parts.push(format!(
            "youtubepot-bgutilhttp:base_url=http://127.0.0.1:{bgutil_pot_port}"
        ));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(";"))
    }
}

const YT_AV_FORMAT_EXPR: &str = "best[vcodec^=avc1][fps>=50]/best[height<=1080][vcodec^=avc1]/best[height<=1080]/bestvideo[vcodec^=avc1][fps>=50]+bestaudio[acodec^=mp4a]/bestvideo[vcodec^=avc1][fps>=50]+bestaudio/bestvideo[vcodec^=avc1]+bestaudio/bestvideo+bestaudio/best";

fn yt_format_for(format_expr: &str, prefer_short_audio_only: bool) -> &str {
    if format_expr.contains("bestaudio") && !format_expr.contains("bestvideo") {
        if prefer_short_audio_only {
            "bestaudio/best"
        } else {
            format_expr
        }
    } else {
        YT_AV_FORMAT_EXPR
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_yt_dlp(
    exe: &Path,
    node_exe: &Path,
    url: &str,
    format_expr: &str,
    cookies_file: Option<&Path>,
    enable_bgutil_pot: bool,
    bgutil_pot_port: u16,
    bind_ip: Option<&str>,
    dash_relay: &DashRelayState,
    live_streams_only: bool,
) -> Result<(ResolvedMedia, bool), ResolveError> {
    let lower_url = url.to_ascii_lowercase();
    let is_yt = lower_url.contains("youtube.com") || lower_url.contains("youtu.be");
    if is_yt && cookies_file.is_none() {
        // tv_simply avoids PO-Token/SABR 403 Forbidden errors on live YouTube HLS (itag 96/301) & progressive MP4
        let tv_fmt = yt_format_for(format_expr, true);
        let tv_ea = build_yt_extractor_args(
            Some("youtube:player_client=tv_simply"),
            enable_bgutil_pot,
            bgutil_pot_port,
        );
        match run_yt_dlp_once(
            exe,
            node_exe,
            url,
            tv_fmt,
            tv_ea.as_deref(),
            None,
            bind_ip,
            dash_relay,
            live_streams_only,
        )
        .await
        {
            Ok(ok) => return Ok(ok),
            Err(ResolveError::Offline(msg)) if msg.contains("live-only mode active") => {
                return Err(ResolveError::Offline(msg));
            }
            Err(_) => {}
        }
    }
    let yt_fmt = if is_yt {
        yt_format_for(format_expr, false)
    } else {
        format_expr
    };
    let default_ea = if is_yt {
        build_yt_extractor_args(None, enable_bgutil_pot, bgutil_pot_port)
    } else {
        None
    };
    run_yt_dlp_once(
        exe,
        node_exe,
        url,
        yt_fmt,
        default_ea.as_deref(),
        cookies_file,
        bind_ip,
        dash_relay,
        live_streams_only,
    )
    .await
}

fn is_present_codec(val: Option<&serde_json::Value>) -> bool {
    matches!(
        val.and_then(|v| v.as_str()),
        Some(s) if !s.is_empty() && !s.eq_ignore_ascii_case("none")
    )
}

fn select_ytdlp_formats(
    req_fmts: &[serde_json::Value],
) -> (Option<&serde_json::Value>, Option<&serde_json::Value>) {
    let vid = req_fmts
        .iter()
        .find(|f| is_present_codec(f.get("vcodec")))
        .or_else(|| req_fmts.first());

    let aud = req_fmts
        .iter()
        .find(|f| is_present_codec(f.get("acodec")))
        .or_else(|| {
            // Live YouTube HLS audio formats (e.g. itag 234) omit `acodec` (`null`)
            // while identifying the track via `audio_ext` or `"audio only"` resolution.
            req_fmts.iter().find(|f| {
                if vid.is_some_and(|v| std::ptr::eq(*f, v)) {
                    return false;
                }
                let has_audio_ext = is_present_codec(f.get("audio_ext"));
                let audio_res = f
                    .get("resolution")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_ascii_lowercase().contains("audio"))
                    .unwrap_or(false);
                let paired_non_video = req_fmts.len() >= 2 && !is_present_codec(f.get("vcodec"));
                has_audio_ext || audio_res || paired_non_video
            })
        });

    (vid, aud)
}

async fn resolve_ytdlp_format_url(
    source_url: &str,
    fmt: &serde_json::Value,
    fallback_headers: &[(String, String)],
    dash_relay: &DashRelayState,
) -> Option<String> {
    let protocol = fmt.get("protocol").and_then(|v| v.as_str()).unwrap_or("");
    if protocol == "http_dash_segments" {
        let mpd_url = fmt
            .get("manifest_url")
            .and_then(|v| v.as_str())
            .or_else(|| fmt.get("url").and_then(|v| v.as_str()))
            .filter(|s| s.starts_with("http"))?;
        let format_id = fmt.get("format_id").and_then(|v| v.as_str()).unwrap_or("");
        let fmt_headers =
            sanitize_http_headers(fmt.get("http_headers").and_then(|h| h.as_object()));
        let hdrs = if fmt_headers.is_empty() {
            fallback_headers.to_vec()
        } else {
            fmt_headers
        };
        return Some(
            dash_relay
                .register(source_url, mpd_url.to_string(), format_id.to_string(), hdrs)
                .await,
        );
    }
    fmt.get("url")
        .and_then(|v| v.as_str())
        .filter(|s| s.starts_with("http"))
        .map(String::from)
}

fn build_yt_dlp_args(
    url: &str,
    format_expr: &str,
    extractor_args: Option<&str>,
    cookies_file: Option<&Path>,
    node_exe: Option<&Path>,
    bind_ip: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "--no-playlist".to_string(),
        "--dump-single-json".to_string(),
        "--no-warnings".to_string(),
        "-f".to_string(),
        format_expr.to_string(),
    ];
    if let Some(ea) = extractor_args {
        args.push("--extractor-args".to_string());
        args.push(ea.to_string());
    }
    if let Some(cf) = cookies_file {
        args.push("--cookies".to_string());
        args.push(cf.display().to_string());
    }
    if let Some(ne) = node_exe.filter(|p| p.exists()) {
        args.push("--js-runtimes".to_string());
        args.push(format!("node:{}", ne.display()));
    }
    if let Some(ip) = valid_bind_ip(bind_ip) {
        args.push("--source-address".to_string());
        args.push(ip.to_string());
    }
    args.push(url.to_string());
    args
}

#[allow(clippy::too_many_arguments)]
async fn run_yt_dlp_once(
    exe: &Path,
    node_exe: &Path,
    url: &str,
    format_expr: &str,
    extractor_args: Option<&str>,
    cookies_file: Option<&Path>,
    bind_ip: Option<&str>,
    dash_relay: &DashRelayState,
    live_streams_only: bool,
) -> Result<(ResolvedMedia, bool), ResolveError> {
    let args = build_yt_dlp_args(
        url,
        format_expr,
        extractor_args,
        cookies_file,
        Some(node_exe),
        bind_ip,
    );
    use pgmwatch_core::process::BackgroundCommandExt;
    let mut cmd = Command::new(exe);
    cmd.no_window()
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = cmd
        .output()
        .await
        .map_err(|e| ResolveError::ExtractorFailed(e.to_string()))?;

    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
            let confirmed_live = match check_ytdlp_live_status(&json) {
                Ok(v) => v,
                Err(reason) if live_streams_only => {
                    return Err(ResolveError::Offline(reason));
                }
                Err(_) => false,
            };
            if let Some(req_fmts) = json.get("requested_formats").and_then(|v| v.as_array()) {
                let (vid, aud) = select_ytdlp_formats(req_fmts);
                if let Some(vid_fmt) = vid {
                    let headers = sanitize_http_headers(
                        vid_fmt
                            .get("http_headers")
                            .and_then(|h| h.as_object())
                            .or_else(|| json.get("http_headers").and_then(|h| h.as_object())),
                    );
                    let uses_dash = [Some(vid_fmt), aud].into_iter().flatten().any(|f| {
                        f.get("protocol").and_then(|p| p.as_str()) == Some("http_dash_segments")
                    });
                    if uses_dash {
                        dash_relay.clear_source(url).await;
                    }
                    if let Some(v_url) =
                        resolve_ytdlp_format_url(url, vid_fmt, &headers, dash_relay).await
                    {
                        let a_url = if let Some(aud_fmt) = aud {
                            resolve_ytdlp_format_url(url, aud_fmt, &headers, dash_relay)
                                .await
                                .filter(|s| s != &v_url)
                        } else {
                            None
                        };
                        return Ok((
                            ResolvedMedia {
                                stream_url: v_url,
                                audio_url: a_url,
                                headers,
                                via: "yt-dlp".to_string(),
                            },
                            confirmed_live,
                        ));
                    }
                }
            }
            let headers = sanitize_http_headers(
                json.get("http_headers").and_then(|h| h.as_object()),
            );
            if json.get("protocol").and_then(|p| p.as_str()) == Some("http_dash_segments") {
                dash_relay.clear_source(url).await;
            }
            if let Some(direct_url) =
                resolve_ytdlp_format_url(url, &json, &headers, dash_relay).await
            {
                return Ok((
                    ResolvedMedia {
                        stream_url: direct_url,
                        audio_url: None,
                        headers,
                        via: "yt-dlp".to_string(),
                    },
                    confirmed_live,
                ));
            }
        }
    }

    let err = String::from_utf8_lossy(&output.stderr).to_string();
    let lower = err.to_ascii_lowercase();
    if lower.contains("sign in") || lower.contains("login") || lower.contains("age") || lower.contains("cookies") {
        Err(ResolveError::LoginGated(err))
    } else if lower.contains("offline")
        || lower.contains("not currently live")
        || lower.contains("premieres in")
        || lower.contains("live event will begin")
        || lower.contains("video unavailable")
    {
        Err(ResolveError::Offline(err))
    } else {
        Err(ResolveError::ExtractorFailed(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_url_resolver_cache_and_invalidate() {
        let guard = Arc::new(WindowsJobGuard::new(512).expect("job guard"));
        let dash_relay = Arc::new(DashRelayState::new(8787));
        let resolver = UrlResolver::new(
            ExtractionConfig {
                auth_mode: "anonymous".into(),
                max_concurrent_extractors: 2,
                enable_bgutil_pot: false,
                bgutil_pot_port: 4416,
                msedge_path: String::new(),
            },
            PathBuf::from("."),
            PathBuf::from("."),
            guard,
            dash_relay,
        );
        let url = "https://youtube.com/watch?v=test_cache";
        resolver.cache.lock().await.insert(
            (url.to_string(), FeedTier::Master, true),
            (
                ResolvedMedia {
                    stream_url: "https://manifest.example/cached.m3u8".into(),
                    audio_url: None,
                    headers: vec![],
                    via: "yt-dlp".into(),
                },
                Instant::now(),
            ),
        );
        let hit = resolver.resolve(url, Platform::Youtube, FeedTier::Master, true).await.unwrap();
        assert_eq!(hit.stream_url, "https://manifest.example/cached.m3u8");
        resolver.invalidate_url(url).await;
        assert!(resolver.cache.lock().await.is_empty());
    }

    #[test]
    fn test_build_yt_extractor_args() {
        assert_eq!(build_yt_extractor_args(None, false, 4416), None);
        assert_eq!(
            build_yt_extractor_args(Some("youtube:player_client=tv_simply"), false, 4416).as_deref(),
            Some("youtube:player_client=tv_simply")
        );
        assert_eq!(
            build_yt_extractor_args(None, true, 4416).as_deref(),
            Some("youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416")
        );
        assert_eq!(
            build_yt_extractor_args(Some("youtube:player_client=tv_simply"), true, 4499).as_deref(),
            Some("youtube:player_client=tv_simply;youtubepot-bgutilhttp:base_url=http://127.0.0.1:4499")
        );
    }

    #[test]
    fn test_select_ytdlp_formats_pairs_null_acodec_hls_audio() {
        let paired = vec![
            serde_json::json!({
                "format_id": "312",
                "vcodec": "avc1.4D402A",
                "acodec": "none",
                "audio_ext": "none",
                "resolution": "1920x1080"
            }),
            serde_json::json!({
                "format_id": "234",
                "vcodec": "none",
                "acodec": null,
                "audio_ext": "mp4",
                "resolution": "audio only"
            }),
        ];
        let (vid, aud) = select_ytdlp_formats(&paired);
        assert_eq!(vid.and_then(|v| v["format_id"].as_str()), Some("312"));
        assert_eq!(aud.and_then(|a| a["format_id"].as_str()), Some("234"));

        // Single video-only track with null acodec must not self-pair as audio.
        let single_vid = vec![serde_json::json!({
            "format_id": "312",
            "vcodec": "avc1.4D402A",
            "acodec": null,
            "audio_ext": "none",
            "resolution": "1920x1080"
        })];
        let (vid2, aud2) = select_ytdlp_formats(&single_vid);
        assert_eq!(vid2.and_then(|v| v["format_id"].as_str()), Some("312"));
        assert!(aud2.is_none());
    }

    #[test]
    fn test_direct_stream_urls_and_outbound_bind_args() {
        assert!(is_direct_stream_url("rtmp://ingest.local/app/key"));
        assert!(is_direct_stream_url("rtmps://ingest.local/app/key"));
        assert!(is_direct_stream_url("srt://192.168.10.20:9000?mode=caller"));
        assert!(is_direct_stream_url("https://cdn.example.com/live/master.m3u8?token=abc"));
        assert!(!is_direct_stream_url("https://www.youtube.com/watch?v=1gilXpZSyKo"));

        let sl_args = build_streamlink_args("https://twitch.tv/test", "best", Some("192.168.10.50"));
        assert!(sl_args.windows(2).any(|w| w == ["--interface", "192.168.10.50"]));

        let yt_args = build_yt_dlp_args(
            "https://youtube.com/watch?v=abc",
            "best",
            None,
            None,
            None,
            Some("192.168.10.50"),
        );
        assert!(yt_args.windows(2).any(|w| w == ["--source-address", "192.168.10.50"]));
    }

    #[test]
    fn test_live_vs_vod_detection_helpers() {
        assert_eq!(
            check_ytdlp_live_status(&serde_json::json!({ "live_status": "is_live", "is_live": true })),
            Ok(true)
        );
        assert!(check_ytdlp_live_status(&serde_json::json!({ "live_status": "was_live" })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "live_status": "not_live" })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "live_status": "is_upcoming" })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "is_live": false })).is_err());
        assert!(check_ytdlp_live_status(&serde_json::json!({ "was_live": true })).is_err());
        assert_eq!(check_ytdlp_live_status(&serde_json::json!({})), Ok(false));

        assert!(classify_hls_manifest_vod("#EXTM3U\n#EXTINF:2.0,\nseg0.ts\n#EXT-X-ENDLIST\n").is_some());
        assert!(classify_hls_manifest_vod("#EXTM3U\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXTINF:2.0,\nseg0.ts\n").is_some());
        assert!(classify_hls_manifest_vod("#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:100\n#EXTINF:2.0,\nseg100.ts\n").is_none());
        assert_eq!(
            first_hls_variant_url(
                "https://cdn.example.com/live/master.m3u8",
                "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=1000000\n720p/index.m3u8\n"
            ).as_deref(),
            Some("https://cdn.example.com/live/720p/index.m3u8")
        );

        assert!(classify_dash_manifest_vod("<MPD type=\"static\" mediaPresentationDuration=\"PT30M\">").is_some());
        assert!(classify_dash_manifest_vod("<MPD type=\"dynamic\" minimumUpdatePeriod=\"PT2S\">").is_none());

        assert!(is_static_vod_media_url("https://cdn.example.com/video/replay.mp4?token=1"));
        assert!(!is_static_vod_media_url("https://cdn.example.com/live/stream.flv"));
        assert!(!is_static_vod_media_url("https://cdn.example.com/live/master.m3u8"));
        assert!(is_always_live_transport_url("sim://channel_1"));
        assert!(is_always_live_transport_url("rtmp://ingest.local/app/key"));
    }
}
