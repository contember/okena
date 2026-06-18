use crate::ui_helpers::{capitalize_first, format_api_timestamp, open_url};
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use okena_extensions::ThemeColors;
use okena_ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Refresh interval for GitHub status
const STATUS_INTERVAL: Duration = Duration::from_secs(60);

/// Hover delay before showing the popover (ms)
const HOVER_DELAY_MS: u64 = 300;

/// A single status update within an incident
#[derive(Clone)]
struct IncidentUpdate {
    status: String,
    body: String,
    created_at: String,
}

/// An unresolved incident on the GitHub status page
#[derive(Clone)]
struct Incident {
    name: String,
    impact: String,
    updates: Vec<IncidentUpdate>,
}

/// Fetched status data for GitHub
#[derive(Clone)]
struct StatusData {
    /// Page-level status indicator: "none" | "minor" | "major" | "critical" | "maintenance"
    indicator: String,
    incidents: Vec<Incident>,
}

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// Global holding a weak handle to the shared status data entity.
///
/// Each window's `GitHubStatus` view keeps a strong handle, so the data entity
/// (and its single poll task) lives exactly as long as at least one window
/// shows the widget — and tears down once they all close.
struct GlobalGitHubStatusData(WeakEntity<GitHubStatusData>);
impl Global for GlobalGitHubStatusData {}

/// Shared status data + the single background poll task.
///
/// Decoupling this from the per-window view means the GitHub status API is
/// fetched once for the whole app rather than once per open window. Per-window
/// UI state (popover, hover) lives on [`GitHubStatus`] instead.
struct GitHubStatusData {
    data: Arc<Mutex<Option<StatusData>>>,
    /// Background polling task. Cancelled automatically when this entity is dropped.
    _poll_task: Task<()>,
}

impl GitHubStatusData {
    /// Get the shared data entity, creating it (and starting the poller) on first use.
    fn shared(cx: &mut App) -> Entity<Self> {
        if let Some(existing) = cx
            .try_global::<GlobalGitHubStatusData>()
            .and_then(|g| g.0.upgrade())
        {
            return existing;
        }
        let entity = cx.new(Self::new);
        cx.set_global(GlobalGitHubStatusData(entity.downgrade()));
        entity
    }

    fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<StatusData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let result = smol::unblock(|| {
                    let resp: serde_json::Value = okena_transport::http::send(
                        okena_transport::http::HttpRequest::get(
                            "https://www.githubstatus.com/api/v2/summary.json",
                        )
                        .timeout(Duration::from_secs(10))
                        .label("github.status")
                        // Safety floor: real cadence is 60s; 5s only ever
                        // catches a runaway re-spawn.
                        .min_interval(Duration::from_secs(5)),
                    )
                    .ok()?
                    .json()
                    .ok()?;

                    let indicator = resp["status"]["indicator"]
                        .as_str()
                        .unwrap_or("none")
                        .to_string();

                    // The summary endpoint only ever lists unresolved incidents,
                    // so no component filtering is needed.
                    let mut incidents = Vec::new();
                    if let Some(incident_list) = resp["incidents"].as_array() {
                        for incident in incident_list {
                            let name = incident["name"].as_str().unwrap_or("Unknown").to_string();
                            let impact = incident["impact"].as_str().unwrap_or("none").to_string();

                            let updates: Vec<IncidentUpdate> = incident["incident_updates"]
                                .as_array()
                                .map(|updates| {
                                    updates
                                        .iter()
                                        .map(|u| IncidentUpdate {
                                            status: u["status"].as_str().unwrap_or("").to_string(),
                                            body: u["body"].as_str().unwrap_or("").to_string(),
                                            created_at: format_api_timestamp(
                                                u["created_at"].as_str().unwrap_or(""),
                                            ),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();

                            incidents.push(Incident {
                                name,
                                impact,
                                updates,
                            });
                        }
                    }

                    Some(StatusData {
                        indicator,
                        incidents,
                    })
                })
                .await;

                if let Some(fetched) = result {
                    *data_for_task.lock() = Some(fetched);
                    if this.update(cx, |_this, cx| cx.notify()).is_err() {
                        break;
                    }
                } else if this.update(cx, |_, _| {}).is_err() {
                    break;
                }

                smol::Timer::after(STATUS_INTERVAL).await;
            }
        });

        Self {
            data,
            _poll_task: poll_task,
        }
    }
}

/// GitHub status indicator with hover popover and click-to-open.
///
/// Unlike the Claude/Codex extensions, the GitHub status page has no single
/// component that represents the whole platform, so we use the page-level
/// `status.indicator` aggregate and surface all unresolved incidents (the
/// Statuspage `summary.json` `incidents` array only contains unresolved ones).
///
/// One of these exists per window; they all share a single [`GitHubStatusData`]
/// poller and hold only per-window UI state.
pub struct GitHubStatus {
    data: Entity<GitHubStatusData>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
}

impl GitHubStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data = GitHubStatusData::shared(cx);
        // Re-render this window's widget whenever the shared poller updates.
        cx.observe(&data, |_, _, cx| cx.notify()).detach();
        Self {
            data,
            popover_visible: false,
            trigger_bounds: Bounds::default(),
            hover_token: Arc::new(AtomicU64::new(0)),
        }
    }

    fn show_popover(&mut self, cx: &mut Context<Self>) {
        if self.popover_visible {
            return;
        }

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
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn render_popover(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let shared = self.data.read(cx);
        let data = shared.data.lock();
        let data = match data.as_ref() {
            Some(d) if self.popover_visible && !d.incidents.is_empty() => d.clone(),
            _ => return div().size_0().into_any_element(),
        };

        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::BottomLeft)
                .snap_to_window()
                .child(
                    div()
                        .id("github-status-popover")
                        .occlude()
                        .min_w(px(320.0))
                        .max_w(px(480.0))
                        .max_h(px(400.0))
                        .overflow_y_scroll()
                        .bg(rgb(t.bg_primary))
                        .border_1()
                        .border_color(rgb(t.border))
                        .rounded(px(6.0))
                        .shadow_lg()
                        .py(px(4.0))
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
                        .on_scroll_wheel(|_, _, cx| {
                            cx.stop_propagation();
                        })
                        .children(data.incidents.iter().enumerate().map(|(idx, incident)| {
                            let impact_color = match incident.impact.as_str() {
                                "critical" | "major" => t.metric_critical,
                                _ => t.metric_warning,
                            };

                            div()
                                .when(idx > 0, |d| d.border_t_1().border_color(rgb(t.border)))
                                .child(
                                    div()
                                        .px(px(10.0))
                                        .py(px(6.0))
                                        .bg(rgb(impact_color))
                                        .when(idx == 0, |d| {
                                            d.rounded_tl(px(5.0)).rounded_tr(px(5.0))
                                        })
                                        .child(
                                            div()
                                                .text_size(ui_text_md(cx))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(rgb(0x000000))
                                                .child(incident.name.clone()),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .px(px(10.0))
                                        .py(px(4.0))
                                        .gap(px(8.0))
                                        .children(incident.updates.iter().map(|update| {
                                            v_flex()
                                                .gap(px(2.0))
                                                .child(
                                                    h_flex()
                                                        .gap(px(4.0))
                                                        .child(
                                                            div()
                                                                .text_size(ui_text_ms(cx))
                                                                .font_weight(FontWeight::BOLD)
                                                                .text_color(rgb(t.text_primary))
                                                                .child(capitalize_first(
                                                                    &update.status,
                                                                )),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_size(ui_text_ms(cx))
                                                                .text_color(rgb(t.text_secondary))
                                                                .child(format!(
                                                                    "- {}",
                                                                    update.body
                                                                )),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(ui_text_sm(cx))
                                                        .text_color(rgb(t.text_muted))
                                                        .child(update.created_at.clone()),
                                                )
                                        })),
                                )
                        })),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl Render for GitHubStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let data = self.data.read(cx).data.lock();
        let (label, color) = match data.as_ref().map(|d| d.indicator.as_str()) {
            Some("none") => ("OK", t.metric_normal),
            Some("minor") => ("Minor", t.metric_warning),
            Some("major") => ("Major Outage", t.metric_critical),
            Some("critical") => ("Critical", t.metric_critical),
            Some("maintenance") => ("Maintenance", t.text_muted),
            Some(_) => ("Unknown", t.text_muted),
            None => ("...", t.text_muted),
        };
        let has_incidents = data
            .as_ref()
            .map(|d| !d.incidents.is_empty())
            .unwrap_or(false);
        drop(data);

        let entity_handle = cx.entity().clone();

        div()
            .child(
                h_flex()
                    .id("github-status-trigger")
                    .cursor_pointer()
                    .gap(px(4.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(div().text_color(rgb(t.text_muted)).child("GitHub"))
                    .child(div().text_color(rgb(color)).child(label))
                    .child(
                        canvas(
                            move |bounds, _window, app| {
                                entity_handle.update(app, |this, _cx| {
                                    this.trigger_bounds = bounds;
                                });
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .when(has_incidents, |d| {
                        d.on_hover(cx.listener(|this, hovered: &bool, _window, cx| {
                            if *hovered {
                                this.show_popover(cx);
                            } else {
                                this.hide_popover(cx);
                            }
                        }))
                    })
                    .on_click(|_, _, _cx| {
                        open_url("https://www.githubstatus.com");
                    }),
            )
            .child(self.render_popover(&t, cx))
    }
}
