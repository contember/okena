//! Reading a file that must be a regular file, so that a FIFO, a device or a
//! symlink swapped in for it can neither block the reader nor grow its memory.

use std::fmt;
use std::io::{self, Read};
use std::path::Path;

#[derive(Debug)]
pub enum ReadRegularFileError {
    /// Opening or reading failed; a symlink in the last component lands here
    /// on Unix (`O_NOFOLLOW`).
    Io(io::Error),
    /// A FIFO, socket, device or directory.
    NotRegular,
    /// The file is longer than the caller's limit; nothing was read.
    TooLarge { len: u64 },
}

impl fmt::Display for ReadRegularFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(f),
            Self::NotRegular => f.write_str("not a regular file"),
            Self::TooLarge { len } => write!(f, "file too large ({len} bytes)"),
        }
    }
}

impl std::error::Error for ReadRegularFileError {}

impl From<ReadRegularFileError> for io::Error {
    fn from(error: ReadRegularFileError) -> Self {
        match error {
            ReadRegularFileError::Io(error) => error,
            ReadRegularFileError::NotRegular => {
                io::Error::new(io::ErrorKind::InvalidData, "not a regular file")
            }
            error @ ReadRegularFileError::TooLarge { .. } => {
                io::Error::new(io::ErrorKind::FileTooLarge, error.to_string())
            }
        }
    }
}

/// Reads the regular file at `path`, at most `max_len` bytes. Type and length
/// come from the open file, so swapping the last path component after any
/// earlier check can neither redirect the read nor make it endless: the read
/// stops at the length seen at open, even if the file keeps growing.
pub fn read(path: &Path, max_len: u64) -> Result<Vec<u8>, ReadRegularFileError> {
    let file = open_no_follow(path).map_err(ReadRegularFileError::Io)?;
    let meta = file.metadata().map_err(ReadRegularFileError::Io)?;
    if !meta.is_file() {
        return Err(ReadRegularFileError::NotRegular);
    }
    if meta.len() > max_len {
        return Err(ReadRegularFileError::TooLarge { len: meta.len() });
    }
    let mut bytes = Vec::new();
    usize::try_from(meta.len())
        .ok()
        .and_then(|len| bytes.try_reserve_exact(len).ok())
        .ok_or_else(|| ReadRegularFileError::Io(io::ErrorKind::OutOfMemory.into()))?;
    file.take(meta.len())
        .read_to_end(&mut bytes)
        .map_err(ReadRegularFileError::Io)?;
    Ok(bytes)
}

/// Opens `path` for reading without following a symlink in its last component
/// and without blocking on a FIFO.
#[cfg(unix)]
fn open_no_follow(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
}

#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(path)
}

#[cfg(all(test, unix))]
mod tests {
    use super::{ReadRegularFileError, read};
    use std::path::Path;

    /// Fails the test instead of hanging it when a read blocks or never ends.
    fn read_within_seconds(path: &Path, max_len: u64) -> Result<Vec<u8>, ReadRegularFileError> {
        let path = path.to_path_buf();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(read(&path, max_len));
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("did not finish: it read a FIFO or a device")
    }

    #[test]
    fn reads_a_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("notes.md");
        std::fs::write(&path, "hello\n").expect("write file");
        assert_eq!(read_within_seconds(&path, 6).expect("read"), b"hello\n");
    }

    #[test]
    fn refuses_a_fifo_without_blocking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .expect("run mkfifo");
        assert!(status.success(), "mkfifo {}", path.display());
        assert!(matches!(
            read_within_seconds(&path, u64::MAX),
            Err(ReadRegularFileError::NotRegular)
        ));
    }

    #[test]
    fn refuses_a_device_and_a_symlink_to_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let link = dir.path().join("zero");
        std::os::unix::fs::symlink("/dev/zero", &link).expect("symlink");
        assert!(matches!(
            read_within_seconds(&link, u64::MAX),
            Err(ReadRegularFileError::Io(_))
        ));
        assert!(matches!(
            read_within_seconds(Path::new("/dev/zero"), u64::MAX),
            Err(ReadRegularFileError::NotRegular)
        ));
    }

    #[test]
    fn refuses_a_file_over_the_limit_without_reading_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.log");
        std::fs::write(&path, "1234567").expect("write file");
        assert!(matches!(
            read_within_seconds(&path, 6),
            Err(ReadRegularFileError::TooLarge { len: 7 })
        ));
    }
}
