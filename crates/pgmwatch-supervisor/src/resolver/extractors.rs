use super::live_check::check_ytdlp_live_status;
use super::{ResolveError, ResolvedMedia};
use crate::dash_relay::DashRelayState;
use crate::win32::WindowsJobGuard;
use pgmwatch_core::models::{FeedTier, Platform};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

pub(super) fn select_streamlink_quality(platform: Platform, tier: FeedTier) -> &'static str {
    match (platform, tier) {
        (_, FeedTier::Master) => "best",
        // YouTube HLS hard floor: never below 360p (itag 93) so AAC-LC stays >=128kbps
        (Platform::Youtube, FeedTier::Simulcast) => "480p,360p,worst",
        (Platform::Youtube, FeedTier::Light) => "audio_only,360p,480p,worst",
        (Platform::Tiktok | Platform::Bigo, FeedTier::Simulcast) => {
            "h264_sd,h265_sd,sd,h264_ld,h265_ld,ld,480p,360p,hd,h264_hd,h265_hd,worst"
        }
        (Platform::Tiktok | Platform::Bigo, FeedTier::Light) => {
            "h264_ao,h265_ao,audio_only,h265_ld,h264_ld,ld,sd,360p,worst"
        }
        (_, FeedTier::Light) => "audio_only,160p,360p,480p,worst",
        _ => "480p,360p,worst",
    }
}

pub(super) fn select_ytdlp_format(platform: Platform, tier: FeedTier) -> &'static str {
    match (platform, tier) {
        (Platform::Youtube, FeedTier::Master) => {
            "bestvideo[vcodec^=avc1]+bestaudio[acodec^=mp4a]/bestvideo+bestaudio/best"
        }
        (_, FeedTier::Master) => "bestvideo+bestaudio/best",
        // DASH paired <=480p video + bestaudio; muxed fallback picks <=480p or lowest >=360p (never 1080p)
        (Platform::Youtube | Platform::Facebook, FeedTier::Simulcast) => {
            "bestvideo[height<=480][vcodec^=avc1]+bestaudio[acodec^=mp4a]/bestvideo[height<=480]+bestaudio/best[height<=480]/worst[height>=360]/worst"
        }
        // TikTok/Bigo Simulcast: explicitly exclude audio-only (`ao` / `only_audio=1`) formats
        // because yt-dlp leaves `vcodec` unset on some TikTok FLV audio-only variants.
        (Platform::Tiktok | Platform::Bigo, FeedTier::Simulcast) => {
            "best[height<=480][format_id!*=ao][url!*=only_audio]/worst[height>=360][format_id!*=ao][url!*=only_audio]/worst[format_id!*=ao][url!*=only_audio]/best[format_id!*=ao][url!*=only_audio]/worst"
        }
        // Audio-only preferred; when platform only offers muxed HLS, pick LOWEST >=360p (never best/1080p!)
        (_, FeedTier::Light) => "bestaudio/worst[height>=360]/best[height<=480]/worst",
        _ => "best[height<=480][vcodec!=none]/worst[height>=360][vcodec!=none]/worst[vcodec!=none]/worst",
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

pub(super) fn valid_bind_ip(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim)
        .filter(|s| !s.is_empty() && s.parse::<std::net::IpAddr>().is_ok())
}

pub(super) fn build_streamlink_args(url: &str, quality: &str, bind_ip: Option<&str>) -> Vec<String> {
    let mut args = vec!["--stream-url".to_string()];
    if let Some(ip) = valid_bind_ip(bind_ip) {
        args.push("--interface".to_string());
        args.push(ip.to_string());
    }
    args.push(url.to_string());
    args.push(quality.to_string());
    args
}

const EXTRACTOR_SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(30);

async fn run_cmd_with_timeout(
    cmd: &mut Command,
    job_guard: &WindowsJobGuard,
    label: &str,
) -> Result<std::process::Output, ResolveError> {
    cmd.kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd
        .spawn()
        .map_err(|e| ResolveError::ExtractorFailed(format!("{label} spawn failed: {e}")))?;
    if let Some(pid) = child.id() {
        let _ = job_guard.assign_pid(pid);
    }
    match tokio::time::timeout(EXTRACTOR_SUBPROCESS_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => res.map_err(|e| ResolveError::ExtractorFailed(e.to_string())),
        Err(_) => Err(ResolveError::ExtractorFailed(format!(
            "{label} timed out after {}s",
            EXTRACTOR_SUBPROCESS_TIMEOUT.as_secs()
        ))),
    }
}

pub(super) async fn run_streamlink(
    exe: &Path,
    url: &str,
    quality: &str,
    bind_ip: Option<&str>,
    job_guard: &WindowsJobGuard,
) -> Result<ResolvedMedia, ResolveError> {
    let args = build_streamlink_args(url, quality, bind_ip);
    use pgmwatch_core::process::BackgroundCommandExt;
    let mut cmd = Command::new(exe);
    cmd.no_window().args(&args);
    let output = run_cmd_with_timeout(&mut cmd, job_guard, "streamlink").await?;

    if output.status.success() {
        let stream_url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stream_url.starts_with("http") {
            let path_no_query = stream_url.split(['?', '#']).next().unwrap_or("");
            if path_no_query.to_ascii_lowercase().ends_with(".mpd") {
                return Err(ResolveError::ExtractorFailed(
                    "Streamlink returned raw DASH .mpd; falling back to yt-dlp DASH relay".into(),
                ));
            }
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
const YT_SIMULCAST_FORMAT_EXPR: &str = "best[height<=480][vcodec^=avc1]/best[height<=480]/bestvideo[height<=480][vcodec^=avc1]+bestaudio[acodec^=mp4a]/bestvideo[height<=480]+bestaudio/worst[height>=360]/worst";
const YT_LIGHT_FORMAT_EXPR: &str = "bestaudio/worst[height>=360]/best[height<=480]/worst";

pub(super) fn yt_format_for(format_expr: &str, prefer_short_audio_only: bool) -> &str {
    if format_expr.contains("bestaudio") && !format_expr.contains("bestvideo") {
        if prefer_short_audio_only {
            YT_LIGHT_FORMAT_EXPR
        } else {
            format_expr
        }
    } else if format_expr.contains("height<=480") {
        YT_SIMULCAST_FORMAT_EXPR
    } else {
        YT_AV_FORMAT_EXPR
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_yt_dlp(
    exe: &Path,
    node_exe: &Path,
    url: &str,
    format_expr: &str,
    cookies_file: Option<&Path>,
    enable_bgutil_pot: bool,
    bgutil_pot_port: u16,
    bind_ip: Option<&str>,
    dash_relay: &DashRelayState,
    job_guard: &WindowsJobGuard,
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
            job_guard,
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
        job_guard,
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

pub(super) fn select_ytdlp_formats(
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

pub(super) fn build_yt_dlp_args(
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
    job_guard: &WindowsJobGuard,
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
    cmd.no_window().args(&args);

    let output = run_cmd_with_timeout(&mut cmd, job_guard, "yt-dlp").await?;

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
    Err(classify_ytdlp_stderr(url, err))
}

pub(super) fn classify_ytdlp_stderr(url: &str, err: String) -> ResolveError {
    let lower = err.to_ascii_lowercase();
    let is_fb_event =
        url.to_ascii_lowercase().contains("facebook.com") && url.to_ascii_lowercase().contains("/events/");

    // Facebook /events/ pages return "No video found ... Use --cookies-from-browser" before
    // the broadcast starts or after it ends. Check offline/not-started indicators BEFORE
    // generic yt-dlp `--cookies` footer hints so pre-live events show Offline (auto-polling)
    // instead of a misleading "Login Required" prompt.
    let explicit_offline = lower.contains("offline")
        || lower.contains("not currently live")
        || lower.contains("premieres in")
        || lower.contains("live event will begin")
        || lower.contains("this live event has ended")
        || lower.contains("video unavailable")
        || (lower.contains("no video found")
            && (is_fb_event
                || (!lower.contains("must be logged in")
                    && !lower.contains("log in to view")
                    && !lower.contains("this video is private"))));

    if explicit_offline {
        return ResolveError::Offline(err);
    }

    let explicit_login = lower.contains("sign in")
        || lower.contains("must be logged in")
        || lower.contains("login required")
        || lower.contains("log in to view")
        || lower.contains("age-restricted")
        || lower.contains("confirm your age")
        || lower.contains("members-only")
        || lower.contains("subscribers-only")
        || lower.contains("private video")
        || lower.contains("this video is private")
        || lower.contains("confirm you're not a bot")
        || lower.contains("confirm you\u{2019}re not a bot");

    if explicit_login {
        ResolveError::LoginGated(err)
    } else {
        ResolveError::ExtractorFailed(err)
    }
}

