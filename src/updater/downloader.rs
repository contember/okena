use crate::updater::UpdateInfo;
use crate::updater::UpdateStatus;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Download an asset to `~/.config/okena/updates/`, reporting progress via `UpdateInfo`.
/// If a `checksum_url` is provided, downloads the SHA256SUMS file and verifies the asset.
/// Returns the path to the downloaded file.
pub async fn download_asset(
    url: String,
    asset_name: String,
    version: String,
    update_info: UpdateInfo,
    cancel_token: u64,
    checksum_url: Option<String>,
) -> Result<PathBuf> {
    smol::unblock(move || {
        let path = download_blocking(&url, &asset_name, &version, &update_info, cancel_token)?;
        if let Some(cs_url) = checksum_url {
            verify_checksum(&path, &asset_name, &cs_url)?;
        }
        Ok(path)
    }).await
}

fn download_blocking(
    url: &str,
    asset_name: &str,
    version: &str,
    update_info: &UpdateInfo,
    cancel_token: u64,
) -> Result<PathBuf> {
    let updates_dir = crate::workspace::persistence::get_config_dir().join("updates");
    std::fs::create_dir_all(&updates_dir)
        .context("failed to create updates directory")?;

    // Clean previous downloads before starting a new one
    cleanup_updates_dir();

    let dest = updates_dir.join(asset_name);

    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(3600))
        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")?;

    let resp = client
        .get(url)
        .send()
        .context("failed to start download")?
        .error_for_status()
        .context("server returned an error status")?;

    let total = resp.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut last_pct: u8 = 0;
    let mut file = std::fs::File::create(&dest)
        .context("failed to create download file")?;

    let mut reader = resp;
    let mut buf = [0u8; 65536];

    // Set initial downloading status so the UI shows progress immediately
    update_info.set_status(UpdateStatus::Downloading {
        version: version.to_string(),
        progress: 0,
    });

    loop {
        if update_info.is_cancelled(cancel_token) {
            // Clean up partial download
            drop(file);
            let _ = std::fs::remove_file(&dest);
            anyhow::bail!("download cancelled");
        }

        let n = std::io::Read::read(&mut reader, &mut buf)
            .context("download read error")?;
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
                update_info.set_status(UpdateStatus::Downloading {
                    version: version.to_string(),
                    progress: pct,
                });
            }
        }
    }

    file.flush().context("failed to flush download")?;

    // Validate downloaded size when server reported content-length
    if total > 0 && downloaded != total {
        let _ = std::fs::remove_file(&dest);
        anyhow::bail!(
            "incomplete download: got {} bytes, expected {} bytes",
            downloaded,
            total
        );
    }

    // Reject zero-byte downloads
    if downloaded == 0 {
        let _ = std::fs::remove_file(&dest);
        anyhow::bail!("downloaded file is empty");
    }

    log::info!("Downloaded {} ({} bytes)", asset_name, downloaded);
    Ok(dest)
}

/// Verify the SHA256 checksum of a downloaded file against a remote SHA256SUMS file.
fn verify_checksum(file_path: &Path, asset_name: &str, checksum_url: &str) -> Result<()> {
    use sha2::{Sha256, Digest};
    use std::io::Read;

    log::info!("Verifying checksum for {}", asset_name);

    // Download the checksums file
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client for checksum")?;

    let body = client
        .get(checksum_url)
        .send()
        .context("failed to fetch checksum file")?
        .error_for_status()
        .context("checksum file request failed")?
        .text()
        .context("failed to read checksum file")?;

    // Parse SHA256SUMS format: "<hex>  <filename>" or "<hex> <filename>"
    let expected_hash = body
        .lines()
        .find_map(|line| {
            let parts: Vec<&str> = line.splitn(2, |c: char| c.is_whitespace()).collect();
            if parts.len() == 2 && parts[1].trim() == asset_name {
                Some(parts[0].to_lowercase())
            } else {
                None
            }
        })
        .with_context(|| format!("no checksum found for '{}' in SHA256SUMS", asset_name))?;

    // Compute actual hash
    let mut file = std::fs::File::open(file_path)
        .context("failed to open downloaded file for checksum")?;
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

    if actual_hash != expected_hash {
        // Remove the corrupt download
        let _ = std::fs::remove_file(file_path);
        anyhow::bail!(
            "checksum mismatch: expected {}, got {}",
            expected_hash,
            actual_hash
        );
    }

    log::info!("Checksum verified for {}", asset_name);
    Ok(())
}

/// Remove all files and directories in `~/.config/okena/updates/`.
pub fn cleanup_updates_dir() {
    let updates_dir = crate::workspace::persistence::get_config_dir().join("updates");
    if updates_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&updates_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}
