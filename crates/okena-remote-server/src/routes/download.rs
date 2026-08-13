use crate::bridge::{BridgeMessage, CommandResult, RemoteCommand};
use crate::routes::AppState;
use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use okena_core::api::{ActionRequest, FileDownloadRequest, ResolvedPath, ResolvedPathKind};
use tokio_util::io::ReaderStream;

async fn resolve_file(
    state: &AppState,
    request: FileDownloadRequest,
) -> Result<ResolvedPath, Response> {
    let action = match request {
        FileDownloadRequest::Project {
            project_id,
            relative_path,
        } => ActionRequest::ResolveProjectPath {
            project_id,
            relative_path,
        },
        FileDownloadRequest::Terminal { terminal_id, path } => {
            ActionRequest::ResolveTerminalPath { terminal_id, path }
        }
        FileDownloadRequest::Path {
            root,
            relative_path,
        } => ActionRequest::ResolvePathInScope {
            root,
            relative_path,
        },
    };
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    state
        .bridge_tx
        .send(BridgeMessage {
            command: RemoteCommand::Action(action),
            reply: Some(reply_tx),
        })
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "bridge unavailable").into_response())?;
    match reply_rx.await {
        Ok(CommandResult::Ok(Some(value))) => serde_json::from_value(value).map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("invalid file response: {error}"),
            )
                .into_response()
        }),
        Ok(CommandResult::Err(error)) => Err((StatusCode::BAD_REQUEST, error).into_response()),
        Ok(_) => Err((StatusCode::INTERNAL_SERVER_ERROR, "missing file response").into_response()),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "command processing failed",
        )
            .into_response()),
    }
}

fn safe_download_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|character| match character {
            '"' | '\r' | '\n' | '/' | '\\' => '_',
            _ => character,
        })
        .collect();
    if safe.is_empty() {
        "download".to_string()
    } else {
        safe
    }
}

pub async fn post_download(
    State(state): State<AppState>,
    Json(request): Json<FileDownloadRequest>,
) -> Response {
    let file = match resolve_file(&state, request).await {
        Ok(file) => file,
        Err(response) => return response,
    };
    if file.kind != ResolvedPathKind::File {
        return (StatusCode::BAD_REQUEST, "path is not a regular file").into_response();
    }
    let source = match tokio::fs::File::open(&file.canonical_path).await {
        Ok(source) => source,
        Err(error) => {
            return (StatusCode::NOT_FOUND, format!("cannot open file: {error}")).into_response();
        }
    };
    let disposition = format!(
        "attachment; filename=\"{}\"",
        safe_download_name(&file.name)
    );
    let mut response = Response::new(Body::from_stream(ReaderStream::new(source)));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(length) = HeaderValue::from_str(&file.size.to_string()) {
        response.headers_mut().insert(CONTENT_LENGTH, length);
    }
    if let Ok(disposition) = HeaderValue::from_str(&disposition) {
        response
            .headers_mut()
            .insert(CONTENT_DISPOSITION, disposition);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::safe_download_name;

    #[test]
    fn download_name_cannot_inject_headers_or_paths() {
        assert_eq!(safe_download_name("../a\r\n.txt"), ".._a__.txt");
        assert_eq!(safe_download_name(""), "download");
    }
}
