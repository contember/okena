use crate::remote::routes::AppState;
use crate::remote::types::HealthResponse;
use axum::Json;
use axum::extract::State;

pub async fn get_health(State(state): State<AppState>) -> Json<HealthResponse> {
    let uptime = state.start_time.elapsed().as_secs();
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        uptime_secs: uptime,
    })
}
