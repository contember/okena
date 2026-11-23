use super::UpdateChannel;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

#[derive(Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Preference {
    pub channel: UpdateChannel,
    revision: u64,
}

impl Preference {
    /// Before a channel is picked, an installation follows the stream its build came from.
    pub fn initial() -> Self {
        Self::unselected(if okena_core::build_info::BUILD_INFO.nightly.is_some() {
            UpdateChannel::Nightly
        } else {
            UpdateChannel::Release
        })
    }

    fn unselected(channel: UpdateChannel) -> Self {
        Self {
            channel,
            revision: 0,
        }
    }

    pub fn selecting(self, channel: UpdateChannel) -> Self {
        Self {
            channel,
            revision: self.revision.wrapping_add(1),
        }
    }
}

/// Both binaries share one installation directory. A portable preview gets its own preference.
pub(super) fn path(config_root: &Path, install_dir: &Path) -> PathBuf {
    let directory = install_dir
        .canonicalize()
        .unwrap_or_else(|_| install_dir.to_owned());
    let id = Sha256::digest(directory.as_os_str().as_encoded_bytes());
    config_root
        .join("installations")
        .join(format!("{id:x}"))
        .join("update-channel.json")
}

/// A missing or unreadable file yields `initial`; the next selection overwrites it.
pub(super) fn load(path: &Path, initial: Preference) -> Result<Preference> {
    match File::open(path) {
        Ok(file) => match serde_json::from_reader(file) {
            Ok(pref) => Ok(pref),
            Err(error) if !error.is_io() => Ok(initial),
            Err(error) => Err(error.into()),
        },
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(initial),
        Err(error) => Err(error.into()),
    }
}

/// Serializes preference writes with installation. An installer holds this file until restart.
pub(super) fn lock(path: &Path) -> Result<File> {
    let directory = path
        .parent()
        .context("missing installation preferences directory")?;
    std::fs::create_dir_all(directory)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    file.try_lock()
        .context("another profile is changing channels or waiting to finish an update; retry after restarting that profile")?;
    Ok(file)
}

pub(super) fn save(path: &Path, pref: Preference) -> Result<()> {
    let directory = path
        .parent()
        .context("missing installation preferences directory")?;
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(&mut file, &pref)?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nightly_without_a_stored_choice_stays_on_nightly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-channel.json");
        let nightly = Preference::unselected(UpdateChannel::Nightly);
        assert_eq!(
            load(&path, nightly).unwrap().channel,
            UpdateChannel::Nightly
        );

        std::fs::write(&path, "{\"chan").unwrap();
        assert_eq!(
            load(&path, nightly).unwrap().channel,
            UpdateChannel::Nightly
        );

        save(&path, nightly.selecting(UpdateChannel::Release)).unwrap();
        assert_eq!(
            load(&path, nightly).unwrap().channel,
            UpdateChannel::Release
        );
    }
}
