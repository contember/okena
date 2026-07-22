//! Materialize pasted images and dropped files on the daemon host, then paste
//! their server-local paths into the target terminal.
//!
//! The image must live where the terminal's process runs: the client's local
//! clipboard isn't reachable from here, and a client-local path would point at
//! a file that doesn't exist on the server. So the bytes travel over HTTP and
//! the file is materialised server-side. This mirrors the desktop's local
//! image-paste (write temp file → `send_paste(path)`), just across the wire.

use crate::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::routes::AppState;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use std::sync::atomic::{AtomicU64, Ordering};

/// Max accepted image upload (20 MiB). Clipboard screenshots — especially
/// retina full-screen PNGs — routinely exceed the global 1 MiB body cap, so
/// this route raises its own limit (see `build_router`).
pub const IMAGE_UPLOAD_LIMIT: usize = 20 * 1024 * 1024;
/// Max accepted dropped file upload (64 MiB).
pub const FILE_UPLOAD_LIMIT: usize = 64 * 1024 * 1024;

pub async fn post_paste_image(
    State(state): State<AppState>,
    Path(terminal_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let mime = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("image/png");
    let ext = image_ext_for_mime(mime);

    materialize_and_paste(state, terminal_id, body, ext, false, "image").await
}

pub async fn post_paste_file(
    State(state): State<AppState>,
    Path(terminal_id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let extension = headers
        .get("x-okena-file-extension")
        .and_then(|value| value.to_str().ok())
        .and_then(safe_file_extension)
        .unwrap_or("bin");
    materialize_and_paste(state, terminal_id, body, extension, true, "file").await
}

async fn materialize_and_paste(
    state: AppState,
    terminal_id: String,
    body: Bytes,
    extension: &str,
    append_space: bool,
    kind: &str,
) -> Response {
    if body.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": format!("empty {kind} body")})),
        )
            .into_response();
    }

    let filename = format!("okena-remote-paste-{}.{}", next_token(), extension);
    let path = std::env::temp_dir().join(filename);

    if let Err(e) = tokio::fs::write(&path, &body).await {
        log::error!("paste upload: failed to write {}: {}", path.display(), e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("failed to write {kind}: {e}")})),
        )
            .into_response();
    }

    let path_str = path.to_string_lossy().into_owned();
    let text = if append_space {
        format!("{path_str} ")
    } else {
        path_str.clone()
    };
    let command = RemoteCommand::PastePath { terminal_id, text };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let msg = BridgeMessage {
        command,
        reply: Some(reply_tx),
    };

    if state.bridge_tx.send(msg).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "bridge unavailable"})),
        )
            .into_response();
    }

    match reply_rx.await {
        Ok(CommandResult::Ok(_))
        | Ok(CommandResult::OkBytes(_))
        | Ok(CommandResult::OkSnapshot { .. }) => {
            (StatusCode::OK, Json(serde_json::json!({"path": path_str}))).into_response()
        }
        Ok(CommandResult::Err(e)) => {
            let _ = tokio::fs::remove_file(&path).await;
            (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": e})),
            )
                .into_response()
        }
        Err(_) => {
            let _ = tokio::fs::remove_file(&path).await;
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "command processing failed"})),
            )
                .into_response()
        }
    }
}

fn safe_file_extension(extension: &str) -> Option<&str> {
    (!extension.is_empty()
        && extension.len() <= 16
        && extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric()))
    .then_some(extension)
}

/// File extension for a clipboard image MIME type. Mirrors the desktop
/// `paste_filename` mapping (GPUI `ImageFormat`), defaulting to `png` for
/// anything unrecognised. Tolerates parameters (`image/png; charset=…`).
fn image_ext_for_mime(mime: &str) -> &'static str {
    let mime = mime.split(';').next().unwrap_or(mime).trim();
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        "image/bmp" | "image/x-bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/x-portable-anymap" => "pnm",
        _ => "png",
    }
}

/// Per-process token keeping temp filenames unique within a run (monotonic
/// counter) and across runs (wall clock). Collisions would only overwrite a
/// sibling paste, but uniqueness is cheap.
fn next_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{n:x}")
}

#[cfg(test)]
mod tests {
    use super::{image_ext_for_mime, safe_file_extension};

    #[test]
    fn maps_known_mime_types() {
        assert_eq!(image_ext_for_mime("image/png"), "png");
        assert_eq!(image_ext_for_mime("image/jpeg"), "jpg");
        assert_eq!(image_ext_for_mime("image/svg+xml"), "svg");
        assert_eq!(image_ext_for_mime("image/x-icon"), "ico");
    }

    #[test]
    fn tolerates_content_type_parameters() {
        assert_eq!(image_ext_for_mime("image/png; charset=binary"), "png");
        assert_eq!(image_ext_for_mime(" image/webp "), "webp");
    }

    #[test]
    fn defaults_unknown_to_png() {
        assert_eq!(image_ext_for_mime("application/octet-stream"), "png");
        assert_eq!(image_ext_for_mime(""), "png");
    }

    #[test]
    fn accepts_only_safe_file_extensions() {
        assert_eq!(safe_file_extension("tar"), Some("tar"));
        assert_eq!(safe_file_extension(""), None);
        assert_eq!(safe_file_extension("../sh"), None);
        assert_eq!(safe_file_extension("extension-way-too-long"), None);
    }
}
