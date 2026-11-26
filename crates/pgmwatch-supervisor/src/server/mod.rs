use crate::supervisor::SupervisorState;
use axum::{
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode, Uri},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rust_embed::Embed;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

mod routes;
mod validation;
mod ws;

#[derive(Embed)]
#[folder = "../../ui/dist"]
struct UiAssets;

pub fn build_router(state: Arc<SupervisorState>) -> Router {
    let clips_service = ServeDir::new(&state.clips_dir);
    let (bind_host, port) = state
        .cfg
        .try_read()
        .map(|c| (c.server.bind_host.clone(), c.server.port))
        .unwrap_or_else(|_| ("127.0.0.1".to_string(), 8787));
    let cors = build_cors_layer(&bind_host, port);
    let allowed_origins = Arc::new(allowed_local_origins(&bind_host, port));

    Router::new()
        .route("/api/health", get(get_health))
        .route("/api/server/shutdown", post(post_server_shutdown))
        .route("/api/snapshot", get(routes::get_snapshot))
        .route("/api/accounting", get(routes::get_accounting))
        .route("/api/accounting/reset", post(routes::post_reset_accounting))
        .route("/api/clips/stats", get(routes::get_clips_stats))
        .route("/api/clips/clear", post(routes::post_clear_clips))
        .route("/api/clips/{filename}/delete", post(routes::post_delete_clip))
        .route("/api/incidents/clear", post(routes::post_clear_resolved_incidents))
        .route("/api/incidents/flush", post(routes::post_flush_all_incidents))
        .route("/api/export.xlsx", get(routes::download_xlsx))
        .route("/api/show", post(routes::post_show_preset))
        .route("/api/show/title", post(routes::post_show_title))
        .route("/api/shows", get(routes::list_shows))
        .route("/api/shows/create", post(routes::post_create_show))
        .route("/api/shows/import", post(routes::post_import_show))
        .route("/api/shows/{id}/export", get(routes::export_show_preset))
        .route("/api/shows/{id}/activate", post(routes::post_activate_show))
        .route("/api/shows/{id}/delete", post(routes::post_delete_show))
        .route("/api/config", get(routes::get_runtime_config).post(routes::post_runtime_config))
        .route("/api/channels/{id}/ack", post(routes::post_ack))
        .route("/api/ack_all", post(routes::post_ack_all))
        .route("/api/channels/retry_offline", post(routes::post_retry_offline))
        .route("/api/bookmarks", post(routes::post_add_bookmark))
        .route("/api/channels/{id}/enabled", post(routes::post_set_channel_enabled))
        .route("/api/channels/{id}/clip", post(routes::post_force_clip))
        .route("/api/channels/{id}/pfl", post(routes::post_toggle_pfl))
        .route("/api/channels/{id}/lufs/reset", post(routes::post_reset_channel_lufs))
        .route("/api/channels/{id}/lufs/target", post(routes::post_set_channel_lufs_target))
        .route("/api/lufs/reset", post(routes::post_reset_all_lufs))
        .route("/api/channels/{id}/tier", post(routes::post_set_tier))
        .route("/api/channels/{id}/stream.fmp4", get(routes::get_channel_fmp4_stream))
        .route("/api/internal/dash-hls/{token}", get(routes::get_dash_hls_playlist))
        .route("/api/channels/{id}/auth/login", post(routes::post_auth_login))
        .route("/api/channels/{id}/auth/wipe", post(routes::post_auth_wipe))
        .route("/api/channels/{id}/retry", post(routes::post_retry_channel))
        .route("/api/intermission", post(routes::post_intermission))
        .route("/api/matches/start", post(routes::post_match_start))
        .route("/api/matches/end", post(routes::post_match_end))
        .route("/api/overrides", post(routes::post_override))
        .route("/ws", get(ws::ws_handler))
        .nest_service("/clips", clips_service)
        .fallback(serve_embedded_ui)
        .layer(middleware::from_fn_with_state(
            allowed_origins,
            require_local_mutation_origin,
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(cors)
        .with_state(state)
}

fn allowed_local_origins(bind_host: &str, port: u16) -> Vec<String> {
    vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
        format!("http://{bind_host}:{port}"),
        "http://127.0.0.1:5173".to_string(),
        "http://localhost:5173".to_string(),
    ]
}

fn build_cors_layer(bind_host: &str, port: u16) -> CorsLayer {
    let origins: Vec<HeaderValue> = allowed_local_origins(bind_host, port)
        .into_iter()
        .filter_map(|o| HeaderValue::from_str(&o).ok())
        .collect();
    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([header::CONTENT_TYPE])
}

fn is_allowed_mutation_headers(
    allowed: &[String],
    sec_fetch_site: Option<&str>,
    origin: Option<&str>,
) -> bool {
    if let Some(sec_site) = sec_fetch_site
        && sec_site.eq_ignore_ascii_case("cross-site")
    {
        return false;
    }
    if let Some(orig) = origin
        && !allowed.iter().any(|a| a.eq_ignore_ascii_case(orig))
    {
        return false;
    }
    true
}

async fn require_local_mutation_origin(
    State(allowed): State<Arc<Vec<String>>>,
    req: Request,
    next: Next,
) -> Response {
    if matches!(
        *req.method(),
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        let sec_site = req
            .headers()
            .get("sec-fetch-site")
            .and_then(|v| v.to_str().ok());
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok());
        if !is_allowed_mutation_headers(&allowed, sec_site, origin) {
            return (StatusCode::FORBIDDEN, "Cross-site mutation rejected").into_response();
        }
    }
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self' ws: wss:; object-src 'none'; frame-ancestors 'none'",
        ),
    );
    res
}

async fn get_health(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let gov = st.governor_tel.read().await.clone();
    Json(serde_json::json!({
        "status": "ok",
        "service": "PGMWatch",
        "version": env!("CARGO_PKG_VERSION"),
        "active_workers": gov.active_workers,
        "degradation_stage": gov.degradation_stage,
        "gpu_adapter": gov.gpu_adapter_name,
        "db_ok": true
    }))
}

async fn post_server_shutdown(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    tracing::info!("Shutdown requested via Web UI /api/server/shutdown");
    let st_clone = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        st_clone.request_shutdown();
    });
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "ok": true, "shutting_down": true })),
    )
}

async fn serve_embedded_ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") || path == "api" {
        return (StatusCode::NOT_FOUND, "API endpoint not found").into_response();
    }
    let candidate = if path.is_empty() { "index.html" } else { path };

    if let Some(file) = UiAssets::get(candidate) {
        let mime = mime_guess::from_path(candidate).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref())],
            file.data.into_owned(),
        )
            .into_response();
    }

    if let Some(index) = UiAssets::get("index.html") {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            index.data.into_owned(),
        )
            .into_response();
    }

    (StatusCode::NOT_FOUND, "PGMWatch UI asset not found").into_response()
}

#[cfg(test)]
mod tests {
    use super::validation::*;
    use super::*;
    use pgmwatch_core::models::{FeedTier, ShowPreset};

    #[test]
    fn test_input_validation_boundaries() {
        assert!(validate_safe_id("id-yt-master_01"));
        assert!(!validate_safe_id("../escape"));
        assert!(!validate_safe_id(""));
        assert!(validate_safe_clip_filename("ch-1_20261125T150000Z_manual.mp4"));
        assert!(!validate_safe_clip_filename("../secret.mp4"));
        assert!(!validate_safe_clip_filename("sub/clip.mp4"));
        assert!(!validate_safe_clip_filename("clip.txt"));
        assert!(!validate_safe_clip_filename(""));
        assert!(validate_stream_url("https://youtube.com/@live"));
        assert!(validate_stream_url("rtmp://ingest.studio.local/live/pgm1"));
        assert!(validate_stream_url("rtmps://ingest.studio.local/live/pgm1"));
        assert!(validate_stream_url("srt://192.168.10.20:9000?mode=caller"));
        assert!(validate_stream_url("sim://healthy/id-1"));
        assert!(!validate_stream_url("file:///C:/Windows/win.ini"));
        assert!(!validate_stream_url("https://bad.url/stream with space"));
    }

    #[test]
    fn test_tier_cost_direction() {
        assert!(tier_cost(FeedTier::Master) > tier_cost(FeedTier::Simulcast));
        assert!(tier_cost(FeedTier::Simulcast) > tier_cost(FeedTier::Light));
        assert!(tier_cost(FeedTier::Simulcast) <= tier_cost(FeedTier::Master));
    }

    #[test]
    fn test_validate_show_preset_groups_and_channels() {
        let base_group = pgmwatch_core::models::LanguageGroupConfig {
            id: "en".into(),
            name: "English".into(),
            language_code: "EN".into(),
            intermission_active: false,
            complaint_keywords: vec![],
        };
        let base_ch = pgmwatch_core::models::ChannelConfig {
            id: "yt-1".into(),
            group_id: "en".into(),
            name: "Main YT".into(),
            url: "https://youtube.com/watch?v=abc".into(),
            platform: pgmwatch_core::models::Platform::Youtube,
            tier: FeedTier::Master,
            priority: 1,
            orientation_vertical: false,
            enabled: true,
            target_lufs: None,
            lufs_tolerance: None,
            broadcast_delay_secs: 0,
            exclude_from_accounting: false,
            live_streams_only: true,
        };
        let valid = ShowPreset {
            show_id: "show-1".into(),
            title: "Main Event".into(),
            created_at: chrono::Utc::now(),
            groups: vec![base_group.clone()],
            channels: vec![base_ch.clone()],
        };
        assert!(validate_show_preset(&valid).is_ok());

        let no_groups = ShowPreset {
            groups: vec![],
            channels: vec![],
            ..valid.clone()
        };
        assert!(validate_show_preset(&no_groups).is_err());

        let unknown_group_ch = ShowPreset {
            channels: vec![pgmwatch_core::models::ChannelConfig {
                group_id: "missing".into(),
                ..base_ch
            }],
            ..valid
        };
        assert!(validate_show_preset(&unknown_group_ch).is_err());
    }

    #[test]
    fn test_mutation_origin_guard() {
        let allowed = allowed_local_origins("127.0.0.1", 8787);
        assert!(is_allowed_mutation_headers(&allowed, None, None));
        assert!(is_allowed_mutation_headers(
            &allowed,
            Some("same-origin"),
            Some("http://127.0.0.1:8787")
        ));
        assert!(!is_allowed_mutation_headers(
            &allowed,
            Some("cross-site"),
            None
        ));
        assert!(!is_allowed_mutation_headers(
            &allowed,
            None,
            Some("https://evil.example")
        ));
    }

    #[test]
    fn test_sanitize_download_slug() {
        assert_eq!(
            routes::sanitize_download_slug("Grand Finals: Day 2!"),
            "grand-finals-day-2"
        );
        assert_eq!(routes::sanitize_download_slug("   !!!   "), "show-preset");
    }
}
