//! Where the management routes listen: a Unix socket inside the owner-only proxy
//! directory (or, when that path is too long for a socket, in an owner-only
//! directory under the user's runtime directory), or on Windows a named pipe
//! only the current user can open. Never a TCP port: another local user could
//! bind the port while the proxy is down and receive the management key.

use std::io;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

/// Room for a socket path in `sockaddr_un`, including its NUL terminator.
#[cfg(target_os = "linux")]
const SOCKET_PATH_CAPACITY: usize = 108;
#[cfg(all(unix, not(target_os = "linux")))]
const SOCKET_PATH_CAPACITY: usize = 104;

#[cfg(unix)]
const SOCKET_NAME: &str = "management.sock";

pub(crate) struct Address {
    #[cfg(unix)]
    socket: PathBuf,
    /// The fallback directory holding `socket`, checked before every use.
    #[cfg(unix)]
    fallback_dir: Option<PathBuf>,
    #[cfg(windows)]
    pipe: String,
}

impl Address {
    /// The proxy and the daemon's client both resolve the socket here.
    #[cfg(unix)]
    pub(crate) fn new(profile_root: &Path, _management_key: &str) -> Self {
        let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
            .or_else(|| std::env::var_os("TMPDIR"))
            .map_or_else(|| PathBuf::from("/tmp"), PathBuf::from);
        Self::in_profile_or(profile_root, &runtime_dir)
    }

    #[cfg(unix)]
    fn in_profile_or(profile_root: &Path, runtime_dir: &Path) -> Self {
        let proxy_dir = okena_core::ai_proxy::proxy_dir(profile_root);
        let socket = proxy_dir.join(SOCKET_NAME);
        if socket.as_os_str().len() < SOCKET_PATH_CAPACITY {
            return Self {
                socket,
                fallback_dir: None,
            };
        }
        use sha2::Digest;
        let digest = sha2::Sha256::digest(proxy_dir.as_os_str().as_encoded_bytes());
        let id: String = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let fallback_dir = runtime_dir.join(format!("okena-ai-{id}"));
        Self {
            socket: fallback_dir.join(SOCKET_NAME),
            fallback_dir: Some(fallback_dir),
        }
    }

    /// Create the fallback directory, or check that the one found is ours and
    /// private: a socket in a directory another user controls could be theirs.
    #[cfg(unix)]
    fn ensure_private(&self) -> io::Result<()> {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        let Some(dir) = &self.fallback_dir else {
            return Ok(());
        };
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Err(error) if error.kind() != io::ErrorKind::AlreadyExists => return Err(error),
            _ => {}
        }
        let metadata = std::fs::symlink_metadata(dir)?;
        // SAFETY: `geteuid(2)` takes no arguments and never fails.
        let euid = unsafe { libc::geteuid() };
        if !metadata.is_dir()
            || metadata.uid() != euid
            || metadata.permissions().mode() & 0o777 != 0o700
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "{} is not a directory that only this user can access",
                    dir.display()
                ),
            ));
        }
        Ok(())
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
        #[cfg(unix)]
        self.ensure_private()?;
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
        self.ensure_private()?;
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
        assert_eq!(address.fallback_dir, None);
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

    #[tokio::test]
    async fn a_profile_path_too_long_for_a_socket_uses_a_private_runtime_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let profile_root = dir.path().join("p".repeat(120));
        okena_core::ai_proxy::create_private_dir(&okena_core::ai_proxy::proxy_dir(&profile_root))
            .unwrap();
        let runtime_dir = tempfile::tempdir().unwrap();
        let address = Address::in_profile_or(&profile_root, runtime_dir.path());
        let fallback = address.fallback_dir.clone().unwrap();
        assert!(fallback.starts_with(runtime_dir.path()));
        assert!(address.socket.as_os_str().len() < SOCKET_PATH_CAPACITY);
        // The client may run first and creates the directory the proxy reuses.
        let client = address.client().unwrap();
        let listener = Address::in_profile_or(&profile_root, runtime_dir.path())
            .bind()
            .unwrap();
        let mode = std::fs::metadata(&fallback).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);
        let app = axum::Router::new().fallback(|| async { "long path" });
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let reply = client
            .get("http://okena-ai-proxy/anything")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(reply, "long path");

        // A directory others can enter is refused, by the client and the proxy.
        std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(address.client().is_err());
        assert!(address.bind().is_err());
        // A symlink in its place is refused too.
        std::fs::set_permissions(&fallback, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::rename(&fallback, runtime_dir.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(runtime_dir.path().join("elsewhere"), &fallback).unwrap();
        assert!(address.client().is_err());
    }
}
