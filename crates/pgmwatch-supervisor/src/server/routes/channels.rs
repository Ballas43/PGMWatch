use crate::server::validation::{tier_cost, validate_safe_id};
use crate::supervisor::SupervisorState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use pgmwatch_core::models::FeedTier;
use serde::Deserialize;
use std::sync::Arc;


pub(crate) async fn post_ack(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    st.ack_channel(&id).await;
    Json(serde_json::json!({ "ok": true }))
}

pub(crate) async fn post_force_clip(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    st.force_clip(&id).await;
    Json(serde_json::json!({ "ok": true }))
}

pub(crate) async fn post_toggle_pfl(
    State(st): State<Arc<SupervisorState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let active = st.toggle_pfl(&id).await;
    Json(serde_json::json!({ "active_pfl_channel_id": active }))
}

pub(crate) async fn post_reset_channel_lufs(
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

pub(crate) async fn post_reset_all_lufs(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    st.reset_lufs(None).await;
    Json(serde_json::json!({ "ok": true }))
}

#[derive(Deserialize)]
pub(crate) struct SetLufsTargetBody {
    target_lufs: Option<f32>,
    #[serde(default)]
    lufs_tolerance: Option<f32>,
}

pub(crate) async fn post_set_channel_lufs_target(
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
pub(crate) struct SetTierBody {
    tier: FeedTier,
}

pub(crate) async fn post_set_tier(
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


pub(crate) async fn post_retry_channel(
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

pub(crate) async fn post_ack_all(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let acked = st.ack_all_channels().await;
    Json(serde_json::json!({ "ok": true, "acked_channels": acked }))
}

pub(crate) async fn post_retry_offline(State(st): State<Arc<SupervisorState>>) -> impl IntoResponse {
    let queued = st.retry_offline_channels().await;
    Json(serde_json::json!({ "ok": true, "queued": queued }))
}

#[derive(Deserialize)]
pub(crate) struct SetEnabledBody {
    enabled: bool,
}

pub(crate) async fn post_set_channel_enabled(
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
