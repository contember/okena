//! Platform logger for the `log` facade.
//!
//! Without one every `log::info!`/`warn!` in the transport vanishes on device,
//! which leaves connection problems undiagnosable. Android logs go to logcat
//! (tag `okena`), iOS logs to the unified log (subsystem
//! `com.contember.okena`). Our crates log at Info, everything else (rustls,
//! hyper, tungstenite, …) only at Warn. Nothing logged here carries tokens.

/// Forwards records from our crates at Info and from dependencies at Warn.
#[cfg(any(target_os = "android", target_os = "ios"))]
struct CrateFilter<L> {
    inner: L,
}

#[cfg(any(target_os = "android", target_os = "ios"))]
impl<L: log::Log> log::Log for CrateFilter<L> {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        let max = if metadata.target().starts_with("okena") {
            log::Level::Info
        } else {
            log::Level::Warn
        };
        metadata.level() <= max && self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.inner.log(record);
        }
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

/// Install the logger once; later calls (and a host that already installed
/// one) are no-ops.
pub(crate) fn init() {
    #[cfg(target_os = "android")]
    {
        let inner = android_logger::AndroidLogger::new(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Info)
                .with_tag("okena"),
        );
        install(CrateFilter { inner });
    }
    #[cfg(target_os = "ios")]
    {
        let inner = oslog::OsLogger::new("com.contember.okena");
        install(CrateFilter { inner });
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn install(logger: impl log::Log + 'static) {
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
        log::info!("okena-mobile-ffi logger installed");
    }
}
