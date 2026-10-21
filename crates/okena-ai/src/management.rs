//! Where the management routes listen: a Unix socket inside the owner-only proxy
//! directory, or on Windows a named pipe only the current user can open. Never
//! a TCP port: another local user could bind the port while the proxy is down
//! and receive the management key.

use std::io;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

pub(crate) struct Address {
    #[cfg(unix)]
    socket: PathBuf,
    #[cfg(windows)]
    pipe: String,
}

impl Address {
    #[cfg(unix)]
    pub(crate) fn new(profile_root: &Path, _management_key: &str) -> Self {
        Self {
            socket: okena_core::ai_proxy::proxy_dir(profile_root).join("management.sock"),
        }
    }

    /// Pipe names are machine-wide. This one is derived from the management key,
    /// so a user who cannot read the key cannot take the name first.
    #[cfg(windows)]
    pub(crate) fn new(_profile_root: &Path, management_key: &str) -> Self {
        use sha2::Digest;
        let digest = sha2::Sha256::digest(management_key.as_bytes());
        let id: String = digest[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            pipe: format!(r"\\.\pipe\okena-ai-{id}"),
        }
    }

    /// HTTP client that sends every request to this address.
    pub(crate) fn client(&self) -> anyhow::Result<reqwest::Client> {
        let builder = reqwest::Client::builder().no_proxy();
        #[cfg(unix)]
        let builder = builder.unix_socket(self.socket.as_path());
        #[cfg(windows)]
        let builder = builder.windows_named_pipe(self.pipe.as_str());
        Ok(builder.build()?)
    }

    /// Listen on this address. The caller holds the instance lock.
    #[cfg(unix)]
    pub(crate) fn bind(&self) -> io::Result<tokio::net::UnixListener> {
        // Under the instance lock, a socket file that is already there is stale.
        match std::fs::remove_file(&self.socket) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        tokio::net::UnixListener::bind(&self.socket)
    }

    #[cfg(windows)]
    pub(crate) fn bind(&self) -> io::Result<pipe::PipeListener> {
        pipe::PipeListener::bind(self.pipe.clone())
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(unix)]
        return write!(formatter, "{}", self.socket.display());
        #[cfg(windows)]
        return write!(formatter, "{}", self.pipe);
    }
}

#[cfg(windows)]
mod pipe {
    use okena_core::owner_only::OwnerOnly;
    use std::io;
    use std::time::Duration;
    use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

    /// Accepts management connections on a named pipe. Every instance of the
    /// pipe carries a current-user-only access list.
    pub(crate) struct PipeListener {
        name: String,
        next: NamedPipeServer,
    }

    impl PipeListener {
        pub(crate) fn bind(name: String) -> io::Result<Self> {
            // The first instance fails when another process already owns the name.
            let next = create(&name, true)?;
            Ok(Self { name, next })
        }

        async fn replace_next(&mut self) -> NamedPipeServer {
            loop {
                match create(&self.name, false) {
                    Ok(next) => return std::mem::replace(&mut self.next, next),
                    Err(error) => {
                        log::warn!("[ai-proxy] management pipe instance failed: {error}");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        }
    }

    fn create(name: &str, first: bool) -> io::Result<NamedPipeServer> {
        let security = OwnerOnly::new(false)?;
        let mut options = ServerOptions::new();
        options
            .first_pipe_instance(first)
            .reject_remote_clients(true);
        security.with_attributes(|attributes| {
            // SAFETY: `attributes` points at a SECURITY_ATTRIBUTES that lives
            // for the duration of this call.
            unsafe { options.create_with_security_attributes_raw(name, attributes) }
        })
    }

    impl axum::serve::Listener for PipeListener {
        type Io = NamedPipeServer;
        type Addr = String;

        async fn accept(&mut self) -> (Self::Io, Self::Addr) {
            loop {
                match self.next.connect().await {
                    Ok(()) => return (self.replace_next().await, self.name.clone()),
                    // A client that left before the connection was set up spoils
                    // this instance; serve the next one.
                    Err(error) => {
                        log::warn!("[ai-proxy] management pipe connect failed: {error}");
                        drop(self.replace_next().await);
                    }
                }
            }
        }

        fn local_addr(&self) -> io::Result<Self::Addr> {
            Ok(self.name.clone())
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stale_socket_is_replaced_and_the_socket_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        okena_core::ai_proxy::create_private_dir(&okena_core::ai_proxy::proxy_dir(dir.path()))
            .unwrap();
        let address = Address::new(dir.path(), "key");
        drop(address.bind().unwrap());
        // The socket file outlives the listener, like after a crash.
        assert!(address.socket.exists());
        let listener = address.bind().unwrap();
        let parent = address.socket.parent().unwrap();
        let mode = std::fs::metadata(parent).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let app = axum::Router::new().fallback(|| async { "over the socket" });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let reply = address
            .client()
            .unwrap()
            .get("http://okena-ai-proxy/anything")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(reply, "over the socket");
    }
}
