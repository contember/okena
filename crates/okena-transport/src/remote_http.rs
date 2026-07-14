//! Connection-aware HTTP client construction for Okena's remote protocol.

use crate::RemoteConnectionConfig;

/// Build an async HTTP client and URL using the connection's complete
/// transport policy: local socket, plain TCP, or pinned TLS.
#[cfg(feature = "client")]
pub fn async_client_and_url(
    config: &RemoteConnectionConfig,
    path: &str,
) -> Result<(reqwest::Client, String), String> {
    #[cfg(unix)]
    if let Some(crate::LocalEndpoint::UnixSocket { path: socket_path }) = &config.local_endpoint {
        let client = reqwest::Client::builder()
            .unix_socket(socket_path.as_str())
            .build()
            .map_err(|error| format!("Cannot initialise Unix socket HTTP client: {error}"))?;
        return Ok((client, config.http_url(path)));
    }

    let client = crate::tls::build_reqwest_client(
        config.tls,
        config.pinned_cert_sha256.clone(),
        crate::tls::new_observed(),
    )?;
    Ok((client, config.http_url(path)))
}

/// Build a blocking HTTP client and URL with the same transport policy as the
/// async connection manager.
#[cfg(feature = "blocking-http")]
pub fn blocking_client_and_url(
    config: &RemoteConnectionConfig,
    path: &str,
    timeout: std::time::Duration,
) -> Result<(reqwest::blocking::Client, String), String> {
    #[cfg(unix)]
    if let Some(crate::LocalEndpoint::UnixSocket { path: socket_path }) = &config.local_endpoint {
        let client = reqwest::blocking::Client::builder()
            .unix_socket(socket_path.as_str())
            .timeout(timeout)
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()
            .map_err(|e| format!("Cannot initialise Unix socket HTTP client: {e}"))?;
        return Ok((client, config.http_url(path)));
    }

    let client = crate::tls::build_blocking_reqwest_client(
        config.tls,
        config.pinned_cert_sha256.clone(),
        crate::tls::new_observed(),
        timeout,
    )?;
    Ok((client, config.http_url(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LocalEndpoint;

    fn config(tls: bool, local_endpoint: Option<LocalEndpoint>) -> RemoteConnectionConfig {
        RemoteConnectionConfig {
            id: "test".to_string(),
            name: "Test".to_string(),
            host: "remote.example".to_string(),
            port: 19100,
            saved_token: Some("secret".to_string()),
            token_obtained_at: None,
            tls,
            pinned_cert_sha256: Some("00".repeat(32)),
            local_endpoint,
        }
    }

    #[cfg(feature = "client")]
    #[test]
    fn async_remote_url_uses_tls_from_connection_config() {
        let (_, url) = async_client_and_url(&config(true, None), "/v1/actions").unwrap();
        assert_eq!(url, "https://remote.example:19100/v1/actions");
    }

    #[cfg(feature = "blocking-http")]
    #[test]
    fn blocking_remote_url_uses_tls_from_connection_config() {
        let (_, url) = blocking_client_and_url(
            &config(true, None),
            "/v1/actions",
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(url, "https://remote.example:19100/v1/actions");
    }

    #[cfg(all(unix, feature = "blocking-http"))]
    #[test]
    fn blocking_local_url_uses_unix_socket_origin() {
        let (_, url) = blocking_client_and_url(
            &config(
                false,
                Some(LocalEndpoint::UnixSocket {
                    path: "/tmp/okena-test.sock".to_string(),
                }),
            ),
            "/v1/actions",
            std::time::Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(url, "http://okena.local/v1/actions");
    }
}
