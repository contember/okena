use crate::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::routes::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use okena_core::api::ApiWindow;
use std::collections::HashMap;

/// The windows desktops declared, in connection order. Empty when none did;
/// the daemon's synthetic main window then stands in.
fn declared_windows(by_connection: &HashMap<u64, Vec<ApiWindow>>) -> Vec<ApiWindow> {
    let mut connections: Vec<_> = by_connection.iter().collect();
    connections.sort_by_key(|(connection_id, _)| **connection_id);
    connections
        .into_iter()
        .flat_map(|(_, windows)| windows.iter().cloned())
        .collect()
}

pub async fn get_state(State(state): State<AppState>) -> impl IntoResponse {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

    let msg = BridgeMessage {
        command: RemoteCommand::GetState,
        reply: Some(reply_tx),
    };

    if state.bridge_tx.send(msg).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "bridge unavailable"})),
        )
            .into_response();
    }

    match reply_rx.await {
        Ok(CommandResult::Ok(Some(mut value))) => {
            let windows = state
                .remote_windows
                .read()
                .map(|map| declared_windows(&map))
                .unwrap_or_default();
            if !windows.is_empty() {
                value["windows"] = serde_json::json!(windows);
            }
            (StatusCode::OK, Json(value)).into_response()
        }
        Ok(CommandResult::Err(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
        _ => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "unexpected response"})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str) -> ApiWindow {
        ApiWindow {
            id: id.into(),
            kind: if id == "main" { "main" } else { "extra" }.into(),
            active: false,
            focused_project_id: None,
            focused_terminal_id: None,
            fullscreen: None,
            visible_project_ids: Vec::new(),
            folder_filter: None,
            bounds: None,
            sidebar_open: None,
        }
    }

    #[test]
    fn declared_windows_are_listed_in_connection_order() {
        let by_connection = HashMap::from([
            (7, vec![window("main"), window("abc")]),
            (3, vec![window("def")]),
        ]);
        let ids: Vec<String> = declared_windows(&by_connection)
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(ids, ["def", "main", "abc"]);
        assert!(declared_windows(&HashMap::new()).is_empty());
    }
}
