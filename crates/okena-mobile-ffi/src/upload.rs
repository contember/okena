//! Files picked on the phone, uploaded to the server host: the terminal's
//! process (or Claude) can only read a path that exists over there.

use std::path::{Path, PathBuf};

use okena_core::api::ApiUploadResponse;
use okena_transport::remote_action::{JsonRequestError, post_bytes_async};

use crate::MobileFfiError;
use crate::client::manager::ConnectionManager;

/// Mirrors `FILE_UPLOAD_LIMIT` of the server's upload routes.
const MAX_UPLOAD_BYTES: u64 = 64 * 1024 * 1024;
const EXTENSION_HEADER: &str = "X-Okena-File-Extension";

/// Upload the file at `local_path` and return its path on the server host.
pub(crate) async fn upload(conn_id: &str, local_path: &str) -> Result<String, MobileFfiError> {
    let (config, token) = ConnectionManager::get().http_auth(conn_id)?;
    let local_path = PathBuf::from(local_path);
    let extension = safe_extension(&local_path);
    let bytes = tokio::task::spawn_blocking(move || read_upload(&local_path))
        .await
        .map_err(|error| action_error(format!("Cannot read the file: {error}")))??;

    let headers: Vec<(&str, &str)> = extension
        .as_deref()
        .map(|extension| (EXTENSION_HEADER, extension))
        .into_iter()
        .collect();
    let response: ApiUploadResponse =
        post_bytes_async(&config, &token, &["v1", "files", "upload"], &headers, bytes)
            .await
            .map_err(upload_error)?;
    Ok(response.path)
}

fn read_upload(path: &Path) -> Result<Vec<u8>, MobileFfiError> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| action_error(format!("Cannot read the file: {error}")))?;
    if !metadata.is_file() {
        return Err(action_error("Not a file".to_string()));
    }
    if metadata.len() == 0 {
        return Err(action_error("The file is empty".to_string()));
    }
    if metadata.len() > MAX_UPLOAD_BYTES {
        return Err(action_error(format!(
            "The file is larger than {} MiB",
            MAX_UPLOAD_BYTES / 1024 / 1024
        )));
    }
    std::fs::read(path).map_err(|error| action_error(format!("Cannot read the file: {error}")))
}

/// The extension the server accepts (ASCII alphanumeric, at most 16 chars);
/// the server falls back to `bin` without one.
fn safe_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?;
    (!extension.is_empty()
        && extension.len() <= 16
        && extension.chars().all(|c| c.is_ascii_alphanumeric()))
    .then(|| extension.to_ascii_lowercase())
}

/// The upload routes answer errors as `{"error": "..."}`.
fn upload_error(error: JsonRequestError) -> MobileFfiError {
    let message = match error {
        JsonRequestError::Status { status, body } => {
            let error = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| value["error"].as_str().map(str::to_string));
            match error {
                Some(error) => error,
                None if body.trim().is_empty() => format!("Server returned {status}"),
                None => body.trim().to_string(),
            }
        }
        JsonRequestError::Transport(message) => message,
    };
    action_error(message)
}

fn action_error(message: String) -> MobileFfiError {
    MobileFfiError::Action { message }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(error: MobileFfiError) -> String {
        match error {
            MobileFfiError::Action { message } => message,
        }
    }

    #[test]
    fn extensions_are_sanitised_for_the_server() {
        assert_eq!(
            safe_extension(Path::new("/c/Shot.PNG")),
            Some("png".to_string())
        );
        assert_eq!(
            safe_extension(Path::new("/c/archive.tar.gz")),
            Some("gz".to_string())
        );
        assert_eq!(safe_extension(Path::new("/c/noext")), None);
        assert_eq!(safe_extension(Path::new("/c/a.p-n")), None);
    }

    #[test]
    fn server_error_bodies_are_unwrapped() {
        let error = JsonRequestError::Status {
            status: 400,
            body: r#"{"error":"empty file body"}"#.to_string(),
        };
        assert_eq!(message(upload_error(error)), "empty file body");

        let plain = JsonRequestError::Status {
            status: 413,
            body: "length limit exceeded".to_string(),
        };
        assert_eq!(message(upload_error(plain)), "length limit exceeded");
    }

    #[test]
    fn empty_and_missing_files_are_refused_before_sending() {
        let directory =
            std::env::temp_dir().join(format!("okena-upload-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let empty = directory.join("empty.txt");
        std::fs::write(&empty, "").unwrap();

        assert_eq!(
            message(read_upload(&empty).unwrap_err()),
            "The file is empty"
        );
        assert!(
            message(read_upload(&directory.join("missing.txt")).unwrap_err())
                .starts_with("Cannot read the file: ")
        );
        assert_eq!(message(read_upload(&directory).unwrap_err()), "Not a file");

        let file = directory.join("a.txt");
        std::fs::write(&file, "hi").unwrap();
        assert_eq!(read_upload(&file).unwrap(), b"hi");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
