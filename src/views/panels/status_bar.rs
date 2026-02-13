use crate::keybindings::ToggleSidebar;
use crate::theme::theme;
use crate::workspace::state::Workspace;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::h_flex;
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::Duration;
use sysinfo::System;
use time::OffsetDateTime;

/// Refresh interval for system stats
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// Cached system stats
#[derive(Clone, Default)]
struct SystemStats {
    cpu_usage: f32,
    memory_used_gb: f32,
    memory_total_gb: f32,
}

/// Global system info cache
struct SystemInfoCache {
    system: System,
    stats: SystemStats,
}

impl SystemInfoCache {
    fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();

        Self {
            system,
            stats: SystemStats::default(),
        }
    }

    fn refresh(&mut self) {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();

        // Calculate average CPU usage across all cores
        let cpu_usage = self.system.cpus().iter()
            .map(|cpu| cpu.cpu_usage())
            .sum::<f32>() / self.system.cpus().len().max(1) as f32;

        let memory_used = self.system.used_memory() as f64 / 1_073_741_824.0; // bytes to GB
        let memory_total = self.system.total_memory() as f64 / 1_073_741_824.0;

        self.stats = SystemStats {
            cpu_usage,
            memory_used_gb: memory_used as f32,
            memory_total_gb: memory_total as f32,
        };
    }

    fn stats(&self) -> SystemStats {
        self.stats.clone()
    }
}

/// Status bar component showing system info and time
pub struct StatusBar {
    workspace: Entity<Workspace>,
    cache: Arc<Mutex<SystemInfoCache>>,
    sidebar_open: bool,
}

impl StatusBar {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let cache = Arc::new(Mutex::new(SystemInfoCache::new()));

        // Initial refresh
        cache.lock().refresh();

        // Start periodic refresh
        let cache_for_task = cache.clone();
        cx.spawn(async move |this: WeakEntity<StatusBar>, cx| {
            loop {
                smol::Timer::after(REFRESH_INTERVAL).await;

                // Refresh system info
                cache_for_task.lock().refresh();

                // Notify to re-render
                let result = this.update(cx, |_this, cx| {
                    cx.notify();
                });

                if result.is_err() {
                    break; // View was dropped
                }
            }
        }).detach();

        // Re-render when workspace changes (for focused project updates)
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();

        Self { workspace, cache, sidebar_open: true }
    }

    pub fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            cx.notify();
        }
    }

    fn format_time() -> String {
        match OffsetDateTime::now_local() {
            Ok(now) => format!("{:02}:{:02}", now.hour(), now.minute()),
            Err(_) => {
                // Fallback to UTC if local time is unavailable
                let now = OffsetDateTime::now_utc();
                format!("{:02}:{:02}", now.hour(), now.minute())
            }
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let stats = self.cache.lock().stats();

        // Get current time using chrono-free approach
        let time_str = Self::format_time();

        // Format memory
        let memory_str = format!("{:.1}/{:.1} GB", stats.memory_used_gb, stats.memory_total_gb);
        let memory_percent = if stats.memory_total_gb > 0.0 {
            (stats.memory_used_gb / stats.memory_total_gb * 100.0) as u32
        } else {
            0
        };

        let cpu_color = if stats.cpu_usage > 80.0 {
            t.metric_critical
        } else if stats.cpu_usage > 50.0 {
            t.metric_warning
        } else {
            t.metric_normal
        };

        let mem_color = if memory_percent > 80 {
            t.metric_critical
        } else if memory_percent > 60 {
            t.metric_warning
        } else {
            t.metric_normal
        };

        div()
            .id("status-bar")
            .h(px(22.0))
            .px(px(12.0))
            .flex()
            .items_center()
            .justify_between()
            .bg(rgb(t.bg_header))
            .border_t_1()
            .border_color(rgb(t.border))
            .text_size(px(11.0))
            // Left side - sidebar toggle (macOS only) + system stats
            .child({
                h_flex().gap(px(16.0))
                    // On macOS, sidebar toggle lives in the status bar footer
                    .when(cfg!(target_os = "macos"), |d| {
                        d.child(
                            div()
                                .id("sidebar-toggle")
                                .cursor_pointer()
                                .px(px(4.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .hover(|s| s.bg(rgb(t.bg_hover)))
                                .text_size(px(14.0))
                                .text_color(if self.sidebar_open {
                                    rgb(t.term_blue)
                                } else {
                                    rgb(t.text_secondary)
                                })
                                .child("☰")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(ToggleSidebar), cx);
                                }),
                        )
                    })
                    // CPU
                    .child(
                        h_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_color(rgb(t.text_muted))
                                    .child("CPU")
                            )
                            .child(
                                div()
                                    .text_color(rgb(cpu_color))
                                    .child(format!("{:.0}%", stats.cpu_usage))
                            )
                    )
                    // Memory
                    .child(
                        h_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_color(rgb(t.text_muted))
                                    .child("MEM")
                            )
                            .child(
                                div()
                                    .text_color(rgb(mem_color))
                                    .child(memory_str)
                            )
                    )
            })
            // Right side - remote info + version + time
            .child({
                let mut right = h_flex()
                    .gap(px(8.0));

                // Show remote server status if active
                if let Some(remote_info) = cx.try_global::<crate::remote::GlobalRemoteInfo>() {
                    if let Some(port) = remote_info.0.port() {
                        right = right.child(
                            div()
                                .id("remote-info")
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_color(rgb(t.term_cyan))
                                        .child(format!("REMOTE :{}", port))
                                )
                                .child(
                                    div()
                                        .id("pair-btn")
                                        .cursor_pointer()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .text_color(rgb(t.term_yellow))
                                        .text_size(px(10.0))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .hover(|s| s.bg(rgb(t.bg_hover)))
                                        .child("Pair")
                                        .on_click(|_, window, cx| {
                                            window.dispatch_action(
                                                Box::new(crate::keybindings::ShowPairingDialog),
                                                cx,
                                            );
                                        })
                                )
                        );
                    }
                }

                // Show update status if available
                if let Some(update_info) = cx.try_global::<crate::updater::GlobalUpdateInfo>() {
                    let info = &update_info.0;
                    if !info.is_dismissed() {
                        match info.status() {
                            crate::updater::UpdateStatus::Ready { version, .. } => {
                                right = right.child(
                                    div()
                                        .id("update-ready")
                                        .cursor_pointer()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .bg(rgb(t.term_green))
                                        .text_color(rgb(t.bg_primary))
                                        .text_size(px(10.0))
                                        .child(format!("Update v{}", version))
                                        .on_click(move |_, window, cx| {
                                            window.dispatch_action(
                                                Box::new(crate::keybindings::InstallUpdate),
                                                cx,
                                            );
                                        })
                                );
                            }
                            crate::updater::UpdateStatus::Installing { version } => {
                                right = right.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .text_color(rgb(t.term_yellow))
                                        .text_size(px(10.0))
                                        .child(format!("Installing v{}...", version))
                                );
                            }
                            crate::updater::UpdateStatus::ReadyToRestart { version } => {
                                right = right.child(
                                    div()
                                        .id("update-restart")
                                        .cursor_pointer()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .rounded(px(3.0))
                                        .bg(rgb(t.term_green))
                                        .text_color(rgb(t.bg_primary))
                                        .text_size(px(10.0))
                                        .child(format!("Restart to v{}", version))
                                        .on_click(move |_, _, cx| {
                                            crate::updater::installer::restart_app(cx);
                                        })
                                );
                            }
                            crate::updater::UpdateStatus::Downloading { version, progress } => {
                                right = right.child(
                                    h_flex()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_color(rgb(t.term_yellow))
                                                .text_size(px(10.0))
                                                .child(format!("Downloading v{}... {}%", version, progress))
                                        )
                                );
                            }
                            crate::updater::UpdateStatus::Checking => {
                                right = right.child(
                                    div()
                                        .px(px(6.0))
                                        .py(px(1.0))
                                        .text_color(rgb(t.text_muted))
                                        .text_size(px(10.0))
                                        .child("Checking for updates...")
                                );
                            }
                            crate::updater::UpdateStatus::Failed { ref error } => {
                                let info_dismiss = info.clone();
                                right = right.child(
                                    div()
                                        .id("update-failed")
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_color(rgb(t.term_red))
                                                .text_size(px(10.0))
                                                .child(format!("Update failed: {}", error))
                                        )
                                        .child(
                                            div()
                                                .id("update-failed-dismiss")
                                                .cursor_pointer()
                                                .text_color(rgb(t.text_muted))
                                                .text_size(px(10.0))
                                                .child("x")
                                                .on_click(move |_, _, _cx| {
                                                    info_dismiss.dismiss();
                                                })
                                        )
                                );
                            }
                            crate::updater::UpdateStatus::BrewUpdate { version } => {
                                let info_dismiss = info.clone();
                                right = right.child(
                                    div()
                                        .id("update-brew")
                                        .flex()
                                        .items_center()
                                        .gap(px(4.0))
                                        .child(
                                            div()
                                                .text_color(rgb(t.text_muted))
                                                .text_size(px(10.0))
                                                .child(format!("v{} — brew upgrade okena", version))
                                        )
                                        .child(
                                            div()
                                                .id("update-dismiss")
                                                .cursor_pointer()
                                                .text_color(rgb(t.text_muted))
                                                .text_size(px(10.0))
                                                .child("x")
                                                .on_click(move |_, _, _cx| {
                                                    info_dismiss.dismiss();
                                                })
                                        )
                                );
                            }
                            _ => {}
                        }
                    }
                }

                // Focused project indicator
                let focused_project = {
                    let ws = self.workspace.read(cx);
                    ws.focused_project_id()
                        .and_then(|id| ws.project(id))
                        .map(|p| p.name.clone())
                };

                if let Some(name) = focused_project {
                    let workspace = self.workspace.clone();
                    right = right.child(
                        h_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_muted))
                                    .child("Focused:"),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(rgb(t.border_focused))
                                    .text_size(px(11.0))
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .cursor_pointer()
                                    .px(px(4.0))
                                    .text_size(px(10.0))
                                    .text_color(rgb(t.text_muted))
                                    .hover(|s| s.text_color(rgb(t.text_primary)))
                                    .child("✕")
                                    .id("clear-focus-btn")
                                    .on_click(move |_, _window, cx| {
                                        workspace.update(cx, |ws, cx| {
                                            ws.set_focused_project(None, cx);
                                        });
                                    }),
                            )
                    );
                }

                right
                    .when(cfg!(not(target_os = "macos")), |el| {
                        el.child(
                            div()
                                .text_color(rgb(t.text_muted))
                                .child(format!("v{}", env!("CARGO_PKG_VERSION")))
                        )
                    })
                    .child(
                        div()
                            .text_color(rgb(t.text_secondary))
                            .child(time_str)
                    )
            })
    }
}
