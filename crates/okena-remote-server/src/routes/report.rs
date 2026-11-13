//! `/v1/report/...`: link a child terminal to its parent, a child's own
//! report, the parent's wait, and the list of children.

use crate::report::ReportError;
use crate::routes::AppState;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use okena_core::report::{
    ApiReportChildrenResponse, ApiReportEvent, ApiReportLinkRequest, ApiReportRequest,
    ApiReportUnlinkRequest, ApiReportWaitRequest, ApiReportWaitResponse,
};
use serde::Deserialize;

impl IntoResponse for ReportError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::NotLinked => (
                StatusCode::NOT_FOUND,
                "this terminal reports to no parent (start it with `okena spawn` or link it with `okena link`)"
                    .to_string(),
            ),
            Self::TerminalNotFound(id) => (StatusCode::NOT_FOUND, format!("terminal not found: {id}")),
            Self::Invalid(message) => (StatusCode::BAD_REQUEST, message),
            Self::BridgeUnavailable => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "bridge unavailable".to_string(),
            ),
        };
        (status, message).into_response()
    }
}

pub async fn post_link(
    State(state): State<AppState>,
    Json(request): Json<ApiReportLinkRequest>,
) -> Result<StatusCode, ReportError> {
    state.report_hub.link(&request).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn post_unlink(
    State(state): State<AppState>,
    Json(request): Json<ApiReportUnlinkRequest>,
) -> Result<StatusCode, ReportError> {
    state.report_hub.unlink(&request.child_terminal_id)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn post_report(
    State(state): State<AppState>,
    Json(request): Json<ApiReportRequest>,
) -> Result<Json<ApiReportEvent>, ReportError> {
    state.report_hub.report(&request).await.map(Json)
}

pub async fn post_wait(
    State(state): State<AppState>,
    Json(request): Json<ApiReportWaitRequest>,
) -> Result<Json<ApiReportWaitResponse>, ReportError> {
    state.report_hub.wait(&request).await.map(Json)
}

#[derive(Deserialize)]
pub struct ChildrenQuery {
    parent: Option<String>,
}

pub async fn get_children(
    State(state): State<AppState>,
    Query(query): Query<ChildrenQuery>,
) -> Json<ApiReportChildrenResponse> {
    Json(state.report_hub.children(query.parent.as_deref()))
}
