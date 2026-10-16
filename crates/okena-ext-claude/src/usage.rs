//! Claude Code usage widget: status-bar trigger + hover popover with chart.
//!
//! The numbers come from the daemon (`GET /v1/usage` / `usage_changed`), via
//! the app-installed [`GlobalDaemonUsage`] entity. This widget never fetches;
//! opening the popover asks the daemon to refresh (throttled there to 60 s).
//! While a host routes new shells through the AI proxy, the widget shows the
//! proxy's account pool instead.

use crate::pool_usage::{AccountKey, PoolHistory};
use crate::settings::PoolDisplay;
use crate::ui_helpers::capitalize_first;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_core::api::{
    ApiExtraUsage, ApiProviderUsage, ApiUsageProvider, ApiUsageWindow, ApiUsageWindowKind,
};
use okena_extensions::{ExtensionSettingsStore, ThemeColors};
use okena_usage::{
    DaemonUsage, GlobalDaemonUsage, WorkingDays, provider_usage, read_working_days,
    render_simple_bar, render_usage_row, usage_body_container, usage_divider, usage_kv_row,
    usage_popover_container, usage_popover_header, usage_trigger_items, window_points, window_row,
    window_trigger_item,
};
use okena_workspace::remote_sync::HostAiAccounts;
use okena_workspace::state::{GlobalWorkspace, Workspace};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// A status-bar entry with its own popover.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
enum Trigger {
    /// The `claude` login's own usage.
    #[default]
    Login,
    Account(AccountKey),
    /// The pool summary of the host with this connection id.
    Pool(String),
}

/// Claude API usage indicator with hover popover.
///
/// One of these exists per window; they all read the shared
/// [`GlobalDaemonUsage`] snapshot and hold only per-window UI state.
pub struct ClaudeUsage {
    store: Entity<DaemonUsage>,
    /// AI proxy accounts of the connected hosts; when any host has some, they
    /// are shown instead of the `claude` login's usage.
    pool: Vec<HostAiAccounts>,
    pool_history: Entity<PoolHistory>,
    active: Trigger,
    trigger_bounds: BTreeMap<Trigger, Bounds<Pixels>>,
    workspace: Option<Entity<Workspace>>,
    _workspace_observation: Option<Subscription>,
    chart_state: Entity<okena_usage::HistoryChartState>,
    popover_visible: bool,
    hover_token: Arc<AtomicU64>,
    history_refresh_task: Option<Task<()>>,
    _summary_history_task: Task<()>,
}

impl ClaudeUsage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = GlobalDaemonUsage::entity(cx);
        let pool_history = PoolHistory::shared(cx);
        cx.observe(&pool_history, |_, _, cx| cx.notify()).detach();
        let chart_state = cx.new(|_| okena_usage::HistoryChartState::default());
        cx.observe(&chart_state, |_, _, cx| cx.notify()).detach();
        // Re-render whenever the daemon pushes a new snapshot.
        cx.observe(&store, |_, _, cx| cx.notify()).detach();
        // Extensions may activate before the app installs the workspace.
        cx.observe_global::<GlobalWorkspace>(|this, cx| this.bind_workspace(cx))
            .detach();
        cx.observe_global::<ExtensionSettingsStore>(|this, cx| {
            this.request_summary_history(cx);
            cx.notify();
        })
        .detach();
        // The summary reads every account's history; the shared cache
        // refetches each at most once a minute.
        let summary_history_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                smol::Timer::after(Duration::from_secs(60)).await;
                if this
                    .update(cx, |this, cx| this.request_summary_history(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut this = Self {
            store,
            pool: Vec::new(),
            pool_history,
            active: Trigger::Login,
            trigger_bounds: BTreeMap::new(),
            workspace: None,
            _workspace_observation: None,
            chart_state,
            popover_visible: false,
            hover_token: Arc::new(AtomicU64::new(0)),
            history_refresh_task: None,
            _summary_history_task: summary_history_task,
        };
        this.bind_workspace(cx);
        this
    }

    fn bind_workspace(&mut self, cx: &mut Context<Self>) {
        let workspace = cx
            .try_global::<GlobalWorkspace>()
            .map(|global| global.0.clone());
        if self.workspace == workspace {
            return;
        }
        self._workspace_observation = workspace.as_ref().map(|workspace| {
            cx.observe(workspace, |this, workspace, cx| {
                this.update_pool(&workspace, cx);
            })
        });
        if let Some(workspace) = &workspace {
            self.update_pool(workspace, cx);
        }
        self.workspace = workspace;
    }

    fn update_pool(&mut self, workspace: &Entity<Workspace>, cx: &mut Context<Self>) {
        let pool = workspace.read(cx).remote_sync.ai_accounts();
        if self.pool != pool {
            self.pool = pool.to_vec();
            let pool = &self.pool;
            self.trigger_bounds.retain(|trigger, _| match trigger {
                Trigger::Login => true,
                Trigger::Account(key) => pool.iter().any(|host| {
                    host.connection_id == key.connection_id
                        && host
                            .accounts
                            .iter()
                            .any(|account| account.id == key.account_id)
                }),
                Trigger::Pool(connection_id) => {
                    pool.iter().any(|host| &host.connection_id == connection_id)
                }
            });
            if !self.trigger_bounds.contains_key(&self.active) {
                self.hover_token.fetch_add(1, Ordering::SeqCst);
                self.active = Trigger::Login;
                self.popover_visible = false;
                self.history_refresh_task = None;
            }
            if self.popover_visible {
                self.request_pool_history(cx);
            }
            self.request_summary_history(cx);
            cx.notify();
        }
    }

    fn request_history(&self, keys: Vec<AccountKey>, cx: &mut Context<Self>) {
        self.pool_history.update(cx, |history, cx| {
            for key in keys {
                history.request(key, cx);
            }
        });
    }

    fn host_keys(host: &HostAiAccounts) -> impl Iterator<Item = AccountKey> + '_ {
        host.accounts
            .iter()
            .map(move |account| AccountKey::new(host, account))
    }

    fn request_pool_history(&self, cx: &mut Context<Self>) {
        let keys = match &self.active {
            Trigger::Login => return,
            Trigger::Account(key) => vec![key.clone()],
            Trigger::Pool(connection_id) => self
                .pool
                .iter()
                .filter(|host| &host.connection_id == connection_id)
                .flat_map(Self::host_keys)
                .collect(),
        };
        self.request_history(keys, cx);
    }

    fn request_summary_history(&self, cx: &mut Context<Self>) {
        if !PoolDisplay::read(cx).summary() {
            return;
        }
        let keys = self.pool.iter().flat_map(Self::host_keys).collect();
        self.request_history(keys, cx);
    }

    fn show_popover(&mut self, trigger: Trigger, cx: &mut Context<Self>) {
        if self.popover_visible && self.active == trigger {
            self.hover_token.fetch_add(1, Ordering::SeqCst);
            return;
        }
        self.active = trigger;
        self.popover_visible = false;
        self.history_refresh_task = None;
        cx.notify();

        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;
        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(HOVER_DELAY_MS)).await;

            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token {
                    this.popover_visible = true;
                    this.request_pool_history(cx);
                    if this.active != Trigger::Login {
                        this.history_refresh_task =
                            Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
                                loop {
                                    smol::Timer::after(Duration::from_secs(60)).await;
                                    if this
                                        .update(cx, |this, cx| this.request_pool_history(cx))
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }));
                    }
                    if this.pool.is_empty() {
                        GlobalDaemonUsage::request_refresh(ApiUsageProvider::Claude, cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn hide_popover(&mut self, cx: &mut Context<Self>) {
        let token = self.hover_token.fetch_add(1, Ordering::SeqCst) + 1;

        if !self.popover_visible {
            return;
        }

        let hover_token = self.hover_token.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(100)).await;

            if hover_token.load(Ordering::SeqCst) != token {
                return;
            }

            let _ = this.update(cx, |this, cx| {
                if hover_token.load(Ordering::SeqCst) == token && this.popover_visible {
                    this.popover_visible = false;
                    this.history_refresh_task = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.popover_visible {
            return div().size_0().into_any_element();
        }
        let working = read_working_days(cx);
        let Some(bounds) = self.trigger_bounds.get(&self.active).copied() else {
            return div().size_0().into_any_element();
        };
        let (title, plan, body) = match &self.active {
            Trigger::Account(key) => {
                let Some(host) = self
                    .pool
                    .iter()
                    .find(|host| host.connection_id == key.connection_id)
                else {
                    return div().size_0().into_any_element();
                };
                let Some(account) = host
                    .accounts
                    .iter()
                    .find(|account| account.id == key.account_id)
                else {
                    return div().size_0().into_any_element();
                };
                let history = self
                    .pool_history
                    .read(cx)
                    .entries
                    .get(key)
                    .map(|entry| entry.history.clone())
                    .unwrap_or_default();
                (
                    "CLAUDE ACCOUNT",
                    None,
                    div()
                        .child(crate::pool_usage::render_body(
                            host,
                            account,
                            &history,
                            working,
                            &self.chart_state,
                            t,
                            cx,
                        ))
                        .when_some(
                            self.pool_history
                                .read(cx)
                                .entries
                                .get(key)
                                .and_then(|entry| entry.error.as_ref()),
                            |el, error| {
                                el.child(
                                    div()
                                        .px(px(12.0))
                                        .pb(px(10.0))
                                        .text_size(okena_ui::tokens::ui_text_xs(cx))
                                        .text_color(rgb(t.text_muted))
                                        .child(format!("History unavailable: {error}")),
                                )
                            },
                        )
                        .into_any_element(),
                )
            }
            Trigger::Pool(connection_id) => {
                let Some(host) = self
                    .pool
                    .iter()
                    .find(|host| &host.connection_id == connection_id)
                else {
                    return div().size_0().into_any_element();
                };
                let summary =
                    crate::pool_summary::summarize(host, self.pool_history.read(cx), working);
                (
                    "CLAUDE POOL",
                    summary.host_name.clone(),
                    crate::pool_summary::render_body(&summary, t, cx).into_any_element(),
                )
            }
            Trigger::Login => {
                if !self.pool.is_empty() {
                    return div().size_0().into_any_element();
                }
                let Some(data) = provider_usage(ApiUsageProvider::Claude, cx)
                    .filter(|data| data.fetched_at.is_some())
                else {
                    return div().size_0().into_any_element();
                };
                let source = self.store.read(cx).source_label().cloned();
                (
                    "CLAUDE USAGE",
                    header_badge(data.plan.as_deref(), source.as_deref()),
                    self.render_usage_body(&data, working, t, cx)
                        .into_any_element(),
                )
            }
        };
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    usage_popover_container(t)
                        .id("claude-usage-popover")
                        .occlude()
                        .on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            if *hovered {
                                this.hover_token.fetch_add(1, Ordering::SeqCst);
                            } else {
                                this.hide_popover(cx);
                            }
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| {
                            cx.stop_propagation();
                        })
                        .child(usage_popover_header(
                            title,
                            plan.as_deref(),
                            "https://claude.ai/settings/usage",
                            "Open usage settings on claude.ai",
                            t,
                            cx,
                        ))
                        .child(body),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn render_usage_body(
        &self,
        data: &ApiProviderUsage,
        working: WorkingDays,
        t: &ThemeColors,
        cx: &App,
    ) -> Div {
        let rows: Vec<AnyElement> = data
            .windows
            .iter()
            .filter_map(|window| {
                let (label, marker_id) = popover_row_identity(window)?;
                Some(
                    render_usage_row(
                        t,
                        cx,
                        &window_row(window, label, marker_id),
                        working,
                        window_points(window),
                        &self.chart_state,
                    )
                    .into_any_element(),
                )
            })
            .collect();

        usage_body_container()
            .children(rows)
            .when_some(data.extra_usage.as_ref(), |el, extra| {
                if !extra.is_enabled {
                    return el;
                }
                el.child(usage_divider(t))
                    .child(render_extra_usage_row(t, cx, extra))
            })
    }
}

/// Plan badge, plus the daemon's machine when it is not this one.
fn header_badge(plan: Option<&str>, source: Option<&str>) -> Option<String> {
    let plan = plan.map(capitalize_first);
    match (plan, source) {
        (Some(plan), Some(source)) => Some(format!("{plan} · {source}")),
        (Some(plan), None) => Some(plan),
        (None, Some(source)) => Some(source.to_string()),
        (None, None) => None,
    }
}

/// Popover label + marker id for a window, or `None` when it is hidden.
fn popover_row_identity(window: &ApiUsageWindow) -> Option<(SharedString, SharedString)> {
    match &window.kind {
        ApiUsageWindowKind::FiveHour => Some(("Session".into(), "claude-marker-session".into())),
        ApiUsageWindowKind::Weekly => Some(("Weekly".into(), "claude-marker-weekly".into())),
        ApiUsageWindowKind::WeeklyScoped { model } if should_show_scoped(window) => {
            Some((model.clone().into(), marker_id_for_scoped(model)))
        }
        _ => None,
    }
}

/// Status-bar label for a window, or `None` when it stays out of the trigger.
fn trigger_label(window: &ApiUsageWindow) -> Option<SharedString> {
    match &window.kind {
        ApiUsageWindowKind::FiveHour => Some("5h".into()),
        ApiUsageWindowKind::Weekly => Some("7d".into()),
        ApiUsageWindowKind::WeeklyScoped { model } if should_show_scoped_in_trigger(window) => {
            Some(model.clone().into())
        }
        _ => None,
    }
}

fn should_show_scoped(window: &ApiUsageWindow) -> bool {
    window.active || window.used_percent >= 0.5
}

fn should_show_scoped_in_trigger(window: &ApiUsageWindow) -> bool {
    window.active || window.used_percent >= 80.0
}

fn marker_id_for_scoped(label: &str) -> SharedString {
    let slug: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("claude-marker-scoped-{slug}").into()
}

fn render_extra_usage_row(t: &ThemeColors, cx: &App, extra: &ApiExtraUsage) -> impl IntoElement {
    v_flex()
        .gap(px(5.0))
        .child(usage_kv_row(
            t,
            cx,
            "Extra Usage",
            format!(
                "${:.2} / ${:.2}",
                extra.used_credits_cents / 100.0,
                extra.monthly_limit_cents / 100.0
            ),
            t.text_primary,
        ))
        .child(render_simple_bar(t, extra.utilization))
}

impl Render for ClaudeUsage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let working = read_working_days(cx);
        let triggers = if self.pool.is_empty() {
            let items = login_trigger_items(working, cx);
            if items.is_empty() {
                return div().size_0().into_any_element();
            }
            vec![self.render_trigger(Trigger::Login, usage_trigger_items(&t, cx, &items), &t, cx)]
        } else {
            let display = PoolDisplay::read(cx);
            let mut triggers = Vec::new();
            for host in &self.pool {
                if display.summary() {
                    let summary =
                        crate::pool_summary::summarize(host, self.pool_history.read(cx), working);
                    let content = crate::pool_summary::render_trigger(&summary, display, &t, cx);
                    triggers.push(self.render_trigger(
                        Trigger::Pool(host.connection_id.clone()),
                        content,
                        &t,
                        cx,
                    ));
                }
                if display.accounts {
                    for account in &host.accounts {
                        let content = crate::pool_usage::render_trigger(account, working, &t, cx);
                        triggers.push(self.render_trigger(
                            Trigger::Account(AccountKey::new(host, account)),
                            vec![content],
                            &t,
                            cx,
                        ));
                    }
                }
            }
            triggers
        };
        h_flex()
            .gap(px(4.0))
            .children(triggers)
            .child(self.render_popover(&t, cx))
            .into_any_element()
    }
}

impl ClaudeUsage {
    fn render_trigger(
        &self,
        trigger: Trigger,
        content: Vec<AnyElement>,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let bounds_trigger = trigger.clone();
        let id: SharedString = match &trigger {
            Trigger::Login => "claude-usage-trigger".into(),
            Trigger::Account(key) => format!("{}-trigger", key.element_id()).into(),
            Trigger::Pool(connection_id) => format!("claude-pool-summary:{connection_id}").into(),
        };
        h_flex()
            .id(id)
            .cursor_pointer()
            .gap(px(8.0))
            .px(px(4.0))
            .py(px(1.0))
            .rounded(px(3.0))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .children(content)
            .child(
                canvas(
                    move |bounds, _window, app| {
                        entity.update(app, |this, _cx| {
                            this.trigger_bounds.insert(bounds_trigger.clone(), bounds);
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .on_hover(cx.listener(move |this, hovered: &bool, _window, cx| {
                if *hovered {
                    this.show_popover(trigger.clone(), cx);
                } else if this.active == trigger {
                    this.hide_popover(cx);
                }
            }))
            .into_any_element()
    }
}

/// The `claude` login's usage for the status bar, empty before the daemon
/// reported any.
fn login_trigger_items(working: WorkingDays, cx: &mut App) -> Vec<okena_usage::TriggerItem> {
    provider_usage(ApiUsageProvider::Claude, cx)
        .map(|data| {
            data.windows
                .iter()
                .filter_map(|window| {
                    let label = trigger_label(window)?;
                    Some(window_trigger_item(window, label, working))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    // gpui::* re-exports a `test` attribute macro that conflicts with the built-in;
    // alias the built-in so `#[test]` works normally in this module.
    use core::prelude::rust_2024::test;

    fn window(kind: ApiUsageWindowKind, used_percent: f64, active: bool) -> ApiUsageWindow {
        ApiUsageWindow {
            id: "x".into(),
            label: "x".into(),
            kind,
            used_percent,
            window_seconds: 604800.0,
            reset_at: None,
            active,
            history: Vec::new(),
        }
    }

    #[test]
    fn scoped_rows_follow_visibility_thresholds() {
        let fable = |pct, active| {
            window(
                ApiUsageWindowKind::WeeklyScoped {
                    model: "Fable".into(),
                },
                pct,
                active,
            )
        };
        assert!(popover_row_identity(&fable(0.0, false)).is_none());
        assert!(popover_row_identity(&fable(0.5, false)).is_some());
        assert!(trigger_label(&fable(79.0, false)).is_none());
        assert_eq!(trigger_label(&fable(10.0, true)).as_deref(), Some("Fable"));
        assert_eq!(
            popover_row_identity(&fable(1.0, false)).map(|(_, id)| id),
            Some("claude-marker-scoped-fable".into())
        );
        assert_eq!(
            trigger_label(&window(ApiUsageWindowKind::FiveHour, 1.0, true)).as_deref(),
            Some("5h")
        );
    }

    #[test]
    fn header_badge_labels_remote_source() {
        assert_eq!(header_badge(Some("max"), None).as_deref(), Some("Max"));
        assert_eq!(
            header_badge(Some("max"), Some("studio")).as_deref(),
            Some("Max · studio")
        );
        assert_eq!(header_badge(None, None), None);
    }
}
