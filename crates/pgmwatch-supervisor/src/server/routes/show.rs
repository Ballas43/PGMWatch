use crate::server::validation::{validate_safe_id, validate_show_preset};
use crate::supervisor::SupervisorState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use pgmwatch_core::models::{LanguageGroupConfig, ShowPreset};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;


pub(crate) async fn post_show_preset(
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
pub(crate) struct ShowScheduleReq {
    scheduled_start_at: Option<chrono::DateTime<Utc>>,
    scheduled_end_at: Option<chrono::DateTime<Utc>>,
}

pub(crate) async fn post_show_schedule(
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

pub(crate) fn sanitize_download_slug(title: &str) -> String {
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

pub(crate) async fn list_shows(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
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

pub(crate) async fn export_show_preset(
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
pub(crate) struct CreateShowReq {
    title: String,
    #[serde(default)]
    group_code: Option<String>,
    #[serde(default)]
    group_name: Option<String>,
    #[serde(default)]
    reset_accounting: bool,
}

pub(crate) async fn post_create_show(
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
pub(crate) struct ImportShowReq {
    preset: ShowPreset,
    #[serde(default = "default_true")]
    activate: bool,
    #[serde(default)]
    reset_accounting: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) async fn post_import_show(
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

pub(crate) async fn post_activate_show(
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

pub(crate) async fn post_delete_show(
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
pub(crate) struct ShowTitleReq {
    title: String,
}

pub(crate) async fn post_show_title(
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

