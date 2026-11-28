use super::validation::{
    tier_cost, validate_safe_clip_filename, validate_safe_id, validate_show_preset,
    validate_stream_url,
};
use crate::ghost_browser::{detect_msedge_path, open_isolated_login_window, wipe_auth_profile};
use crate::supervisor::SupervisorState;
use crate::win32::{query_dxgi_adapters, DxgiAdapterInfo};
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use pgmwatch_core::config::PgmWatchConfig;
use pgmwatch_core::db::DbCommand;
use pgmwatch_core::export::build_accounting_workbook;
use pgmwatch_core::grid::{
    apply_overrides_to_samples, filter_samples_by_window, prepare_accounting_samples,
    slice_matches, summarize_grid,
};
use pgmwatch_core::models::{FeedTier, LanguageGroupConfig, ManualOverride, Platform, ShowPreset};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

pub(super) async fn get_snapshot(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    Json(st.snapshot().await)
}

#[derive(Serialize)]
struct AccountingResponse {
    summary: pgmwatch_core::grid::AccountingSummary,
    matches: Vec<pgmwatch_core::grid::MatchAccounting>,
    overrides: Vec<ManualOverride>,
}

pub(super) async fn get_accounting(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let samples = st.db.load_grid_samples(None).await.unwrap_or_default();
    let overrides = st.overrides.read().await.clone();
    let matches = st.matches.read().await.clone();
    let (show_channels, sched_start, sched_end) = {
        let s = st.show.read().await;
        (s.channels.clone(), s.scheduled_start_at, s.scheduled_end_at)
    };
    let merged = filter_samples_by_window(
        prepare_accounting_samples(
            &apply_overrides_to_samples(&samples, &overrides),
            &show_channels,
        ),
        sched_start,
        sched_end,
    );
    let summary = summarize_grid(&merged);
    let match_slices = slice_matches(&merged, &matches);
    Json(AccountingResponse {
        summary,
        matches: match_slices,
        overrides,
    })
}

pub(super) async fn download_xlsx(State(st): State<Arc<SupervisorState>>) -> Response {
    let snap = st.snapshot().await;
    let samples = st.db.load_grid_samples(None).await.unwrap_or_default();
    let overrides = st.overrides.read().await.clone();
    match build_accounting_workbook(
        &snap.show,
        &snap.channels,
        &samples,
        &snap.matches,
        &snap.incidents,
        &overrides,
    ) {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (
                    header::CONTENT_TYPE,
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                ),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"PGMWatch-Accounting.xlsx\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(err) => {
            tracing::error!("XLSX export error: {err}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to generate workbook").into_response()
        }
    }
}

pub(super) async fn post_show_preset(
    State(st): State<Arc<SupervisorState>>,
    Json(preset): Json<ShowPreset>,
) -> Response {
    if let Err(msg) = validate_show_preset(&preset) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    let current_id = st.show.read().await.show_id.clone();
    if preset.show_id != current_id
        && let Some(label) = st.active_segment_label().await
    {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!(
                    "Segment \"{label}\" is currently recording. End the segment in Broadcast Accounting before switching shows."
                )
            })),
        )
            .into_response();
    }
    st.apply_show_preset(preset).await;
    Json(st.snapshot().await).into_response()
}

#[derive(Deserialize)]
pub(super) struct ShowScheduleReq {
    scheduled_start_at: Option<chrono::DateTime<Utc>>,
    scheduled_end_at: Option<chrono::DateTime<Utc>>,
}

pub(super) async fn post_show_schedule(
    State(st): State<Arc<SupervisorState>>,
    Json(req): Json<ShowScheduleReq>,
) -> Response {
    if let (Some(start), Some(end)) = (req.scheduled_start_at, req.scheduled_end_at)
        && end <= start
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Stream end time must be after stream push (start) time." })),
        )
            .into_response();
    }
    let updated = {
        let mut s = st.show.write().await;
        s.scheduled_start_at = req.scheduled_start_at;
        s.scheduled_end_at = req.scheduled_end_at;
        s.clone()
    };
    st.persist_show_preset(&updated).await;
    st.recompute_live_accounting_totals().await;
    st.broadcast_snapshot().await;
    Json(st.snapshot().await).into_response()
}

#[derive(Serialize)]
struct ShowPresetSummary {
    show_id: String,
    title: String,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    group_count: usize,
    channel_count: usize,
    is_active: bool,
}

#[derive(Serialize)]
struct ShowPresetListResponse {
    active_show_id: String,
    active_segment_label: Option<String>,
    shows: Vec<ShowPresetSummary>,
}

pub(super) fn sanitize_download_slug(title: &str) -> String {
    let slug: String = title
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = slug
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if trimmed.is_empty() {
        "show-preset".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

pub(super) async fn list_shows(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let active_show = st.show.read().await.clone();
    let active_segment_label = st.active_segment_label().await;
    let stored = st.db.list_shows_now().await.unwrap_or_default();

    let mut shows: Vec<ShowPresetSummary> = stored
        .into_iter()
        .map(|(preset, updated_at)| {
            let is_active = preset.show_id == active_show.show_id;
            ShowPresetSummary {
                show_id: preset.show_id,
                title: preset.title,
                created_at: preset.created_at,
                updated_at,
                group_count: preset.groups.len(),
                channel_count: preset.channels.len(),
                is_active,
            }
        })
        .collect();

    if !shows.iter().any(|s| s.show_id == active_show.show_id) {
        shows.insert(
            0,
            ShowPresetSummary {
                show_id: active_show.show_id.clone(),
                title: active_show.title.clone(),
                created_at: active_show.created_at,
                updated_at: Utc::now(),
                group_count: active_show.groups.len(),
                channel_count: active_show.channels.len(),
                is_active: true,
            },
        );
    }

    Json(ShowPresetListResponse {
        active_show_id: active_show.show_id,
        active_segment_label,
        shows,
    })
}

pub(super) async fn export_show_preset(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid show_id" })),
        )
            .into_response();
    }
    let active_show = st.show.read().await.clone();
    let preset = if id == active_show.show_id {
        active_show
    } else {
        match st.db.get_show_now(&id).await {
            Ok(Some(p)) => p,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "Show preset not found" })),
                )
                    .into_response();
            }
            Err(err) => {
                tracing::error!("Failed reading show preset {id}: {err}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "Failed reading show preset" })),
                )
                    .into_response();
            }
        }
    };

    let Ok(bytes) = serde_json::to_vec_pretty(&preset) else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Failed serializing show preset" })),
        )
            .into_response();
    };
    let filename = format!("{}.pgmwatch-show.json", sanitize_download_slug(&preset.title));
    let mut resp = Response::new(axum::body::Body::from(bytes));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    if let Ok(cd) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        resp.headers_mut().insert(header::CONTENT_DISPOSITION, cd);
    }
    resp
}

#[derive(Deserialize)]
pub(super) struct CreateShowReq {
    title: String,
    #[serde(default)]
    group_code: Option<String>,
    #[serde(default)]
    group_name: Option<String>,
    #[serde(default)]
    reset_accounting: bool,
}

pub(super) async fn post_create_show(
    State(st): State<Arc<SupervisorState>>,
    Json(req): Json<CreateShowReq>,
) -> Response {
    if let Some(label) = st.active_segment_label().await {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": format!(
                    "Segment \"{label}\" is currently recording. End the segment in Broadcast Accounting before creating a new show."
                )
            })),
        )
            .into_response();
    }
    let title = req.title.trim();
    if title.is_empty() || title.len() > 160 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Show title must be 1..160 characters" })),
        )
            .into_response();
    }
    let group_code = req
        .group_code
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("MAIN")
        .to_ascii_uppercase();
    if group_code.len() > 16 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Starter group tag must be 1..16 characters" })),
        )
            .into_response();
    }
    let group_name = req
        .group_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Main Broadcast")
        .to_string();
    if group_name.len() > 80 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Starter group name must be 1..80 characters" })),
        )
            .into_response();
    }

    let suffix = &Uuid::new_v4().simple().to_string()[..10];
    let code_slug: String = group_code
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .take(12)
        .collect();
    let group_id = if code_slug.is_empty() {
        format!("grp-{suffix}")
    } else {
        format!("grp-{code_slug}")
    };

    let preset = ShowPreset {
        show_id: format!("show-{suffix}"),
        title: title.to_string(),
        created_at: Utc::now(),
        scheduled_start_at: None,
        scheduled_end_at: None,
        groups: vec![LanguageGroupConfig {
            id: group_id,
            name: group_name,
            language_code: group_code,
            intermission_active: false,
            complaint_keywords: vec![
                "no sound".to_string(),
                "muted".to_string(),
                "bisu".to_string(),
                "no audio".to_string(),
                "lag".to_string(),
                "freeze".to_string(),
                "stutter".to_string(),
                "suara hilang".to_string(),
            ],
        }],
        channels: Vec::new(),
    };

    if req.reset_accounting {
        st.reset_accounting().await;
    }
    st.apply_show_preset(preset).await;
    Json(st.snapshot().await).into_response()
}

#[derive(Deserialize)]
pub(super) struct ImportShowReq {
    preset: ShowPreset,
    #[serde(default = "default_true")]
    activate: bool,
    #[serde(default)]
    reset_accounting: bool,
}

fn default_true() -> bool {
    true
}

pub(super) async fn post_import_show(
    State(st): State<Arc<SupervisorState>>,
    Json(req): Json<ImportShowReq>,
) -> Response {
    if let Err(msg) = validate_show_preset(&req.preset) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": msg }))).into_response();
    }
    if req.activate {
        if let Some(label) = st.active_segment_label().await {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": format!(
                        "Segment \"{label}\" is currently recording. End the segment in Broadcast Accounting before activating an imported show."
                    )
                })),
            )
                .into_response();
        }
        if req.reset_accounting {
            st.reset_accounting().await;
        }
        st.apply_show_preset(req.preset).await;
    } else {
        let active_id = st.show.read().await.show_id.clone();
        if req.preset.show_id == active_id {
            return (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "Cannot save an inactive preset with the same show_id as the currently active show. Activate it to overwrite, or change its show_id."
                })),
            )
                .into_response();
        }
        if let Err(err) = st.db.upsert_show_now(&req.preset).await {
            tracing::error!("Failed saving imported show preset to DB: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed saving imported show preset" })),
            )
                .into_response();
        }
    }
    Json(st.snapshot().await).into_response()
}

pub(super) async fn post_activate_show(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid show_id" })),
        )
            .into_response();
    }
    match st.switch_show_preset(&id).await {
        Ok(()) => Json(st.snapshot().await).into_response(),
        Err((code, msg)) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            (status, Json(serde_json::json!({ "error": msg }))).into_response()
        }
    }
}

pub(super) async fn post_delete_show(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid show_id" })),
        )
            .into_response();
    }
    let active_id = st.show.read().await.show_id.clone();
    if id == active_id {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "Cannot delete the currently active show preset. Switch to another show first."
            })),
        )
            .into_response();
    }
    match st.db.delete_show_now(&id).await {
        Ok(true) => Json(serde_json::json!({ "ok": true })).into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Show preset not found" })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("Failed deleting show preset {id}: {err}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "Failed deleting show preset" })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub(super) struct ShowTitleReq {
    title: String,
}

pub(super) async fn post_show_title(
    State(st): State<Arc<SupervisorState>>,
    Json(req): Json<ShowTitleReq>,
) -> Response {
    let trimmed = req.title.trim();
    if trimmed.is_empty() || trimmed.len() > 160 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Show title must be 1..160 characters" })),
        )
            .into_response();
    }
    st.rename_show_title(trimmed.to_string()).await;
    Json(serde_json::json!({ "ok": true, "title": trimmed })).into_response()
}

pub(super) async fn post_ack(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    st.ack_channel(&id).await;
    Json(serde_json::json!({ "ok": true }))
}

pub(super) async fn post_force_clip(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    st.force_clip(&id).await;
    Json(serde_json::json!({ "ok": true }))
}

pub(super) async fn post_toggle_pfl(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let active = st.toggle_pfl(&id).await;
    Json(serde_json::json!({ "active_pfl_channel_id": active }))
}

pub(super) async fn post_reset_channel_lufs(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid channel id" })),
        )
            .into_response();
    }
    if !st.reset_lufs(Some(&id)).await {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Channel not found" })),
        )
            .into_response();
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub(super) async fn post_reset_all_lufs(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    st.reset_lufs(None).await;
    Json(serde_json::json!({ "ok": true }))
}

#[derive(Deserialize)]
pub(super) struct SetLufsTargetBody {
    target_lufs: Option<f32>,
    #[serde(default)]
    lufs_tolerance: Option<f32>,
}

pub(super) async fn post_set_channel_lufs_target(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
    Json(body): Json<SetLufsTargetBody>,
) -> Response {
    if !validate_safe_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid channel id" })),
        )
            .into_response();
    }
    if let Some(target) = body.target_lufs
        && (!target.is_finite() || !(-36.0..=-5.0).contains(&target))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Target loudness must be between -36.0 and -5.0 LUFS" })),
        )
            .into_response();
    }
    if let Some(tol) = body.lufs_tolerance
        && (!tol.is_finite() || !(0.5..=10.0).contains(&tol))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Loudness tolerance must be between 0.5 and 10.0 LU" })),
        )
            .into_response();
    }
    if st
        .set_channel_lufs_target(&id, body.target_lufs, body.lufs_tolerance)
        .await
        .is_none()
    {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Channel not found" })),
        )
            .into_response();
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub(super) struct SetTierBody {
    tier: FeedTier,
}

pub(super) async fn post_set_tier(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
    Json(body): Json<SetTierBody>,
) -> Response {
    let current_tier = {
        let show = st.show.read().await;
        match show.channels.iter().find(|c| c.id == id) {
            Some(ch) => ch.tier,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({ "error": "Channel not found" })),
                )
                    .into_response();
            }
        }
    };
    if current_tier == body.tier {
        return Json(serde_json::json!({ "ok": true })).into_response();
    }
    if tier_cost(body.tier) > tier_cost(current_tier) {
        let mut gov = st.governor.lock().await;
        if let Err(reason) = gov.admit_tier_upgrade() {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({ "error": reason })),
            )
                .into_response();
        }
    }
    if st.set_channel_tier(&id, body.tier).await.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Channel not found" })),
        )
            .into_response();
    }
    st.reload_channel_worker(&id).await;
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub(super) async fn post_reset_accounting(State(st): State<Arc<SupervisorState>>) -> Response {
    st.reset_accounting().await;
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub(super) async fn get_clips_stats(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    Json(st.clip_storage_stats().await)
}

pub(super) async fn post_clear_clips(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let res = st.clear_all_clips().await;
    Json(serde_json::json!({
        "ok": true,
        "deleted_files": res.deleted_files,
        "freed_bytes": res.freed_bytes,
        "stats": res.stats,
    }))
}

pub(super) async fn post_delete_clip(
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

pub(super) async fn post_clear_resolved_incidents(
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

pub(super) async fn post_flush_all_incidents(
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

pub(super) async fn get_channel_fmp4_stream(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "Invalid channel id").into_response();
    }
    let ch_opt = {
        let map = st.channels.read().await;
        map.get(&id).map(|m| m.config.clone())
    };
    let Some(ch) = ch_opt else {
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
            .resolve(&ch.url, ch.platform, ch.tier, ch.live_streams_only)
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

pub(super) async fn get_dash_hls_playlist(
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

pub(super) async fn get_platform_auth_status(
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

pub(super) async fn post_platform_auth_login(
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

pub(super) async fn post_platform_auth_sync(
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

pub(super) async fn post_platform_auth_wipe(
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

pub(super) async fn post_auth_login(
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

pub(super) async fn post_auth_wipe(
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

pub(super) async fn post_retry_channel(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> Response {
    if !validate_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "Invalid channel id").into_response();
    }
    let ch_url = {
        let map = st.channels.read().await;
        match map.get(&id) {
            Some(m) => m.config.url.clone(),
            None => return (StatusCode::NOT_FOUND, "Channel not found").into_response(),
        }
    };
    st.resolver.invalidate_url(&ch_url).await;
    let st_clone = Arc::clone(&st);
    tokio::spawn(async move {
        st_clone.reload_channel_worker(&id).await;
    });
    Json(serde_json::json!({ "ok": true })).into_response()
}

pub(super) async fn post_ack_all(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let acked = st.ack_all_channels().await;
    Json(serde_json::json!({ "ok": true, "acked_channels": acked }))
}

pub(super) async fn post_retry_offline(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let queued = st.retry_offline_channels().await;
    Json(serde_json::json!({ "ok": true, "queued": queued }))
}

#[derive(Deserialize)]
pub(super) struct SetEnabledBody {
    enabled: bool,
}

pub(super) async fn post_set_channel_enabled(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
    Json(body): Json<SetEnabledBody>,
) -> Response {
    if !validate_safe_id(&id) {
        return (StatusCode::BAD_REQUEST, "Invalid channel id").into_response();
    }
    match st.set_channel_enabled(&id, body.enabled).await {
        Some(show) => Json(serde_json::json!({ "ok": true, "show": show })).into_response(),
        None => (StatusCode::NOT_FOUND, "Channel not found").into_response(),
    }
}

#[derive(Deserialize)]
pub(super) struct BookmarkBody {
    channel_id: Option<String>,
    note: String,
}

pub(super) async fn post_add_bookmark(
    State(st): State<Arc<SupervisorState>>,
    Json(body): Json<BookmarkBody>,
) -> Response {
    let trimmed = body.note.trim();
    if trimmed.is_empty() || trimmed.len() > 240 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Note must be 1..240 characters" })),
        )
            .into_response();
    }
    if let Some(ref cid) = body.channel_id
        && !validate_safe_id(cid)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid channel_id" })),
        )
            .into_response();
    }
    let inc = st
        .add_operator_bookmark(body.channel_id.as_deref(), trimmed.to_string())
        .await;
    Json(inc).into_response()
}

#[derive(Deserialize)]
pub(super) struct IntermissionBody {
    group_id: Option<String>,
    active: bool,
}

pub(super) async fn post_intermission(
    State(st): State<Arc<SupervisorState>>,
    Json(body): Json<IntermissionBody>,
) -> impl IntoResponse {
    st.set_intermission(body.group_id.as_deref(), body.active).await;
    Json(st.snapshot().await)
}

#[derive(Deserialize)]
pub(super) struct MatchStartBody {
    label: String,
}

pub(super) async fn post_match_start(
    State(st): State<Arc<SupervisorState>>,
    Json(body): Json<MatchStartBody>,
) -> Response {
    let trimmed = body.label.trim();
    if trimmed.is_empty() || trimmed.len() > 120 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Match label must be 1..120 characters" })),
        )
            .into_response();
    }
    let marker = st.start_match(trimmed.to_string()).await;
    Json(marker).into_response()
}

pub(super) async fn post_match_end(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let ended = st.end_active_match().await;
    Json(serde_json::json!({ "ended": ended }))
}

#[derive(Deserialize)]
pub(super) struct OverrideBody {
    channel_id: String,
    metric_name: String,
    bucket_utc: Option<chrono::DateTime<Utc>>,
    value: f64,
    operator_note: String,
}

pub(super) async fn post_override(
    State(st): State<Arc<SupervisorState>>,
    Json(body): Json<OverrideBody>,
) -> Response {
    if !validate_safe_id(&body.channel_id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid channel_id" })),
        )
            .into_response();
    }
    if !body.value.is_finite() || body.value < 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Override value must be finite and >= 0" })),
        )
            .into_response();
    }
    let ov = ManualOverride {
        id: Uuid::new_v4().to_string(),
        channel_id: body.channel_id,
        metric_name: body.metric_name,
        bucket_utc: body.bucket_utc,
        value: body.value,
        operator_note: body.operator_note.chars().take(240).collect(),
        updated_at: Utc::now(),
    };
    st.overrides.write().await.insert(0, ov.clone());
    st.db.enqueue(DbCommand::UpsertOverride(ov.clone())).await;
    Json(ov).into_response()
}

#[derive(Serialize)]
struct RuntimeConfigPayload {
    config: PgmWatchConfig,
    adapters: Vec<DxgiAdapterInfo>,
}

pub(super) async fn get_runtime_config(State(st): State<Arc<SupervisorState>>) -> Response {
    let config = st.cfg.read().await.clone();
    let adapters = tokio::task::spawn_blocking(query_dxgi_adapters)
        .await
        .unwrap_or_default();
    Json(RuntimeConfigPayload { config, adapters }).into_response()
}

pub(super) async fn post_runtime_config(
    State(st): State<Arc<SupervisorState>>,
    Json(next): Json<PgmWatchConfig>,
) -> Response {
    match st.apply_runtime_config(next).await {
        Ok(restart_required) => {
            let config = st.cfg.read().await.clone();
            Json(serde_json::json!({
                "ok": true,
                "config": config,
                "restart_required": restart_required,
            }))
            .into_response()
        }
        Err(msg) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response(),
    }
}
