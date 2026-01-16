use crate::theme::theme;
use gpui::*;
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
    cache: Arc<Mutex<SystemInfoCache>>,
}

impl StatusBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
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

        Self { cache }
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

        // CPU color based on usage
        let cpu_color = if stats.cpu_usage > 80.0 {
            t.term_red
        } else if stats.cpu_usage > 50.0 {
            t.term_yellow
        } else {
            t.term_green
        };

        // Memory color based on usage
        let mem_color = if memory_percent > 80 {
            t.term_red
        } else if memory_percent > 60 {
            t.term_yellow
        } else {
            t.term_green
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
            // Left side - system stats
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(16.0))
                    // CPU
                    .child(
                        div()
                            .flex()
                            .items_center()
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
                        div()
                            .flex()
                            .items_center()
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
            )
            // Right side - time
            .child(
                div()
                    .text_color(rgb(t.text_secondary))
                    .child(time_str)
            )
    }
}
