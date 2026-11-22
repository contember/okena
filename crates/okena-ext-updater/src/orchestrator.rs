//! Progress notifications for a user-initiated check in GPUI.
use crate::status::UpdateInfo;
use gpui::AsyncApp;

pub async fn run_manual_check<F>(info: UpdateInfo, token: u64, cx: &mut AsyncApp, notify: F)
where
    F: Fn(&mut AsyncApp),
{
    let mut check = std::pin::pin!(crate::manager::run_check(info, token, true));
    loop {
        let done = std::future::poll_fn(|task| {
            std::task::Poll::Ready(check.as_mut().poll(task).is_ready())
        })
        .await;
        notify(cx);
        if done {
            break;
        }
        smol::Timer::after(std::time::Duration::from_millis(250)).await;
    }
}
