use crate::ui_helpers::{format_api_timestamp, capitalize_first, open_url};
use okena_extensions::ThemeColors;
use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{h_flex, v_flex};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Refresh interval for Claude Code status
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

/// An unresolved incident affecting the Claude Code component
#[derive(Clone)]
struct Incident {
    name: String,
    impact: String,
    updates: Vec<IncidentUpdate>,
}

/// Fetched status data for Claude Code
#[derive(Clone)]
struct StatusData {
    status: String,
    incidents: Vec<Incident>,
}

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// Claude Code status indicator with hover popover and click-to-open.
pub struct ClaudeStatus {
    data: Arc<Mutex<Option<StatusData>>>,
    popover_visible: bool,
    trigger_bounds: Bounds<Pixels>,
    hover_token: Arc<AtomicU64>,
    /// Background polling task. Cancelled automatically when this entity is dropped.
    _poll_task: Task<()>,
}

impl ClaudeStatus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let data: Arc<Mutex<Option<StatusData>>> = Arc::new(Mutex::new(None));
        let data_for_task = data.clone();

        let poll_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            loop {
                let result = smol::unblock(|| {
                    let client = reqwest::blocking::Client::builder()
                        .timeout(Duration::from_secs(10))
                        .user_agent(format!("okena/{}", env!("CARGO_PKG_VERSION")))
                        .build()
                        .ok()?;

                    let resp: serde_json::Value = client
                        .get("https://status.claude.com/api/v2/summary.json")
                        .send()
                        .ok()?
                        .json()
                        .ok()?;

                    // Find the Claude Code component and its ID
                    let components = resp["components"].as_array()?;
                    let mut component_status = None;
                    let mut component_id = None;
                    for component in components {
                        if component["name"].as_str() == Some("Claude Code") {
                            component_status = component["status"].as_str().map(|s| s.to_string());
                            component_id = component["id"].as_str().map(|s| s.to_string());
                            break;
                        }
                    }
                    let status = component_status?;
                    let comp_id = component_id?;

                    // Collect unresolved incidents that affect this component
                    let mut incidents = Vec::new();
                    if let Some(incident_list) = resp["incidents"].as_array() {
                        for incident in incident_list {
                            let affects = incident["components"]
                                .as_array()
                                .map(|comps| {
                                    comps.iter().any(|c| c["id"].as_str() == Some(&comp_id))
                                })
                                .unwrap_or(false);

                            if !affects {
                                continue;
                            }

                            let name =
                                incident["name"].as_str().unwrap_or("Unknown").to_string();
                            let impact =
                                incident["impact"].as_str().unwrap_or("none").to_string();

                            let updates: Vec<IncidentUpdate> = incident["incident_updates"]
                                .as_array()
                                .map(|updates| {
                                    updates
                                        .iter()
                                        .map(|u| IncidentUpdate {
                                            status: u["status"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string(),
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

                    Some(StatusData { status, incidents })
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
            popover_visible: false,
            trigger_bounds: Bounds::default(),
            hover_token: Arc::new(AtomicU64::new(0)),
            _poll_task: poll_task,
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

    fn render_popover(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let data = self.data.lock();
        let data = match data.as_ref() {
            Some(d) if self.popover_visible && !d.incidents.is_empty() => d.clone(),
            _ => return div().size_0().into_any_element(),
        };

        let bounds = self.trigger_bounds;
        let position = point(bounds.origin.x, bounds.origin.y - px(4.0));

        deferred(
            anchored()
                .position(position)
                .anchor(Corner::BottomLeft)
                .snap_to_window()
                .child(
                    div()
                        .id("claude-status-popover")
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
                        .children(
                            data.incidents
                                .iter()
                                .enumerate()
                                .map(|(idx, incident)| {
                                    let impact_color = match incident.impact.as_str() {
                                        "critical" | "major" => t.metric_critical,
                                        _ => t.metric_warning,
                                    };

                                    div()
                                        .when(idx > 0, |d| {
                                            d.border_t_1().border_color(rgb(t.border))
                                        })
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
                                                        .text_size(px(12.0))
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
                                                .children(incident.updates.iter().map(
                                                    |update| {
                                                        v_flex()
                                                            .gap(px(2.0))
                                                            .child(
                                                                h_flex().gap(px(4.0)).child(
                                                                    div()
                                                                        .text_size(px(11.0))
                                                                        .font_weight(
                                                                            FontWeight::BOLD,
                                                                        )
                                                                        .text_color(rgb(
                                                                            t.text_primary,
                                                                        ))
                                                                        .child(capitalize_first(
                                                                            &update.status,
                                                                        )),
                                                                )
                                                                .child(
                                                                    div()
                                                                        .text_size(px(11.0))
                                                                        .text_color(rgb(
                                                                            t.text_secondary,
                                                                        ))
                                                                        .child(format!(
                                                                            "- {}",
                                                                            update.body
                                                                        )),
                                                                ),
                                                            )
                                                            .child(
                                                                div()
                                                                    .text_size(px(10.0))
                                                                    .text_color(rgb(t.text_muted))
                                                                    .child(
                                                                        update
                                                                            .created_at
                                                                            .clone(),
                                                                    ),
                                                            )
                                                    },
                                                )),
                                        )
                                }),
                        ),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }
}

impl Render for ClaudeStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let data = self.data.lock();
        let (label, color) = match data.as_ref().map(|d| d.status.as_str()) {
            Some("operational") => ("OK", t.metric_normal),
            Some("degraded_performance") => ("Degraded", t.metric_warning),
            Some("partial_outage") => ("Partial Outage", t.metric_warning),
            Some("major_outage") => ("Major Outage", t.metric_critical),
            Some("under_maintenance") => ("Maintenance", t.text_muted),
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
                    .id("claude-status-trigger")
                    .cursor_pointer()
                    .gap(px(4.0))
                    .px(px(4.0))
                    .py(px(1.0))
                    .rounded(px(3.0))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(div().text_color(rgb(t.text_muted)).child("Claude Code"))
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
                        open_url("https://status.claude.com");
                    }),
            )
            .child(self.render_popover(&t, cx))
    }
}
