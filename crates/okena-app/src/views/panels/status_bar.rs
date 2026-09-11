use crate::keybindings::ToggleSidebar;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::metrics::{SparklineStyle, StatusBarStyle, metric_bar, sparkline};
use crate::ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xl};
use crate::workspace::state::Workspace;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{ApiLayoutNode, ApiSystemStats};
use okena_extensions::{ExtensionInstance, ExtensionRegistry};
use okena_transport::client::{ConnectionStatus, LOCAL_DAEMON_CONNECTION_ID};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::System;
use time::OffsetDateTime;

/// Refresh interval for system stats
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How many samples of CPU/MEM history the graphs keep. At [`REFRESH_INTERVAL`]
/// this is a little under a minute — enough to see a spike come and go.
const HISTORY_LEN: usize = 24;

/// Cached system stats
#[derive(Clone, Default)]
struct SystemStats {
    cpu_usage: f32,
    memory_used_gb: f32,
    memory_total_gb: f32,
    /// Recent CPU load, oldest first, each 0.0..=1.0.
    cpu_history: Vec<f32>,
    /// Recent memory pressure, oldest first, each 0.0..=1.0.
    memory_history: Vec<f32>,
}

/// Everything the status bar needs to draw one system metric (CPU or MEM).
struct SystemMetric {
    id: &'static str,
    label: &'static str,
    value_text: String,
    tooltip: String,
    /// Current value as 0.0..=1.0, for the bar.
    fraction: f32,
    /// Recent values as 0.0..=1.0, oldest first, for the graph.
    history: Vec<f32>,
    color: u32,
}

#[derive(Clone)]
struct RemoteStatusSnapshot {
    id: String,
    name: String,
    endpoint: String,
    status: ConnectionStatus,
    tls: bool,
    project_count: usize,
    window_count: usize,
    terminal_count: usize,
    has_state: bool,
    system_stats: Option<ApiSystemStats>,
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
        let cpu_usage = self
            .system
            .cpus()
            .iter()
            .map(|cpu| cpu.cpu_usage())
            .sum::<f32>()
            / self.system.cpus().len().max(1) as f32;

        let memory_used = self.system.used_memory() as f64 / 1_073_741_824.0; // bytes to GB
        let memory_total = self.system.total_memory() as f64 / 1_073_741_824.0;
        let memory_fraction = if memory_total > 0.0 {
            (memory_used / memory_total) as f32
        } else {
            0.0
        };

        let mut cpu_history = std::mem::take(&mut self.stats.cpu_history);
        let mut memory_history = std::mem::take(&mut self.stats.memory_history);
        push_sample(&mut cpu_history, cpu_usage / 100.0);
        push_sample(&mut memory_history, memory_fraction);

        self.stats = SystemStats {
            cpu_usage,
            memory_used_gb: memory_used as f32,
            memory_total_gb: memory_total as f32,
            cpu_history,
            memory_history,
        };
    }

    fn stats(&self) -> SystemStats {
        self.stats.clone()
    }
}

/// Append a sample to a history buffer, dropping the oldest past [`HISTORY_LEN`].
fn push_sample(history: &mut Vec<f32>, value: f32) {
    if history.len() >= HISTORY_LEN {
        history.remove(0);
    }
    history.push(value.clamp(0.0, 1.0));
}

/// Status bar component showing system info and time
pub struct StatusBar {
    window_id: crate::workspace::state::WindowId,
    workspace: Entity<Workspace>,
    focus_manager: Entity<crate::workspace::focus::FocusManager>,
    cache: Arc<Mutex<SystemInfoCache>>,
    /// Activate functions cloned from registry (keyed by extension ID).
    activate_fns: Vec<(String, okena_extensions::ActivateFn)>,
    /// Active extension instances. Dropping an instance deactivates the extension
    /// (cancels background tasks, releases views).
    active_extensions: HashMap<String, ExtensionInstance>,
    sidebar_open: bool,
    remote_manager: Option<Entity<RemoteConnectionManager>>,
    remote_status_bounds: Bounds<Pixels>,
    remote_popover_visible: bool,
}

impl StatusBar {
    pub fn new(
        window_id: crate::workspace::state::WindowId,
        workspace: Entity<Workspace>,
        focus_manager: Entity<crate::workspace::focus::FocusManager>,
        cx: &mut Context<Self>,
    ) -> Self {
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
        })
        .detach();

        // Clone activate functions from the global registry.
        let activate_fns: Vec<_> = cx
            .try_global::<ExtensionRegistry>()
            .map(|registry| {
                registry
                    .extensions()
                    .iter()
                    .map(|ext| (ext.manifest.id.to_string(), ext.activate.clone()))
                    .collect()
            })
            .unwrap_or_default();

        // Activate initially enabled extensions
        let enabled = settings_entity(cx)
            .read(cx)
            .settings
            .enabled_extensions
            .clone();
        let active_extensions = Self::activate_extensions(&activate_fns, &enabled, cx);

        // Observe settings to sync extensions when enabled_extensions changes
        let settings = settings_entity(cx);
        cx.observe(&settings, |this, entity, cx| {
            let enabled = entity.read(cx).settings.enabled_extensions.clone();
            this.sync_extensions(&enabled, cx);
        })
        .detach();

        // Re-render when workspace changes (for focused project updates)
        cx.observe(&workspace, |_, _, cx| cx.notify()).detach();
        // Also re-render when focus state changes (focus_manager moved off Workspace in slice 03)
        cx.observe(&focus_manager, |_, _, cx| cx.notify()).detach();
        // And when a harness view opens or closes, which hides the focused
        // project indicator.
        if let Some(harness) = okena_workspace::harness_state::harness_state_entity(cx) {
            cx.observe(&harness, |_, _, cx| cx.notify()).detach();
        }

        Self {
            window_id,
            workspace,
            focus_manager,
            cache,
            activate_fns,
            active_extensions,
            sidebar_open: true,
            remote_manager: None,
            remote_status_bounds: Bounds::default(),
            remote_popover_visible: false,
        }
    }

    /// Activate extensions that are in the enabled set.
    fn activate_extensions(
        activate_fns: &[(String, okena_extensions::ActivateFn)],
        enabled: &HashSet<String>,
        cx: &mut App,
    ) -> HashMap<String, ExtensionInstance> {
        activate_fns
            .iter()
            .filter(|(id, _)| enabled.contains(id.as_str()))
            .map(|(id, activate)| (id.clone(), activate(cx)))
            .collect()
    }

    /// Sync active extensions with the current enabled set.
    /// Activates newly enabled extensions, deactivates disabled ones
    /// (dropping the instance cancels background tasks and releases views).
    fn sync_extensions(&mut self, enabled: &HashSet<String>, cx: &mut Context<Self>) {
        // Deactivate disabled (drop instances → cancel tasks)
        self.active_extensions
            .retain(|id, _| enabled.contains(id.as_str()));

        // Activate newly enabled
        for (id, activate) in &self.activate_fns {
            if enabled.contains(id.as_str()) && !self.active_extensions.contains_key(id) {
                self.active_extensions.insert(id.clone(), activate(cx));
            }
        }

        cx.notify();
    }

    pub fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            cx.notify();
        }
    }

    pub fn set_remote_manager(
        &mut self,
        manager: Entity<RemoteConnectionManager>,
        cx: &mut Context<Self>,
    ) {
        self.remote_manager = Some(manager);
        cx.notify();
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

    fn remote_snapshots(&self, cx: &App) -> Vec<RemoteStatusSnapshot> {
        let Some(manager) = &self.remote_manager else {
            return Vec::new();
        };

        manager
            .read(cx)
            .connections_with_system_stats()
            .into_iter()
            .filter(|(config, _, _, _)| config.id != LOCAL_DAEMON_CONNECTION_ID)
            .map(|(config, status, state, system_stats)| {
                let (project_count, window_count, terminal_count) = match state {
                    Some(state) => {
                        let terminals = state
                            .projects
                            .iter()
                            .map(|project| Self::layout_terminal_count(project.layout.as_ref()))
                            .sum();
                        (state.projects.len(), state.windows.len(), terminals)
                    }
                    None => (0, 0, 0),
                };

                RemoteStatusSnapshot {
                    id: config.id.clone(),
                    name: config.name.clone(),
                    endpoint: config.display_endpoint(),
                    status: status.clone(),
                    tls: config.tls,
                    project_count,
                    window_count,
                    terminal_count,
                    has_state: state.is_some(),
                    system_stats: system_stats.cloned(),
                }
            })
            .collect()
    }

    fn layout_terminal_count(node: Option<&ApiLayoutNode>) -> usize {
        match node {
            Some(ApiLayoutNode::Terminal {
                terminal_id: Some(_),
                ..
            }) => 1,
            Some(ApiLayoutNode::Terminal { .. }) => 0,
            Some(ApiLayoutNode::Split { children, .. } | ApiLayoutNode::Tabs { children, .. }) => {
                children
                    .iter()
                    .map(|child| Self::layout_terminal_count(Some(child)))
                    .sum()
            }
            None => 0,
        }
    }

    fn count_label(count: usize, singular: &str) -> String {
        if count == 1 {
            format!("1 {singular}")
        } else {
            format!("{count} {singular}s")
        }
    }

    fn status_label(status: &ConnectionStatus) -> String {
        match status {
            ConnectionStatus::Disconnected => "Disconnected".to_string(),
            ConnectionStatus::Connecting => "Connecting".to_string(),
            ConnectionStatus::Pairing => "Pairing".to_string(),
            ConnectionStatus::Connected => "Connected".to_string(),
            ConnectionStatus::Reconnecting { attempt } => format!("Reconnecting #{attempt}"),
            ConnectionStatus::Error(message) => {
                if message.is_empty() {
                    "Error".to_string()
                } else {
                    format!("Error: {message}")
                }
            }
        }
    }

    fn status_color(status: &ConnectionStatus, t: &okena_core::theme::ThemeColors) -> u32 {
        match status {
            ConnectionStatus::Connected => t.term_green,
            ConnectionStatus::Connecting
            | ConnectionStatus::Pairing
            | ConnectionStatus::Reconnecting { .. } => t.term_yellow,
            ConnectionStatus::Disconnected => t.text_muted,
            ConnectionStatus::Error(_) => t.term_red,
        }
    }

    fn cpu_metric_color(cpu_usage: f32, t: &okena_core::theme::ThemeColors) -> u32 {
        if cpu_usage > 80.0 {
            t.metric_critical
        } else if cpu_usage > 50.0 {
            t.metric_warning
        } else {
            t.metric_normal
        }
    }

    fn memory_metric_color(memory_percent: u64, t: &okena_core::theme::ThemeColors) -> u32 {
        if memory_percent > 80 {
            t.metric_critical
        } else if memory_percent > 60 {
            t.metric_warning
        } else {
            t.metric_normal
        }
    }

    fn memory_percent_from_bytes(stats: &ApiSystemStats) -> u64 {
        stats
            .memory_used_bytes
            .saturating_mul(100)
            .checked_div(stats.memory_total_bytes)
            .unwrap_or(0)
    }

    fn format_gib_tenths(bytes: u64) -> String {
        const GIB: u64 = 1_073_741_824;
        let tenths = bytes.saturating_mul(10).saturating_add(GIB / 2) / GIB;
        format!("{}.{}", tenths / 10, tenths % 10)
    }

    fn format_memory_bytes(stats: &ApiSystemStats) -> String {
        format!(
            "{}/{} GB",
            Self::format_gib_tenths(stats.memory_used_bytes),
            Self::format_gib_tenths(stats.memory_total_bytes),
        )
    }

    fn aggregate_remote_color(
        snapshots: &[RemoteStatusSnapshot],
        t: &okena_core::theme::ThemeColors,
    ) -> u32 {
        if snapshots
            .iter()
            .any(|snap| matches!(snap.status, ConnectionStatus::Error(_)))
        {
            return t.term_red;
        }
        if snapshots.iter().any(|snap| {
            matches!(
                snap.status,
                ConnectionStatus::Connecting
                    | ConnectionStatus::Pairing
                    | ConnectionStatus::Reconnecting { .. }
            )
        }) {
            return t.term_yellow;
        }
        if snapshots
            .iter()
            .all(|snap| matches!(snap.status, ConnectionStatus::Connected))
        {
            return t.term_green;
        }
        t.text_muted
    }

    /// One system metric (CPU or MEM) as the status bar draws it.
    fn render_system_metric(
        metric: SystemMetric,
        style: StatusBarStyle,
        graph: bool,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let SystemMetric {
            id,
            label,
            value_text,
            tooltip,
            fraction,
            history,
            color,
        } = metric;

        let label_el = div().text_color(rgb(t.text_muted)).child(label);

        let body = if style.is_minimal() {
            // No number — the graph (or bar) carries the value, the tooltip
            // carries the exact figure.
            h_flex()
                .gap(px(4.0))
                .text_size(ui_text_sm(cx))
                .child(label_el)
                .child(if graph {
                    sparkline(&history, SparklineStyle::tall(), color, t).into_any_element()
                } else {
                    div()
                        .w(px(34.0))
                        .child(metric_bar(fraction, color, t))
                        .into_any_element()
                })
                .into_any_element()
        } else {
            v_flex()
                .gap(px(1.0))
                .child(
                    h_flex()
                        .gap(px(3.0))
                        .text_size(ui_text_sm(cx))
                        .child(label_el)
                        .child(div().text_color(rgb(color)).child(value_text)),
                )
                .child(if graph {
                    sparkline(&history, SparklineStyle::compact(), color, t).into_any_element()
                } else {
                    metric_bar(fraction, color, t).into_any_element()
                })
                .into_any_element()
        };

        div()
            .id(id)
            .child(body)
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .into_any_element()
    }

    fn render_remote_status_popover(
        &self,
        snapshots: &[RemoteStatusSnapshot],
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.remote_popover_visible || snapshots.is_empty() {
            return div().size_0().into_any_element();
        }

        let bounds = self.remote_status_bounds;
        let position = point(
            bounds.origin.x + bounds.size.width,
            bounds.origin.y - px(6.0),
        );

        let mut rows = Vec::new();
        for snapshot in snapshots {
            let status_label = Self::status_label(&snapshot.status);
            let detail = if snapshot.has_state {
                format!(
                    "{} / {} / {}",
                    Self::count_label(snapshot.project_count, "project"),
                    Self::count_label(snapshot.terminal_count, "terminal"),
                    Self::count_label(snapshot.window_count, "window"),
                )
            } else {
                "Waiting for state".to_string()
            };
            let security = if snapshot.tls { "TLS" } else { "no TLS" };
            let status_color = Self::status_color(&snapshot.status, t);
            let system_stats = snapshot.system_stats.clone();

            rows.push(
                div()
                    .id(ElementId::Name(
                        format!("remote-status-row-{}", snapshot.id).into(),
                    ))
                    .py(px(6.0))
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div()
                            .mt(px(5.0))
                            .w(px(7.0))
                            .h(px(7.0))
                            .rounded_full()
                            .bg(rgb(status_color))
                            .flex_shrink_0(),
                    )
                    .child(
                        v_flex()
                            .gap(px(2.0))
                            .min_w_0()
                            .flex_1()
                            .child(
                                h_flex()
                                    .gap(px(6.0))
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .text_size(ui_text_ms(cx))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(t.text_primary))
                                            .child(snapshot.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .text_size(ui_text_sm(cx))
                                            .text_color(rgb(if snapshot.tls {
                                                t.text_muted
                                            } else {
                                                t.term_yellow
                                            }))
                                            .child(security),
                                    ),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child(snapshot.endpoint.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(status_color))
                                    .child(status_label),
                            )
                            .child(
                                div()
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_secondary))
                                    .child(detail),
                            )
                            .when_some(system_stats, |el, stats| {
                                let memory_percent = Self::memory_percent_from_bytes(&stats);
                                el.child(
                                    h_flex()
                                        .gap(px(10.0))
                                        .text_size(ui_text_sm(cx))
                                        .child(
                                            h_flex()
                                                .gap(px(4.0))
                                                .child(
                                                    div()
                                                        .text_color(rgb(t.text_muted))
                                                        .child("CPU"),
                                                )
                                                .child(
                                                    div()
                                                        .text_color(rgb(Self::cpu_metric_color(
                                                            stats.cpu_usage,
                                                            t,
                                                        )))
                                                        .child(format!(
                                                            "{:02.0}%",
                                                            stats.cpu_usage
                                                        )),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap(px(4.0))
                                                .min_w_0()
                                                .child(
                                                    div()
                                                        .text_color(rgb(t.text_muted))
                                                        .child("MEM"),
                                                )
                                                .child(
                                                    div()
                                                        .min_w_0()
                                                        .overflow_hidden()
                                                        .text_ellipsis()
                                                        .text_color(rgb(Self::memory_metric_color(
                                                            memory_percent,
                                                            t,
                                                        )))
                                                        .child(Self::format_memory_bytes(&stats)),
                                                ),
                                        ),
                                )
                            }),
                    )
                    .into_any_element(),
            );
        }

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomRight)
                .snap_to_window()
                .child(
                    okena_ui::popover::popover_panel("remote-status-popover", t)
                        .w(px(360.0))
                        .max_h(px(280.0))
                        .overflow_y_scroll()
                        .child(
                            h_flex()
                                .justify_between()
                                .pb(px(6.0))
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(rgb(t.text_secondary))
                                        .child("REMOTE CONNECTIONS"),
                                )
                                .child(
                                    div()
                                        .text_size(ui_text_sm(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("{}", snapshots.len())),
                                ),
                        )
                        .children(rows),
                ),
        )
        .into_any_element()
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let stats = self.cache.lock().stats();
        let remote_snapshots = self.remote_snapshots(cx);

        // Get current time using chrono-free approach
        let time_str = Self::format_time();

        let memory_detail = format!(
            "{:.1}/{:.1} GB",
            stats.memory_used_gb, stats.memory_total_gb
        );
        let memory_percent = if stats.memory_total_gb > 0.0 {
            (stats.memory_used_gb / stats.memory_total_gb * 100.0) as u32
        } else {
            0
        };

        // Same thresholds as the remote rows in the popover.
        let cpu_color = Self::cpu_metric_color(stats.cpu_usage, &t);
        let mem_color = Self::memory_metric_color(memory_percent as u64, &t);

        let status_bar = settings_entity(cx).read(cx).settings.status_bar.clone();
        let bar_style = status_bar.style;
        let graphs = status_bar.metrics_graph;

        let cpu_metric = SystemMetric {
            id: "cpu-status-metric",
            label: "CPU",
            value_text: format!("{:02.0}%", stats.cpu_usage),
            tooltip: format!("CPU {:.0}%", stats.cpu_usage),
            fraction: (stats.cpu_usage / 100.0).clamp(0.0, 1.0),
            history: stats.cpu_history.clone(),
            color: cpu_color,
        };
        let memory_metric = SystemMetric {
            id: "memory-status-metric",
            label: "MEM",
            value_text: format!("{memory_percent}%"),
            tooltip: format!("MEM {memory_percent}% — {memory_detail}"),
            fraction: memory_percent.min(100) as f32 / 100.0,
            history: stats.memory_history.clone(),
            color: mem_color,
        };

        // Collect widgets in stable registry order from active extensions
        let left_widgets: Vec<&Vec<AnyView>> = self
            .activate_fns
            .iter()
            .filter_map(|(id, _)| self.active_extensions.get(id))
            .map(|inst| &inst.status_bar_widgets)
            .filter(|w| !w.is_empty())
            .collect();
        let right_widgets: Vec<&Vec<AnyView>> = self
            .activate_fns
            .iter()
            .filter_map(|(id, _)| self.active_extensions.get(id))
            .map(|inst| &inst.status_bar_right_widgets)
            .filter(|w| !w.is_empty())
            .collect();

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
            .text_size(ui_text_ms(cx))
            // Left side - sidebar toggle (macOS only) + system stats
            .child({
                let mut left = h_flex()
                    .gap(px(16.0))
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
                                .text_size(ui_text_xl(cx))
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
                    .child(Self::render_system_metric(
                        cpu_metric, bar_style, graphs, &t, cx,
                    ))
                    .child(Self::render_system_metric(
                        memory_metric,
                        bar_style,
                        graphs,
                        &t,
                        cx,
                    ));

                // Left-side extension widgets
                for widgets in &left_widgets {
                    for widget in *widgets {
                        left = left.child(widget.clone());
                    }
                }

                left
            })
            // Right side - remote info + version + time
            .child({
                let mut right = h_flex().gap(px(8.0));

                // Right-side extension widgets
                for widgets in &right_widgets {
                    for widget in *widgets {
                        right = right.child(widget.clone());
                    }
                }

                // Show daemon remote endpoint when active. In thin-client mode
                // the server lives in the daemon process, so the GUI no longer
                // has an in-process GlobalRemoteInfo/AuthStore to inspect.
                if let Some(daemon) = crate::remote::local::running_daemon() {
                    let port = daemon.port;
                    right = right.child(
                        div()
                            .id("remote-info")
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    // Neutral footer chrome (matches version/time),
                                    // not a terminal ANSI accent — the accent read
                                    // inconsistent in themes like Pastel.
                                    .text_color(rgb(t.text_secondary))
                                    .child(format!("REMOTE :{}", port)),
                            )
                            .child(
                                div()
                                    .id("pair-btn")
                                    .cursor_pointer()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    // White label + hover bg for the clickable
                                    // affordance, instead of the ANSI yellow accent.
                                    .text_color(rgb(t.text_primary))
                                    .text_size(ui_text_sm(cx))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .hover(|s| s.bg(rgb(t.bg_hover)))
                                    .child("Pair")
                                    .on_click(|_, window, cx| {
                                        window.dispatch_action(
                                            Box::new(crate::keybindings::ShowPairingDialog),
                                            cx,
                                        );
                                    }),
                            ),
                    );
                }

                if !remote_snapshots.is_empty() {
                    let connected = remote_snapshots
                        .iter()
                        .filter(|snap| matches!(snap.status, ConnectionStatus::Connected))
                        .count();
                    let status_color = Self::aggregate_remote_color(&remote_snapshots, &t);
                    let entity_for_bounds = cx.entity().clone();

                    right = right.child(
                        div()
                            .id("remote-status-pill")
                            .relative()
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .gap(px(5.0))
                            .px(px(6.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .hover(|s| s.bg(rgb(t.bg_hover)))
                            .on_click(cx.listener(|this, _, _window, cx| {
                                this.remote_popover_visible = !this.remote_popover_visible;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(rgb(status_color)),
                            )
                            .child(div().text_color(rgb(t.text_secondary)).child("REMOTES"))
                            .child(
                                div()
                                    .text_color(rgb(status_color))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{}/{}", connected, remote_snapshots.len())),
                            )
                            .child(
                                canvas(
                                    move |bounds, _window, app| {
                                        entity_for_bounds.update(
                                            app,
                                            |this: &mut StatusBar, _cx| {
                                                this.remote_status_bounds = bounds;
                                            },
                                        );
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            ),
                    );
                } else if self.remote_popover_visible {
                    self.remote_popover_visible = false;
                }

                // Focused project indicator. Project focus narrows the projects
                // grid, so it is hidden while a harness view replaces the grid —
                // otherwise it names a project the user isn't looking at.
                let harness_showing =
                    okena_workspace::harness_state::active_harness(self.window_id, cx).is_some();
                let focused_project = if harness_showing {
                    None
                } else {
                    let ws = self.workspace.read(cx);
                    let fm = self.focus_manager.read(cx);
                    fm.focused_project_id()
                        .and_then(|id| ws.project(id))
                        .map(|p| p.name.clone())
                };

                if let Some(name) = focused_project {
                    let workspace = self.workspace.clone();
                    let focus_manager = self.focus_manager.clone();
                    right = right.child(
                        h_flex()
                            .gap(px(4.0))
                            .child(
                                div()
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_muted))
                                    .child("Focused:"),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .text_size(ui_text_ms(cx))
                                    .text_color(rgb(t.text_primary))
                                    .child(name),
                            )
                            .child(
                                div()
                                    .cursor_pointer()
                                    .px(px(4.0))
                                    .text_size(ui_text_sm(cx))
                                    .text_color(rgb(t.text_muted))
                                    .hover(|s| s.text_color(rgb(t.text_primary)))
                                    .child("✕")
                                    .id("clear-focus-btn")
                                    .on_click(move |_, _window, cx| {
                                        focus_manager.update(cx, |fm, cx| {
                                            workspace.update(cx, |ws, cx| {
                                                ws.set_focused_project(fm, None, cx);
                                            });
                                            cx.notify();
                                        });
                                    }),
                            ),
                    );
                }

                right
                    .when(cfg!(not(target_os = "macos")), |el| {
                        el.child(
                            div()
                                .text_color(rgb(t.text_muted))
                                .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                        )
                    })
                    .child(div().text_color(rgb(t.text_secondary)).child(time_str))
                    .child(self.render_remote_status_popover(&remote_snapshots, &t, cx))
            })
    }
}
