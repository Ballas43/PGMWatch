use crate::server::validation::validate_safe_id;
use crate::supervisor::SupervisorState;
use crate::win32::{query_dxgi_adapters, DxgiAdapterInfo};
use axum::{
    extract::State,
    http::{header, StatusCode},
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
use pgmwatch_core::models::ManualOverride;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;


mod channels;
mod media_auth;
mod show;

pub(super) use channels::*;
pub(super) use media_auth::*;
pub(super) use show::*;

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


pub(super) async fn post_reset_accounting(State(st): State<Arc<SupervisorState>>) -> Response {
    st.reset_accounting().await;
    Json(serde_json::json!({ "ok": true })).into_response()
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
