use crate::ghost_browser::{detect_msedge_path, open_isolated_login_window, wipe_auth_profile};
use crate::server::validation::{
    validate_safe_clip_filename, validate_safe_id, validate_stream_url,
};
use crate::supervisor::SupervisorState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use pgmwatch_core::models::Platform;
use std::sync::Arc;


pub(crate) async fn get_clips_stats(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    Json(st.clip_storage_stats().await)
}

pub(crate) async fn post_clear_clips(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let res = st.clear_all_clips().await;
    Json(serde_json::json!({
        "ok": true,
        "deleted_files": res.deleted_files,
        "freed_bytes": res.freed_bytes,
        "stats": res.stats,
    }))
}

pub(crate) async fn post_delete_clip(
    State(st): State<Arc<SupervisorState>>,
    Path(filename): Path<String>,
) -> Response {
    if !validate_safe_clip_filename(&filename) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid clip filename" })),
        )
            .into_response();
    }
    let res = st.delete_single_clip(&filename).await;
    Json(serde_json::json!({
        "ok": true,
        "deleted_files": res.deleted_files,
        "freed_bytes": res.freed_bytes,
        "stats": res.stats,
    }))
    .into_response()
}

pub(crate) async fn post_clear_resolved_incidents(
    State(st): State<Arc<SupervisorState>>,
) -> impl IntoResponse {
    let res = st.clear_resolved_incidents().await;
    Json(serde_json::json!({
        "ok": true,
        "removed_incidents": res.removed_incidents,
        "deleted_files": res.deleted_files,
        "freed_bytes": res.freed_bytes,
        "stats": res.stats,
    }))
}

pub(crate) async fn post_flush_all_incidents(
    State(st): State<Arc<SupervisorState>>,
) -> impl IntoResponse {
    let res = st.clear_all_incidents().await;
    Json(serde_json::json!({
        "ok": true,
        "removed_incidents": res.removed_incidents,
        "deleted_files": res.deleted_files,
        "freed_bytes": res.freed_bytes,
        "stats": res.stats,
    }))
}

pub(crate) async fn get_channel_fmp4_stream(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "Invalid channel id").into_response();
    }
    let ch_opt = {
        let map = st.channels.read().await;
        map.get(&id).map(|m| (m.config.clone(), m.effective_tier))
    };
    let Some((ch, eff_tier)) = ch_opt else {
        return (StatusCode::NOT_FOUND, "Channel not found").into_response();
    };
    if !ch.enabled || ch.url.trim().to_ascii_lowercase().starts_with("sim://") {
        return (StatusCode::NO_CONTENT, "").into_response();
    }
    let media = if let Some(cached) = st.resolved_media.read().await.get(&id).cloned() {
        cached
    } else {
        match st
            .resolver
            .resolve(&ch.url, ch.platform, eff_tier, ch.live_streams_only)
            .await
        {
            Ok(resolved) => {
                st.resolved_media.write().await.insert(id.clone(), resolved.clone());
                resolved
            }
            Err(err) => {
                tracing::debug!("MSE stream resolve failed for {id}: {err}");
                return (StatusCode::BAD_GATEWAY, "Stream URL resolution unavailable").into_response();
            }
        }
    };
    if !validate_stream_url(&media.stream_url)
        || media
            .audio_url
            .as_deref()
            .is_some_and(|u| !validate_stream_url(u))
    {
        return (StatusCode::BAD_REQUEST, "Unsafe resolved media URL").into_response();
    }
    let sub = match st.mse_hubs.subscribe(&id, &media, Arc::clone(&st.job_guard)).await {
        Ok(s) => s,
        Err(err) => {
            tracing::debug!("MSE stream unavailable for {id}: {err}");
            return (StatusCode::BAD_GATEWAY, "Unable to initialize fMP4 stream").into_response();
        }
    };

    let codec = sub.codec.clone();
    let shutdown_rx = st.subscribe_shutdown();
    let stream = futures_util::stream::unfold(
        Some((sub, shutdown_rx)),
        |state_opt| async move {
            let (mut sub, mut shutdown_rx) = state_opt?;
            if *shutdown_rx.borrow() {
                return None;
            }
            if !sub.init_segment.is_empty() {
                let init = std::mem::take(&mut sub.init_segment);
                return Some((
                    Ok::<axum::body::Bytes, std::convert::Infallible>(init),
                    Some((sub, shutdown_rx)),
                ));
            }
            if let Some(pre) = sub.prebuffer.pop_front() {
                return Some((Ok(pre), Some((sub, shutdown_rx))));
            }
            tokio::select! {
                chunk = sub.rx.recv() => match chunk {
                    Ok(c) => Some((Ok(c), Some((sub, shutdown_rx)))),
                    Err(_) => None,
                },
                _ = shutdown_rx.changed() => None,
            }
        },
    );

    let mut res = (StatusCode::OK, axum::body::Body::from_stream(stream)).into_response();
    let hdrs = res.headers_mut();
    hdrs.insert(header::CONTENT_TYPE, HeaderValue::from_static("video/mp4"));
    hdrs.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Ok(val) = HeaderValue::from_str(&codec)
        && let Ok(name) = header::HeaderName::from_bytes(b"x-mse-codec")
    {
        hdrs.insert(name, val);
    }
    res
}

pub(crate) async fn get_dash_hls_playlist(
    State(st): State<Arc<SupervisorState>>,
    Path(token): Path<String>,
) -> Response {
    match st.dash_relay.render_hls(&token).await {
        Ok(m3u8) => {
            let mut res = (StatusCode::OK, m3u8).into_response();
            let hdrs = res.headers_mut();
            hdrs.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/vnd.apple.mpegurl; charset=utf-8"),
            );
            hdrs.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            res
        }
        Err(err) => {
            let msg = err.to_string();
            tracing::debug!("DASH/HLS relay error for token '{token}': {msg}");
            let status = if msg.contains("HTTP 401") {
                StatusCode::UNAUTHORIZED
            } else if msg.contains("HTTP 403") {
                StatusCode::FORBIDDEN
            } else if msg.contains("HTTP 410")
                || msg.contains("HTTP 404")
                || msg.contains("Unknown or expired DASH relay token")
            {
                StatusCode::GONE
            } else {
                StatusCode::BAD_GATEWAY
            };
            (status, format!("Manifest relay unavailable: {msg}")).into_response()
        }
    }
}

fn parse_auth_platform(raw: &str) -> Option<(Platform, &'static str, &'static str)> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "facebook" => Some((Platform::Facebook, "Facebook", "https://www.facebook.com/login")),
        "tiktok" => Some((Platform::Tiktok, "TikTok", "https://www.tiktok.com/login")),
        _ => None,
    }
}

pub(crate) async fn get_platform_auth_status(
    State(st): State<Arc<SupervisorState>>,
) -> impl IntoResponse {
    let channels_platforms: std::collections::HashSet<Platform> = {
        let map = st.channels.read().await;
        map.values().map(|m| m.config.platform).collect()
    };

    let entries = [
        (
            "facebook",
            Platform::Facebook,
            "Facebook",
            "Unlocks Reactions (Likes) and Shares on live & event pages",
        ),
        (
            "tiktok",
            Platform::Tiktok,
            "TikTok",
            "Reduces rate limits for live Likes, Shares, and Gifts",
        ),
    ];

    let platforms: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|(key, plat, label, unlocks)| {
            let profile_exists = st
                .profiles_dir
                .join(crate::resolver::auth_profile_slug(plat))
                .join("Default")
                .exists();
            let cookies_path = st.profiles_dir.join(crate::resolver::auth_cookies_slug(plat));
            let has_cookies = cookies_path
                .metadata()
                .map(|m| m.len() > 64)
                .unwrap_or(false);
            serde_json::json!({
                "platform": key,
                "label": label,
                "has_channels": channels_platforms.contains(&plat),
                "profile_ready": profile_exists,
                "connected": has_cookies,
                "unlocks": unlocks,
            })
        })
        .collect();

    Json(serde_json::json!({ "platforms": platforms }))
}

pub(crate) async fn post_platform_auth_login(
    State(st): State<Arc<SupervisorState>>,
    Path(platform_raw): Path<String>,
) -> Response {
    let Some((platform, label, login_url)) = parse_auth_platform(&platform_raw) else {
        return (StatusCode::BAD_REQUEST, "Unsupported platform for login").into_response();
    };
    let configured_msedge = st.cfg.read().await.extraction.msedge_path.clone();
    let Some(msedge) = detect_msedge_path(&configured_msedge) else {
        return (StatusCode::BAD_REQUEST, "Microsoft Edge not detected on this PC").into_response();
    };
    let slug = crate::resolver::auth_profile_slug(platform);
    let auth_dir = st.profiles_dir.join(slug);
    match open_isolated_login_window(&msedge, &auth_dir, login_url) {
        Ok(()) => Json(serde_json::json!({
            "ok": true,
            "platform": platform_raw,
            "label": label,
            "login_url": login_url
        }))
        .into_response(),
        Err(e) => {
            tracing::error!("Failed to launch Edge login for {label}: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open Edge login window").into_response()
        }
    }
}

pub(crate) async fn post_platform_auth_sync(
    State(st): State<Arc<SupervisorState>>,
    Path(platform_raw): Path<String>,
) -> Response {
    let Some((platform, label, _)) = parse_auth_platform(&platform_raw) else {
        return (StatusCode::BAD_REQUEST, "Unsupported platform").into_response();
    };
    let configured_msedge = st.cfg.read().await.extraction.msedge_path.clone();
    let Some(msedge) = detect_msedge_path(&configured_msedge) else {
        return (StatusCode::BAD_REQUEST, "Microsoft Edge not detected on this PC").into_response();
    };
    let auth_dir = st.profiles_dir.join(crate::resolver::auth_profile_slug(platform));
    if !auth_dir.join("Default").exists() {
        return (
            StatusCode::BAD_REQUEST,
            format!("Please click Sign In to {label} first to create a browser session."),
        )
            .into_response();
    }
    let cookies_path = st.profiles_dir.join(crate::resolver::auth_cookies_slug(platform));
    match crate::ghost_browser::export_cookies_from_auth_profile(
        &msedge,
        &auth_dir,
        &cookies_path,
        Arc::clone(&st.job_guard),
    )
    .await
    {
        Ok(count) if count > 0 => {
            st.stats_notify.notify_one();
            if platform == Platform::Tiktok {
                st.sync_tiktok_subscriptions().await;
            }
            Json(serde_json::json!({
                "ok": true,
                "cookies_exported": count
            }))
            .into_response()
        }
        Ok(_) => (
            StatusCode::BAD_REQUEST,
            format!("No {label} session cookies found yet. Finish signing in inside the Edge window, then click Sync Session again."),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!("Cookie export failed for {label}: {err}");
            (
                StatusCode::BAD_REQUEST,
                format!("Could not read {label} cookies yet (close the Edge login window first if it is still open)."),
            )
                .into_response()
        }
    }
}

pub(crate) async fn post_platform_auth_wipe(
    State(st): State<Arc<SupervisorState>>,
    Path(platform_raw): Path<String>,
) -> Response {
    let Some((platform, _, _)) = parse_auth_platform(&platform_raw) else {
        return (StatusCode::BAD_REQUEST, "Unsupported platform").into_response();
    };
    let _ = wipe_auth_profile(&st.profiles_dir.join(crate::resolver::auth_profile_slug(platform)));
    let _ = std::fs::remove_file(st.profiles_dir.join(crate::resolver::auth_cookies_slug(platform)));
    st.stats_notify.notify_one();
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub(crate) async fn post_auth_login(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    let snap = st.snapshot().await;
    let Some(ch) = snap.channels.into_iter().find(|c| c.config.id == id) else {
        return (StatusCode::NOT_FOUND, "Channel not found").into_response();
    };
    let configured_msedge = st.cfg.read().await.extraction.msedge_path.clone();
    let Some(msedge) = detect_msedge_path(&configured_msedge) else {
        return (StatusCode::BAD_REQUEST, "Microsoft Edge not detected").into_response();
    };
    let slug = crate::resolver::auth_profile_slug(ch.config.platform);
    let auth_dir = st.profiles_dir.join(slug);
    match open_isolated_login_window(&msedge, &auth_dir, &ch.config.url) {
        Ok(()) => Json(serde_json::json!({ "ok": true, "profile": slug })).into_response(),
        Err(e) => {
            tracing::error!("Failed to launch Edge login fallback: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to open login window").into_response()
        }
    }
}

pub(crate) async fn post_auth_wipe(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if !validate_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "Invalid channel id").into_response();
    }
    let ch_opt = {
        let map = st.channels.read().await;
        map.get(&id).map(|m| m.config.clone())
    };
    let platform = ch_opt.as_ref().map(|c| c.platform).unwrap_or(Platform::Custom);
    let _ = wipe_auth_profile(&st.profiles_dir.join(crate::resolver::auth_profile_slug(platform)));
    let _ = std::fs::remove_file(st.profiles_dir.join(crate::resolver::auth_cookies_slug(platform)));
    if let Some(ref ch) = ch_opt {
        st.resolver.invalidate_url(&ch.url).await;
    }
    let st_clone = Arc::clone(&st);
    let id_clone = id.clone();
    tokio::spawn(async move {
        st_clone.reload_channel_worker(&id_clone).await;
    });
    Json(serde_json::json!({ "ok": true })).into_response()
}

