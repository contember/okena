//! PTY IO that can be interrupted when a terminal is torn down.

use super::PtyShutdownState;
use portable_pty::MasterPty;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::Arc;

type ReaderWriter = (Box<dyn Read + Send>, Box<dyn Write + Send>);

pub(super) fn reader_writer(
    master: &dyn MasterPty,
    shutdown: &Arc<PtyShutdownState>,
) -> anyhow::Result<ReaderWriter> {
    let fd = master
        .as_raw_fd()
        .ok_or_else(|| anyhow::anyhow!("PTY has no master fd"))?;
    // Keep a poll descriptor alive after shutdown drops the MasterPty. dup
    // shares the open-file description, so O_NONBLOCK applies to the portable
    // reader/writer clones too (including the writer's EOT-on-drop backstop).
    // SAFETY: fd belongs to the live master; fcntl takes integer arguments.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: fcntl returned a new descriptor, now owned exclusively here.
    let fd = Arc::new(unsafe { OwnedFd::from_raw_fd(duplicate) });
    // SAFETY: fd remains live and fcntl takes integer arguments.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error().into());
    }
    let (wake_writer, wake_reader) = UnixStream::pair()?;
    wake_writer.set_nonblocking(true)?;
    shutdown.install_io_waker(wake_writer);
    let wake_reader = Arc::new(wake_reader);
    Ok((
        Box::new(CancellableIo {
            inner: master.try_clone_reader()?,
            fd: fd.clone(),
            wake: wake_reader.clone(),
            shutdown: shutdown.clone(),
        }),
        Box::new(CancellableIo {
            inner: master.take_writer()?,
            fd,
            wake: wake_reader,
            shutdown: shutdown.clone(),
        }),
    ))
}

struct CancellableIo<T> {
    inner: T,
    fd: Arc<OwnedFd>,
    wake: Arc<UnixStream>,
    shutdown: Arc<PtyShutdownState>,
}

impl<T> CancellableIo<T> {
    fn check_shutdown(&self) -> io::Result<()> {
        if self.shutdown.is_broken() {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "PTY shut down"))
        } else {
            Ok(())
        }
    }

    fn wait_ready(&self, events: libc::c_short) -> io::Result<()> {
        loop {
            self.check_shutdown()?;
            let mut fds = [
                libc::pollfd {
                    fd: self.fd.as_raw_fd(),
                    events,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.wake.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            // SAFETY: both descriptors are owned by this wrapper and fds is a
            // valid two-element pollfd array. No timeout/polling is necessary:
            // the shutdown byte wakes both threads, and is never consumed.
            let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            self.check_shutdown()?;
            if fds[0].revents != 0 {
                return Ok(());
            }
        }
    }
}

impl<T: Read> Read for CancellableIo<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check_shutdown()?;
            match self.inner.read(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait_ready(libc::POLLIN)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
}

impl<T: Write> Write for CancellableIo<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        loop {
            self.check_shutdown()?;
            match self.inner.write(buf) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.wait_ready(libc::POLLOUT)?
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;
    use std::sync::mpsc;
    use std::time::Duration;

    fn raw_pty() -> (portable_pty::PtyPair, std::fs::File) {
        let pair = portable_pty::native_pty_system()
            .openpty(portable_pty::PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let slave = OpenOptions::new()
            .read(true)
            .write(true)
            .open(pair.master.tty_name().unwrap())
            .unwrap();
        // SAFETY: the live slave fd and initialized termios buffer are valid.
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut termios), 0);
            libc::cfmakeraw(&mut termios);
            assert_eq!(
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios),
                0
            );
            let flags = libc::fcntl(slave.as_raw_fd(), libc::F_GETFL);
            assert_eq!(
                libc::fcntl(slave.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK),
                0
            );
        }
        (pair, slave)
    }

    /// An exited attach client does not guarantee the slave is closed. A
    /// retained slave with a full input buffer must not strand a teardown
    /// worker inside write_all, blocking all subsequent worktree removals.
    #[test]
    fn shutdown_interrupts_a_write_to_a_full_pty_with_a_retained_slave() {
        let (pair, mut slave) = raw_pty();
        let shutdown = Arc::new(PtyShutdownState::new(
            "full-pty".to_string(),
            super::super::PtyGeneration(1),
        ));
        let (_reader, mut writer) = reader_writer(pair.master.as_ref(), &shutdown).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = writer.write_all(&vec![b'x'; 65536]);
            drop(writer);
            let _ = done_tx.send(result);
        });
        assert!(
            matches!(
                done_rx.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "fixture must fill the PTY before shutdown"
        );
        shutdown.mark_broken();
        let result = done_rx.recv_timeout(Duration::from_secs(1));

        // Release a blocked pre-fix writer before asserting, so the red test
        // never leaves a stuck thread or a live process behind.
        if result.is_err() {
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            let mut buffer = [0; 65536];
            while !worker.is_finished() && std::time::Instant::now() < deadline {
                match slave.read(&mut buffer) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture drain failed: {error}"),
                }
            }
        }
        assert!(
            worker.is_finished() || result.is_ok(),
            "fixture writer must be releasable"
        );
        worker.join().unwrap();
        assert!(
            result.is_ok(),
            "terminal teardown timed out on a PTY write despite shutdown"
        );
        assert!(
            result.unwrap().is_err(),
            "shutdown must cancel the pending write"
        );
    }

    #[test]
    fn shutdown_wakes_an_idle_reader_after_master_is_dropped() {
        let (pair, mut slave) = raw_pty();
        let shutdown = Arc::new(PtyShutdownState::new(
            "idle-reader".to_string(),
            super::super::PtyGeneration(1),
        ));
        let (mut reader, _writer) = reader_writer(pair.master.as_ref(), &shutdown).unwrap();
        drop(pair.master);
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = done_tx.send(reader.read(&mut [0; 1]));
        });
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        shutdown.mark_broken();
        let result = done_rx.recv_timeout(Duration::from_secs(1));
        if result.is_err() {
            slave.write_all(b"x").unwrap();
        }
        worker.join().unwrap();
        assert_eq!(
            result.unwrap().unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
    }

    #[test]
    fn io_preserves_bytes_and_wakes_when_output_arrives() {
        let (pair, mut slave) = raw_pty();
        let shutdown = Arc::new(PtyShutdownState::new(
            "round-trip".to_string(),
            super::super::PtyGeneration(1),
        ));
        let (mut reader, _writer) = reader_writer(pair.master.as_ref(), &shutdown).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut output = [0; 6];
            reader.read_exact(&mut output).unwrap();
            let _ = done_tx.send(output);
        });
        assert!(matches!(
            done_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        slave.write_all(b"output").unwrap();
        let output = done_rx.recv_timeout(Duration::from_secs(1));
        shutdown.mark_broken();
        worker.join().unwrap();
        assert_eq!(output.unwrap(), *b"output");

        // Use a fresh state for writing (the reader state was shut down above).
        let (pair, mut slave) = raw_pty();
        let shutdown = Arc::new(PtyShutdownState::new(
            "input".to_string(),
            super::super::PtyGeneration(1),
        ));
        let (_, mut writer) = reader_writer(pair.master.as_ref(), &shutdown).unwrap();
        writer.write_all(b"input").unwrap();
        let mut input = [0; 5];
        slave.read_exact(&mut input).unwrap();
        assert_eq!(input, *b"input");
        shutdown.mark_broken();
    }
}
