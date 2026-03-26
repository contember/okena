use crate::remote::routes::AppState;
use axum::extract::State;
use axum::http::StatusCode;

pub async fn post_reload(State(state): State<AppState>) -> StatusCode {
    state.auth_store.reload_tokens();
    StatusCode::OK
}
