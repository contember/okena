use crate::status::{UpdateInfo, UpdateStatus, UpdateStatusWidget};
use gpui::*;

/// Mirror daemon-owned update state into the status widget.
pub fn start_update_status_poll(update_info: UpdateInfo, cx: &mut Context<UpdateStatusWidget>) {
    cx.spawn(async move |this: WeakEntity<UpdateStatusWidget>, cx| {
        loop {
            if let Ok(snapshot) = smol::unblock(crate::daemon_client::fetch_status).await {
                update_info.apply_snapshot(snapshot);
                let _ = this.update(cx, |_, cx| cx.notify());
            }

            let delay = match update_info.status() {
                UpdateStatus::Checking
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Installing { .. } => std::time::Duration::from_millis(500),
                _ => std::time::Duration::from_secs(5),
            };
            smol::Timer::after(delay).await;

            if this.update(cx, |_, _| {}).is_err() {
                return;
            }
        }
    })
    .detach();
}
