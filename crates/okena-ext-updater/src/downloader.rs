use crate::status::{UpdateInfo, UpdateStatus};
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DOWNLOAD_DIR_PREFIX: &str = "download-";

/// Download an asset, reporting progress via `UpdateInfo`.
pub async fn download_asset(
    url: String,
    asset_name: String,
    version: String,
    update_info: UpdateInfo,
    cancel_token: u64,
    sha256: String,
) -> Result<PathBuf> {
    download_asset_in(
        url,
        asset_name,
        version,
        update_info,
        cancel_token,
        sha256,
        crate::process::get_config_dir().join("updates"),
    )
    .await
}

async fn download_asset_in(
    url: String,
    asset_name: String,
    version: String,
    update_info: UpdateInfo,
    cancel_token: u64,
    sha256: String,
    updates_dir: PathBuf,
) -> Result<PathBuf> {
    let (staging_dir, path) = smol::unblock(move || {
        if Path::new(&asset_name).file_name() != Some(std::ffi::OsStr::new(&asset_name)) {
            anyhow::bail!("invalid update asset name");
        }
        std::fs::create_dir_all(&updates_dir).context("failed to create updates directory")?;
        prune_abandoned_downloads(&updates_dir);
        let staging_dir = tempfile::Builder::new()
            .prefix(&format!("{DOWNLOAD_DIR_PREFIX}{}-", std::process::id()))
            .tempdir_in(&updates_dir)
            .context("failed to create update staging directory")?;
        let path = staging_dir.path().join(&asset_name);
        download_blocking(&url, &path, &version, &update_info, cancel_token)?;
        anyhow::ensure!(
            !update_info.is_cancelled(cancel_token),
            "download cancelled"
        );
        verify_checksum(&path, &asset_name, &sha256)?;
        anyhow::ensure!(
            !update_info.is_cancelled(cancel_token),
            "download cancelled"
        );
        Ok::<_, anyhow::Error>((staging_dir, path))
    })
    .await?;
    let _ = staging_dir.keep();
    Ok(path)
}

// Ready is held in memory. A restart or crash can leave its verified archive behind.
// Keep concurrent downloads, including work owned by a still-running daemon.
fn prune_abandoned_downloads(updates_dir: &Path) {
    // The earlier updater used a flat platform archive and extraction directory.
    let _ = std::fs::remove_file(updates_dir.join(crate::checker::platform_asset_name()));
    let _ = std::fs::remove_dir_all(updates_dir.join("extracted"));
    let Ok(entries) = std::fs::read_dir(updates_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|name| name.strip_prefix(DOWNLOAD_DIR_PREFIX))
            .and_then(|name| name.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<u32>().ok());
        if let Some(pid @ 1..=2147483647) = pid
            && pid != std::process::id()
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && !okena_core::process::is_process_alive(pid)
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn download_blocking(
    url: &str,
    dest: &Path,
    version: &str,
    update_info: &UpdateInfo,
    cancel_token: u64,
) -> Result<()> {
    anyhow::ensure!(
        update_info.set_status_if_current(
            cancel_token,
            UpdateStatus::Downloading {
                version: version.to_string(),
                progress: 0,
            },
        ),
        "download cancelled"
    );

    let resp = okena_transport::http::stream(
        okena_transport::http::HttpRequest::get(url)
            .timeout(Duration::from_secs(3600))
            .label("updater.download"),
    )
    .context("failed to start download")?
    .error_for_status()
    .context("server returned an error status")?;

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut last_pct: u8 = 0;
    let mut file = std::fs::File::create(dest).context("failed to create download file")?;

    let mut reader = resp;
    let mut buf = [0u8; 65536];

    loop {
        anyhow::ensure!(
            !update_info.is_cancelled(cancel_token),
            "download cancelled"
        );

        let n = std::io::Read::read(&mut reader, &mut buf).context("download read error")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .context("failed to write download")?;
        downloaded += n as u64;

        if total > 0 {
            let pct = ((downloaded as f64 / total as f64) * 100.0).min(100.0) as u8;
            if pct != last_pct {
                last_pct = pct;
                anyhow::ensure!(
                    update_info.set_status_if_current(
                        cancel_token,
                        UpdateStatus::Downloading {
                            version: version.to_string(),
                            progress: pct,
                        },
                    ),
                    "download cancelled"
                );
            }
        }
    }

    file.flush().context("failed to flush download")?;

    if total > 0 && downloaded != total {
        anyhow::bail!(
            "incomplete download: got {} bytes, expected {} bytes",
            downloaded,
            total
        );
    }

    if downloaded == 0 {
        anyhow::bail!("downloaded file is empty");
    }

    log::info!("Downloaded {} ({} bytes)", dest.display(), downloaded);
    Ok(())
}

/// `expected_hash` is the release's `SHA256SUMS` entry, read once by the checker.
fn verify_checksum(file_path: &Path, asset_name: &str, expected_hash: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    log::info!("Verifying checksum for {asset_name}");

    let mut file =
        std::fs::File::open(file_path).context("failed to open downloaded file for checksum")?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).context("read error during checksum")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let actual_hash = format!("{:x}", hasher.finalize());

    if !actual_hash.eq_ignore_ascii_case(expected_hash) {
        anyhow::bail!(
            "checksum mismatch: expected {}, got {}",
            expected_hash,
            actual_hash
        );
    }

    log::info!("Checksum verified for {asset_name}: sha256 {actual_hash}");
    Ok(())
}

/// Discard an archive returned by `download_asset` and any extracted files.
pub(crate) fn discard_download(path: &Path) {
    if let Some(staging_dir) = path.parent()
        && staging_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(DOWNLOAD_DIR_PREFIX))
        && let Err(error) = std::fs::remove_dir_all(staging_dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("Could not discard update at {}: {error}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    fn serve(body: String) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                    break;
                }
            }

            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        (url, server)
    }

    fn sha256_hex(body: &str) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(body))
    }

    #[test]
    fn downloads_of_the_same_asset_have_independent_staging_directories() {
        let updates_dir = tempfile::tempdir().unwrap();
        let info = UpdateInfo::new("0.1.0".into());
        let asset_name = "okena.zip";

        let (url, server) = serve("first archive".into());
        let first = smol::block_on(download_asset_in(
            url,
            asset_name.into(),
            "0.2.0".into(),
            info.clone(),
            info.current_token(),
            sha256_hex("first archive"),
            updates_dir.path().to_path_buf(),
        ))
        .unwrap();
        server.join().unwrap();

        let (url, server) = serve("second archive".into());
        let second = smol::block_on(download_asset_in(
            url,
            asset_name.into(),
            "0.2.0".into(),
            info.clone(),
            info.current_token(),
            sha256_hex("second archive"),
            updates_dir.path().to_path_buf(),
        ))
        .unwrap();
        server.join().unwrap();

        assert_ne!(first.parent(), second.parent());
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "first archive");
        let first_dir = first.parent().unwrap();
        std::fs::create_dir(first_dir.join("extracted")).unwrap();
        discard_download(&first);
        assert!(!first_dir.exists());
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second archive");
        std::fs::remove_file(&second).unwrap();
        discard_download(&second);
        assert_eq!(std::fs::read_dir(updates_dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn checksum_mismatch_discards_the_staged_download() {
        let updates_dir = tempfile::tempdir().unwrap();
        let info = UpdateInfo::new("0.1.0".into());
        let asset_name = "okena.zip";
        let (url, archive_server) = serve("archive contents".into());

        let error = smol::block_on(download_asset_in(
            url,
            asset_name.into(),
            "0.2.0".into(),
            info.clone(),
            info.current_token(),
            "0".repeat(64),
            updates_dir.path().to_path_buf(),
        ))
        .unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error}");
        archive_server.join().unwrap();
        assert_eq!(std::fs::read_dir(updates_dir.path()).unwrap().count(), 0);
    }
}
