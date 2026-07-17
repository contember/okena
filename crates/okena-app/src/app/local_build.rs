use gpui::*;
use std::ffi::OsString;
use std::process::Command;

use super::Okena;

impl Okena {
    pub(super) fn rebuild_local(&mut self, cx: &mut Context<Self>) {
        let Some(state) = cx
            .try_global::<okena_ext_updater::GlobalLocalBuild>()
            .map(|global| global.0.clone())
        else {
            return;
        };
        let Some(checkout) = state.update(cx, |state, cx| state.try_start_build(cx)) else {
            return;
        };

        let root = checkout.root().to_path_buf();
        cx.spawn(async move |this, cx| {
            let build_result = cx
                .background_executor()
                .spawn(async move { run_release_build(&root) })
                .await;
            let _ = this.update(cx, |_this, cx| match build_result {
                Ok(()) => state.update(cx, |state, cx| {
                    state.set_status(okena_ext_updater::LocalBuildStatus::ReadyToRestart, cx);
                }),
                Err(error) => {
                    state.update(cx, |state, cx| {
                        state.set_status(
                            okena_ext_updater::LocalBuildStatus::Failed {
                                error: error.clone(),
                            },
                            cx,
                        );
                    });
                    crate::workspace::toast::ToastManager::error(
                        format!("Okena rebuild failed: {error}"),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(super) fn restart_local_build(&mut self, cx: &mut Context<Self>) {
        let Some(state) = cx
            .try_global::<okena_ext_updater::GlobalLocalBuild>()
            .map(|global| global.0.clone())
        else {
            return;
        };
        let Some(checkout) = state.update(cx, |state, cx| state.try_start_restart(cx)) else {
            return;
        };

        let Some(daemon) = okena_remote_server::local::running_daemon() else {
            state.update(cx, |state, cx| {
                state.set_status(
                    okena_ext_updater::LocalBuildStatus::Failed {
                        error: "local daemon is unavailable".to_string(),
                    },
                    cx,
                );
            });
            return;
        };
        if !daemon.ui_owned {
            state.update(cx, |state, cx| {
                state.set_daemon_ui_owned(false, cx);
                state.set_status(okena_ext_updater::LocalBuildStatus::ReadyToRestart, cx);
            });
            crate::workspace::toast::ToastManager::warning(
                "The local daemon is externally managed; restart it manually",
                cx,
            );
            return;
        }

        let release_executable = checkout.release_executable().to_path_buf();
        let daemon_host = daemon.host().to_string();
        let daemon_port = daemon.port;
        let daemon_endpoint = daemon.local_endpoint.clone();
        let app_args: Vec<OsString> = std::env::args_os().skip(1).collect();

        cx.spawn(async move |this, cx| {
            let restart_result = cx
                .background_executor()
                .spawn(async move {
                    okena_remote_server::local::restart_local_daemon(
                        &daemon_host,
                        daemon_port,
                        daemon_endpoint.as_ref(),
                    )
                })
                .await;
            if let Err(error) = restart_result {
                let _ = this.update(cx, |_this, cx| {
                    state.update(cx, |state, cx| {
                        state.set_status(
                            okena_ext_updater::LocalBuildStatus::Failed {
                                error: error.clone(),
                            },
                            cx,
                        );
                    });
                    crate::workspace::toast::ToastManager::error(
                        format!("Daemon restart failed: {error}"),
                        cx,
                    );
                });
                return;
            }

            state.update(cx, |state, cx| {
                state.set_status(okena_ext_updater::LocalBuildStatus::RestartingApp, cx);
            });
            let _ = this.update(cx, |this, cx| {
                match Command::new(&release_executable)
                    .args(&app_args)
                    .env("OKENA_ACTIVATE", "1")
                    .spawn()
                {
                    Ok(_) => {
                        this.preserve_daemon_on_quit = true;
                        this.quitting
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        log::info!("Restarting Okena from {}", release_executable.display());
                        cx.quit();
                    }
                    Err(error) => {
                        let message = format!("failed to launch rebuilt Okena: {error}");
                        state.update(cx, |state, cx| {
                            state.set_status(
                                okena_ext_updater::LocalBuildStatus::Failed {
                                    error: message.clone(),
                                },
                                cx,
                            );
                        });
                        crate::workspace::toast::ToastManager::error(message, cx);
                    }
                }
            });
        })
        .detach();
    }
}

fn run_release_build(root: &std::path::Path) -> Result<(), String> {
    let output = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("failed to start cargo: {error}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        log::info!("cargo build --release stdout:\n{stdout}");
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if output.status.success() {
            log::info!("cargo build --release stderr:\n{stderr}");
        } else {
            log::error!("cargo build --release stderr:\n{stderr}");
        }
    }

    if output.status.success() {
        Ok(())
    } else {
        Err(last_output_line(&stderr)
            .unwrap_or_else(|| format!("cargo exited with {}", output.status)))
    }
}

fn last_output_line(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::last_output_line;

    #[test]
    fn build_error_uses_last_non_empty_line_and_stays_short() {
        let long = "x".repeat(200);
        let output = format!("first\n\n{long}\n");
        assert_eq!(last_output_line(&output).unwrap().len(), 160);
    }
}
