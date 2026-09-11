use crate::keybindings::ToggleSidebar;
use crate::remote_client::manager::RemoteConnectionManager;
use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::metrics::{SparklineStyle, StatusBarStyle, metric_bar, sparkline};
use crate::ui::tokens::{ui_text_ms, ui_text_sm, ui_text_xl};
use crate::workspace::state::{ProjectLayoutMode, WindowId, Workspace};
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
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The window this footer belongs to. The grid controls act on one
    /// window's grid, and another window's must not move with it; the focused
    /// project indicator is per-window for the same reason.
    window_id: WindowId,
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
    /// Which grid menu is open, if any, and where each button ended up so the
    /// menu can hang off the right one.
    grid_menu: Option<GridMenu>,
    grid_button_bounds: HashMap<GridMenu, Bounds<Pixels>>,
    /// Where the pointer is. Read together by `sync_grid_menu`, because the
    /// two elements report their hovers independently and in no fixed order.
    grid_hover_button: Option<GridMenu>,
    grid_hover_panel: bool,
    /// Invalidates a pending open or close, so the latest reading wins.
    ///
    /// The button and its menu are separate elements, so crossing from one to
    /// the other reports "not hovering" for an instant. Without a delay that a
    /// later reading can cancel, the menu would shut under the pointer on its
    /// way in.
    grid_hover_token: Arc<AtomicU64>,
}

/// How long a hover has to settle before a menu opens.
///
/// Enough that sweeping the pointer across the footer to reach the clock does
/// not flash both menus on the way past.
const GRID_MENU_OPEN_MS: u64 = 140;

/// How long the menu survives the pointer leaving it, which is the window in
/// which the pointer can cross the gap between button and menu.
const GRID_MENU_CLOSE_MS: u64 = 160;

/// The two questions the footer's grid buttons ask.
///
/// Separate buttons rather than one, because the answers are unrelated:
/// rearranging the columns has nothing to do with whether they open on a
/// terminal or on a session's facts, and pairing them made a menu you had to
/// read past half of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GridMenu {
    /// How the columns are arranged.
    Layout,
    /// What each column opens on.
    Content,
}

impl GridMenu {
    fn button_id(self) -> &'static str {
        match self {
            GridMenu::Layout => "grid-layout-button",
            GridMenu::Content => "grid-content-button",
        }
    }

    fn panel_id(self) -> &'static str {
        match self {
            GridMenu::Layout => "grid-layout-menu",
            GridMenu::Content => "grid-content-menu",
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            GridMenu::Layout => "How the columns are arranged",
            GridMenu::Content => "What each column opens on",
        }
    }
}

impl StatusBar {
    pub fn new(
        window_id: WindowId,
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
        // And when a harness view opens or closes: it hides the focused
        // project indicator, and it replaces the grid, so the grid controls
        // have to leave with it. That selection lives in neither entity above.
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
            grid_menu: None,
            grid_button_bounds: HashMap::new(),
            grid_hover_button: None,
            grid_hover_panel: false,
            grid_hover_token: Arc::new(AtomicU64::new(0)),
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
    /// The grid controls, or `None` when a grid is not what is on screen.
    ///
    /// Two buttons, because they answer two unrelated questions: how the
    /// columns are arranged, and what each one opens on. Both are about a grid
    /// of several things, so a single focused project, a zoomed one, or a
    /// harness view in the grid's place all leave nothing for them to act on.
    fn render_grid_controls(
        &self,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.grid_controls_apply(cx) {
            return None;
        }
        let window_id = self.window_id;
        let (rows, show_info) = {
            let ws = self.workspace.read(cx);
            (
                ws.grid_layout_mode(window_id).is_rows(),
                ws.grid_show_info(window_id),
            )
        };

        Some(
            h_flex()
                .gap(px(2.0))
                .child(self.grid_button(GridMenu::Layout, layout_label(rows), t, cx))
                .child(self.grid_button(GridMenu::Content, content_label(show_info), t, cx))
                .into_any_element(),
        )
    }

    /// One footer button: what it is set to now, and a menu to change it.
    fn grid_button(
        &self,
        menu: GridMenu,
        label: &'static str,
        t: &okena_core::theme::ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.grid_menu == Some(menu);
        let entity = cx.entity().clone();
        h_flex()
            .id(menu.button_id())
            .cursor_pointer()
            .flex_shrink_0()
            .items_center()
            .gap(px(3.0))
            .px(px(6.0))
            .rounded(px(4.0))
            .when(open, |d| d.bg(rgb(t.bg_hover)))
            .when(!open, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .text_color(rgb(t.text_secondary))
            // The current setting is the label: a button that only said
            // "Layout" would hide the one thing worth seeing at a glance.
            .child(label)
            // The menu opens against this button, and the bounds a canvas
            // reports are window-absolute — which is what `anchored` wants.
            .child(
                canvas(
                    move |bounds, _window, app| {
                        entity.update(app, |this, _cx| {
                            this.grid_button_bounds.insert(menu, bounds);
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .tooltip(move |window, cx| Tooltip::new(menu.tooltip()).build(window, cx))
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                this.hover_grid_button(menu, *hovered, cx);
            }))
            .into_any_element()
    }

    /// Note that the pointer entered or left one of the buttons.
    fn hover_grid_button(&mut self, menu: GridMenu, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.grid_hover_button = Some(menu);
        } else if self.grid_hover_button == Some(menu) {
            // Only clear what this button set: moving to its sibling may
            // already have claimed the flag.
            self.grid_hover_button = None;
        }
        self.sync_grid_menu(cx);
    }

    /// Note that the pointer entered or left the open menu.
    fn hover_grid_panel(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.grid_hover_panel = hovered;
        self.sync_grid_menu(cx);
    }

    /// Decide what should be showing from where the pointer actually is.
    ///
    /// Deliberately not driven by the hover events themselves. `on_hover` is
    /// edge-triggered and the two elements fire independently: crossing from
    /// the button into its menu produces both a "left the button" and an
    /// "entered the menu", in no guaranteed order. Acting on each event in
    /// turn meant a late-arriving "left the button" scheduled a close that
    /// nothing was left to cancel — and the menu vanished as you reached it.
    /// Reading both flags together has no such order to get wrong.
    fn sync_grid_menu(&mut self, cx: &mut Context<Self>) {
        let wanted = match self.grid_hover_button {
            // A button wins over the panel, so sliding onto the sibling
            // button swaps menus rather than keeping the old one alive.
            Some(menu) => Some(menu),
            None if self.grid_hover_panel => self.grid_menu,
            None => None,
        };

        // Whatever was pending is now stale either way.
        let token = self.grid_hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        let hover_token = self.grid_hover_token.clone();

        let Some(menu) = wanted else {
            if self.grid_menu.is_none() {
                return;
            }
            cx.spawn(async move |this: WeakEntity<Self>, cx| {
                smol::Timer::after(Duration::from_millis(GRID_MENU_CLOSE_MS)).await;
                if hover_token.load(Ordering::SeqCst) != token {
                    return;
                }
                let _ = this.update(cx, |this, cx| {
                    if hover_token.load(Ordering::SeqCst) == token {
                        this.close_grid_menu(cx);
                    }
                });
            })
            .detach();
            return;
        };

        if self.grid_menu == Some(menu) {
            return;
        }
        // Moving along a row of menus that are already showing is browsing
        // them, not opening one; pausing over each would feel broken.
        if self.grid_menu.is_some() {
            self.grid_menu = Some(menu);
            cx.notify();
            return;
        }
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(GRID_MENU_OPEN_MS)).await;
            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token {
                    this.grid_menu = Some(menu);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Shut the menu and forget where the pointer was.
    ///
    /// The panel only exists while a menu is open, so a stale "the pointer is
    /// on the menu" would otherwise keep the next one alive forever.
    fn close_grid_menu(&mut self, cx: &mut Context<Self>) {
        self.grid_menu = None;
        self.grid_hover_panel = false;
        self.grid_hover_token.fetch_add(1, Ordering::SeqCst);
        cx.notify();
    }

    /// Whether the grid controls have anything to act on right now.
    fn grid_controls_apply(&self, cx: &App) -> bool {
        let focus = self.focus_manager.read(cx);
        shows_grid_controls(
            okena_workspace::harness_state::active_harness(self.window_id, cx).is_some(),
            focus.fullscreen_project_id().is_some(),
            focus.focused_project_id().is_some(),
        )
    }

    /// The open grid menu, growing upward from its own button.
    ///
    /// Rendered off the bar's root rather than inside it: the footer is 22px
    /// tall and would clip this, and the bounds captured above are
    /// window-absolute, so placing it inside a positioned ancestor would
    /// offset it by that ancestor's own origin.
    fn render_grid_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.grid_menu?;
        // Focusing a project or opening Tasks takes the buttons away with the
        // rest of the controls. Forgetting the menu was open, rather than only
        // hiding it, stops it reappearing unbidden on the way back.
        if !self.grid_controls_apply(cx) {
            self.close_grid_menu(cx);
            return None;
        }
        let t = theme(cx);
        let window_id = self.window_id;
        let (rows, show_info) = {
            let ws = self.workspace.read(cx);
            (
                ws.grid_layout_mode(window_id).is_rows(),
                ws.grid_show_info(window_id),
            )
        };
        let bounds = self
            .grid_button_bounds
            .get(&menu)
            .copied()
            .unwrap_or_default();
        // Anchored by its bottom-right to the button's top-right: the buttons
        // sit at the bottom of the window on its right-hand side, so a menu
        // has to grow up and leftward to have anywhere to go.
        //
        // Flush rather than offset, because the pointer has to travel from the
        // button into the menu — a gap between them is dead space that would
        // start closing the very menu you are reaching for.
        let position = point(bounds.origin.x + bounds.size.width, bounds.origin.y);

        let panel = okena_ui::menu::context_menu_panel(menu.panel_id(), &t)
            .min_w(px(180.0))
            // On the panel itself, not on a wrapper around it: the hitbox that
            // occludes is the one that has to receive the hover keeping this
            // menu alive, and nesting the two put a layer between them.
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_grid_menu(cx);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                this.hover_grid_panel(*hovered, cx);
            }));
        let panel = match menu {
            GridMenu::Layout => panel
                .child(self.grid_menu_toggle(
                    "grid-menu-columns",
                    "Columns",
                    !rows,
                    "icons/split-vertical.svg",
                    move |this, cx| {
                        let ws = this.workspace.clone();
                        ws.update(cx, |ws, cx| {
                            ws.set_grid_layout_mode(window_id, ProjectLayoutMode::Columns, cx)
                        });
                    },
                    cx,
                ))
                .child(self.grid_menu_toggle(
                    "grid-menu-stacked",
                    "Stacked",
                    rows,
                    "icons/split-horizontal.svg",
                    move |this, cx| {
                        let ws = this.workspace.clone();
                        ws.update(cx, |ws, cx| {
                            ws.set_grid_layout_mode(window_id, ProjectLayoutMode::Rows, cx)
                        });
                    },
                    cx,
                ))
                // Listed before it exists, so the menu keeps its shape when it
                // lands. Disabled rather than absent: "coming soon" is an
                // answer, and a missing entry is not.
                .child(okena_ui::menu::menu_item_disabled(
                    "grid-menu-canvas",
                    "icons/select-all.svg",
                    "Canvas — coming soon",
                    &t,
                )),
            GridMenu::Content => panel
                .child(self.grid_menu_toggle(
                    "grid-menu-info",
                    "Info",
                    show_info,
                    "icons/lightbulb.svg",
                    move |this, cx| {
                        let ws = this.workspace.clone();
                        ws.update(cx, |ws, cx| ws.set_grid_show_info(window_id, true, cx));
                    },
                    cx,
                ))
                .child(self.grid_menu_toggle(
                    "grid-menu-terminals",
                    "Terminals",
                    !show_info,
                    "icons/terminal.svg",
                    move |this, cx| {
                        let ws = this.workspace.clone();
                        ws.update(cx, |ws, cx| ws.set_grid_show_info(window_id, false, cx));
                    },
                    cx,
                )),
        };

        Some(
            deferred(
                anchored()
                    .position(position)
                    .anchor(Anchor::BottomRight)
                    .snap_to_window()
                    .child(panel),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    /// One row of the grid menu, with a check when it is the current choice.
    fn grid_menu_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        idle_icon: &'static str,
        apply: impl Fn(&mut StatusBar, &mut Context<StatusBar>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        okena_ui::menu::menu_item(
            id,
            if on { "icons/check.svg" } else { idle_icon },
            label,
            &t,
        )
        .on_click(cx.listener(move |this, _, _window, cx| {
            // Picking one of two exclusive options answers the question the
            // menu asked, so it closes.
            this.close_grid_menu(cx);
            apply(this, cx);
            cx.notify();
        }))
        .into_any_element()
    }

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

/// What the current layout is called, on the button and in the menu.
fn layout_label(rows: bool) -> &'static str {
    if rows { "Stacked" } else { "Columns" }
}

/// What the current column contents are called.
fn content_label(show_info: bool) -> &'static str {
    if show_info { "Info" } else { "Terminals" }
}

/// Whether the grid controls belong in the footer.
///
/// They configure a grid of several things. A harness view has taken the
/// grid's place; focus and zoom have narrowed it to one project, which has no
/// columns to arrange and no second thing to show info for.
fn shows_grid_controls(harness_view: bool, fullscreen: bool, focused: bool) -> bool {
    !harness_view && !fullscreen && !focused
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

        // Built before the widget borrows below, which hold `&self` for the
        // rest of this function.
        let grid_menu = self.render_grid_menu(cx);

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
            // Deferred and window-anchored, so it escapes this 22px strip
            // rather than being clipped by it.
            .children(grid_menu)
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
                // First in the right-hand group: it is a control rather than a
                // reading, so it sits away from the clock and the connection
                // status at the far edge.
                let mut right = h_flex()
                    .gap(px(8.0))
                    .children(self.render_grid_controls(&t, cx));

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

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{content_label, layout_label, shows_grid_controls};

    #[test]
    fn the_button_and_its_menu_call_each_mode_the_same_thing() {
        // The button wears the current choice and the menu offers it; two
        // spellings of "Stacked" would read as two different settings.
        assert_eq!(layout_label(false), "Columns");
        assert_eq!(layout_label(true), "Stacked");
        assert_eq!(content_label(true), "Info");
        assert_eq!(content_label(false), "Terminals");
    }

    #[test]
    fn the_grid_controls_belong_to_a_grid_and_nothing_else() {
        assert!(
            shows_grid_controls(false, false, false),
            "an overview is a grid"
        );
        // Each of these on its own means there is no grid to arrange, and the
        // controls would sit in the footer offering to rearrange columns that
        // are not on screen.
        assert!(
            !shows_grid_controls(true, false, false),
            "Tasks took the grid"
        );
        assert!(
            !shows_grid_controls(false, false, true),
            "one project is not a grid"
        );
        assert!(!shows_grid_controls(false, true, false), "zoom is for room");
    }
}
