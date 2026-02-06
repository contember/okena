use crate::remote::routes::AppState;
use crate::remote::types::HealthResponse;
use axum::Json;
use axum::extract::State;

pub async fn get_health(State(state): State<AppState>) -> Json<HealthResponse> {
    let uptime = state.start_time.elapsed().as_secs();
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        uptime_secs: uptime,
    })
}
