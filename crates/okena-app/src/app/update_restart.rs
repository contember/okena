use gpui::*;
use std::sync::atomic::Ordering;

use super::Okena;

impl Okena {
    /// The local daemon is restarting into an installed update: wait for its
    /// replacement, then relaunch this desktop detached into the new binary.
    pub(super) fn relaunch_for_update(
        &mut self,
        request: okena_core::api::ApiAppRestartRequest,
        cx: &mut Context<Self>,
    ) {
        if self.relaunching_for_update || self.quitting.load(Ordering::SeqCst) {
            return;
        }
        self.relaunching_for_update = true;
        log::info!(
            "Daemon pid {} is restarting into v{}; relaunching once it is replaced",
            request.daemon_pid,
            request.version
        );
        let daemon_pid = request.daemon_pid;
        cx.spawn(async move |this, cx| {
            let replaced = smol::unblock(move || {
                okena_ext_updater::daemon_client::wait_for_replacement_daemon(daemon_pid)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                // The new desktop attaches to the replacement daemon, so this
                // one must not hand it a shutdown on its way out.
                this.preserve_daemon_on_quit = true;
                if let Err(error) = replaced.and_then(|()| okena_ext_updater::restart_app(cx)) {
                    this.preserve_daemon_on_quit = false;
                    this.relaunching_for_update = false;
                    crate::workspace::toast::ToastManager::error(
                        format!("Relaunch into the update failed: {error:#}"),
                        cx,
                    );
                }
            });
        })
        .detach();
    }
}
