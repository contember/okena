use crate::routes::AppState;
use crate::types::HealthResponse;
use axum::Json;
use axum::extract::State;

pub async fn get_health(State(state): State<AppState>) -> Json<HealthResponse> {
    let uptime = state.start_time.elapsed().as_secs();
    Json(HealthResponse {
        status: "ok".into(),
        version: state.update_info.app_version(),
        uptime_secs: uptime,
    })
}
