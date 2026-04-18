use std::sync::OnceLock;

/// Application version used in XTVERSION responses. Injected once at
/// startup from the main binary (which knows its own `CARGO_PKG_VERSION`);
/// defaults to `"0.0.0"` so unit tests and library-only consumers still
/// get a parseable response.
static APP_VERSION: OnceLock<String> = OnceLock::new();

/// Register the application version that will be reported to terminal
/// applications via XTVERSION (`DCS > | okena(<version>) ST`). Safe to
/// call multiple times — the first value wins.
pub fn set_app_version(version: impl Into<String>) {
    let _ = APP_VERSION.set(version.into());
}

pub(super) fn app_version() -> &'static str {
    APP_VERSION.get().map(String::as_str).unwrap_or("0.0.0")
}
