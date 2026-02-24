use gpui::prelude::*;
use gpui::*;
use crate::theme::{theme, ThemeColors};
use crate::ui::tokens::*;
use crate::views::components::dropdown::{
    dropdown_anchored_below, dropdown_button, dropdown_option, dropdown_overlay,
};
use crate::views::components::simple_input::{SimpleInput, SimpleInputState};
use crate::views::components::ui_helpers::*;

use super::config::AGENTS;
use super::types::{EditTarget, KruhState, PlanInfo};
use super::KruhPane;

impl KruhPane {
    pub fn render_view(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let state = self.state.clone();
        let header = self.render_header(cx).into_any_element();
        let body = match state {
            KruhState::Scanning => self.render_scanning(cx).into_any_element(),
            KruhState::PlanPicker => self.render_plan_picker(cx).into_any_element(),
            KruhState::TaskBrowser => self.render_task_browser(window, cx).into_any_element(),
            KruhState::Editing => self.render_editor(cx).into_any_element(),
            KruhState::Settings => self.render_settings(window, cx).into_any_element(),
            _ => self.render_running_view(window, cx).into_any_element(),
        };

        div()
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_primary))
            .text_color(rgb(t.text_primary))
            .child(header)
            .child(body)
    }

    // ── Header ──────────────────────────────────────────────────────────

    fn render_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let has_context = self.selected_plan.is_some()
            || self.state == KruhState::TaskBrowser
            || self.state == KruhState::Editing
            || self.state == KruhState::Settings
            || matches!(
                self.state,
                KruhState::Running | KruhState::Paused | KruhState::WaitingForStep | KruhState::Completed
            );

        if !has_context {
            return div();
        }

        div()
            .flex()
            .items_center()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .gap(SPACE_MD)
            .border_b_1()
            .border_color(rgb(t.border))
            // Breadcrumb: plan name
            .when_some(self.selected_plan.as_ref().map(|p| p.name.clone()), |el, name| {
                let t = theme(cx);
                el.child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(rgb(t.text_muted))
                        .child(name),
                )
            })
            // Spacer
            .child(div().flex_1())
            // Agent/model label
            .child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(rgb(t.text_muted))
                    .child(format!("{} / {}", self.config.agent, self.config.model)),
            )
    }

    // ── Scanning ────────────────────────────────────────────────────────

    fn render_scanning(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .text_size(TEXT_MD)
                    .text_color(rgb(t.text_muted))
                    .child("Scanning for plans..."),
            )
    }

    // ── Plan Picker ─────────────────────────────────────────────────────

    fn render_plan_picker(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // Pre-build plan cards to avoid closure capture issues
        let selected_idx = self.selected_plan_index;
        let plan_cards: Vec<_> = self
            .plans
            .iter()
            .enumerate()
            .map(|(i, plan)| {
                self.render_plan_card(i, plan, i == selected_idx, cx).into_any_element()
            })
            .collect();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("plan-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.plan_scroll)
                    .p(SPACE_LG)
                    .flex()
                    .flex_col()
                    .gap(SPACE_SM)
                    .children(plan_cards),
            )
            .child(
                div()
                    .px(SPACE_MD)
                    .py(SPACE_SM)
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_XS)
                    .child(
                        toolbar_button("pp-settings", "icons/edit.svg", "Settings", &t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state = KruhState::Settings;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .gap(SPACE_XS)
                            .items_center()
                            .child(kbd("\u{2191}\u{2193}", &t))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child("Navigate"),
                            )
                            .child(div().w(SPACE_MD))
                            .child(kbd("Enter", &t))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child("Select"),
                            ),
                    ),
            )
    }

    fn render_plan_card(
        &self,
        index: usize,
        plan: &PlanInfo,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme(cx);
        let is_complete = plan.pending == 0 && plan.total > 0;
        let total = plan.total.max(1) as f32;
        let ratio = plan.done as f32 / total;

        let bar_color = if is_complete {
            rgb(t.success)
        } else if ratio >= 0.5 {
            rgb(t.warning)
        } else {
            rgb(t.error)
        };

        let status_el: AnyElement = if is_complete {
            badge("Completed", &t).into_any_element()
        } else {
            div()
                .text_size(TEXT_SM)
                .text_color(rgb(t.text_muted))
                .child(format!("{}/{}", plan.done, plan.total))
                .into_any_element()
        };

        let mut card = div()
            .id(ElementId::Name(format!("plan-{index}").into()))
            .px(SPACE_LG)
            .py(SPACE_MD)
            .rounded(RADIUS_STD)
            .cursor_pointer()
            .border_1();

        if selected {
            card = card
                .border_color(rgb(t.border_active))
                .bg(rgb(t.bg_secondary));
        } else {
            card = card
                .border_color(rgb(t.border))
                .hover(|s| s.bg(rgb(t.bg_hover)));
        }

        card = card.on_click(cx.listener(move |this, _, _window, cx| {
            this.select_plan(index, cx);
        }));

        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(TEXT_MD)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(plan.name.clone()),
                )
                .child(status_el),
        );

        card = card.child(
            div()
                .mt(SPACE_SM)
                .h(px(4.0))
                .w_full()
                .rounded(RADIUS_MD)
                .bg(rgb(t.bg_secondary))
                .child(
                    div()
                        .h_full()
                        .rounded(RADIUS_MD)
                        .bg(bar_color)
                        .w(relative(ratio)),
                ),
        );

        card
    }

    // ── Task Browser ────────────────────────────────────────────────────

    fn render_task_browser(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        // Pre-build issue rows to avoid closure capture issues
        let issue_rows: Vec<_> = self
            .issues
            .iter()
            .enumerate()
            .map(|(i, issue)| {
                let selected = i == self.selected_issue_index;
                self.render_issue_row_from_detail(i, issue, selected, cx)
            })
            .collect();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("issue-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.issue_scroll)
                    .px(SPACE_LG)
                    .py(SPACE_SM)
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(issue_rows),
            )
            // Footer: icon button toolbar
            .child(
                div()
                    .px(SPACE_MD)
                    .py(SPACE_SM)
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_XS)
                    // Back button
                    .child(
                        toolbar_button("tb-back", "icons/chevron-left.svg", "Back", &t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.navigate_to_plan_picker(cx);
                            })),
                    )
                    // Separator
                    .child(toolbar_separator(&t))
                    // Edit Status
                    .child(
                        toolbar_button("tb-status", "icons/file.svg", "Status", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor(EditTarget::Status, window, cx);
                            })),
                    )
                    // Edit Instructions
                    .child(
                        toolbar_button("tb-instructions", "icons/edit.svg", "Instructions", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_editor(EditTarget::Instructions, window, cx);
                            })),
                    )
                    // Settings button
                    .child(
                        toolbar_button("tb-settings", "icons/edit.svg", "Settings", &t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state = KruhState::Settings;
                                cx.notify();
                            })),
                    )
                    // Spacer pushes Run button to the right
                    .child(div().flex_1().min_w(SPACE_SM))
                    // Run Loop - primary action, always visible
                    .child(
                        toolbar_button_primary("tb-run", "icons/play.svg", "Run Loop", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_loop_from_plan(window, cx);
                            })),
                    ),
            )
    }

    fn render_issue_row_from_detail(
        &self,
        index: usize,
        issue: &super::types::IssueDetail,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme(cx);
        let done = issue.done;
        let number = issue.ref_info.number.clone();
        let name = issue.ref_info.name.clone();

        let icon_path: SharedString = if done {
            "icons/check.svg".into()
        } else {
            "icons/focus.svg".into()
        };

        let text_color = if done { rgb(t.text_muted) } else { rgb(t.text_primary) };
        let icon_color = if done { rgb(t.success) } else { rgb(t.text_muted) };

        let mut label = div()
            .flex()
            .gap(SPACE_SM);

        if !number.is_empty() {
            label = label.child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(rgb(t.text_muted))
                    .child(format!("#{number}")),
            );
        }

        let mut name_el = div()
            .text_size(TEXT_MD)
            .text_color(text_color);
        if done {
            name_el = name_el.line_through();
        }
        name_el = name_el.child(name);

        label = label.child(name_el);

        let override_labels = issue.overrides.labels();

        let mut row = div()
            .id(ElementId::Name(format!("issue-{index}").into()))
            .px(SPACE_MD)
            .py(SPACE_XS)
            .rounded(RADIUS_STD)
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(SPACE_MD);

        if selected {
            row = row.bg(rgb(t.bg_secondary));
        } else {
            row = row.hover(|s| s.bg(rgb(t.bg_hover)));
        }

        row = row.child(
            svg()
                .path(icon_path)
                .size(ICON_STD)
                .text_color(icon_color),
        );

        row = row.child(label);

        // Override badges (pushed right)
        if !override_labels.is_empty() {
            row = row
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(4.0))
                        .children(override_labels.into_iter().map(|label| {
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(3.0))
                                .bg(rgb(t.bg_hover))
                                .text_size(px(11.0))
                                .text_color(rgb(t.text_muted))
                                .child(label)
                        })),
                );
        }

        row = row.on_click(cx.listener(move |this, _, window, cx| {
            this.open_issue_editor(index, window, cx);
        }));

        row
    }

    // ── Settings View ──────────────────────────────────────────────────

    fn render_settings(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        // Agent dropdown state
        let agent_open = self.agent_dropdown_open;
        let current_agent = self.config.agent.clone();
        let agent_bounds = self.agent_button_bounds;

        // Config values for steppers
        let max_iterations = self.config.max_iterations;
        let sleep_secs = self.config.sleep_secs;
        let dangerous = self.config.dangerous;

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.settings_scroll)
                    .p(SPACE_LG)
                    .flex()
                    .flex_col()
                    .max_w(px(500.0))
                    // ── PLANS section ──
                    .child(settings_section_header("Plans", &t))
                    .child(
                        settings_section_container(&t)
                            .child(
                                settings_row("settings-plans-dir", "Plans Directory", &t, false)
                                    .child(
                                        div()
                                            .w(px(220.0))
                                            .bg(rgb(t.bg_secondary))
                                            .border_1()
                                            .border_color(rgb(t.border))
                                            .rounded(px(4.0))
                                            .child(SimpleInput::new(&self.setup_path_input).text_size(px(12.0))),
                                    ),
                            ),
                    )
                    // ── AGENT section ──
                    .child(settings_section_header("Agent", &t))
                    .child(
                        settings_section_container(&t)
                            // Agent dropdown row
                            .child(
                                settings_row("settings-agent", "Agent", &t, true)
                                    .child(
                                        dropdown_button(
                                            "agent-dropdown-btn",
                                            &current_agent,
                                            agent_open,
                                            &t,
                                            {
                                                let entity = cx.entity().downgrade();
                                                move |bounds: Bounds<Pixels>, _, cx: &mut App| {
                                                    if let Some(entity) = entity.upgrade() {
                                                        entity.update(cx, |this, _| {
                                                            this.agent_button_bounds = Some(bounds);
                                                        });
                                                    }
                                                }
                                            },
                                        )
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.agent_dropdown_open = !this.agent_dropdown_open;
                                            cx.notify();
                                        })),
                                    ),
                            )
                            // Model input row
                            .child(
                                settings_row("settings-model", "Model", &t, false)
                                    .child(
                                        div()
                                            .w(px(200.0))
                                            .bg(rgb(t.bg_secondary))
                                            .border_1()
                                            .border_color(rgb(t.border))
                                            .rounded(px(4.0))
                                            .child(SimpleInput::new(&self.model_input).text_size(px(12.0))),
                                    ),
                            ),
                    )
                    // ── EXECUTION section ──
                    .child(settings_section_header("Execution", &t))
                    .child(
                        settings_section_container(&t)
                            // Max Iterations stepper
                            .child(
                                settings_row("settings-max-iter", "Max Iterations", &t, true)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                settings_stepper_button("max-iter-dec", "-", &t)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if this.config.max_iterations > 1 {
                                                            this.config.max_iterations -= 10;
                                                            if this.config.max_iterations == 0 {
                                                                this.config.max_iterations = 1;
                                                            }
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(settings_value_display(max_iterations.to_string(), 50.0, &t))
                                            .child(
                                                settings_stepper_button("max-iter-inc", "+", &t)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.config.max_iterations += 10;
                                                        cx.notify();
                                                    })),
                                            ),
                                    ),
                            )
                            // Sleep stepper
                            .child(
                                settings_row("settings-sleep", "Sleep (seconds)", &t, true)
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(4.0))
                                            .child(
                                                settings_stepper_button("sleep-dec", "-", &t)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        if this.config.sleep_secs > 0 {
                                                            this.config.sleep_secs -= 1;
                                                        }
                                                        cx.notify();
                                                    })),
                                            )
                                            .child(settings_value_display(sleep_secs.to_string(), 40.0, &t))
                                            .child(
                                                settings_stepper_button("sleep-inc", "+", &t)
                                                    .on_click(cx.listener(|this, _, _, cx| {
                                                        this.config.sleep_secs += 1;
                                                        cx.notify();
                                                    })),
                                            ),
                                    ),
                            )
                            // Dangerous toggle
                            .child(
                                settings_row("settings-dangerous", "Dangerous Mode", &t, false)
                                    .child(
                                        settings_toggle_switch("dangerous-toggle", dangerous, &t)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.config.dangerous = !this.config.dangerous;
                                                cx.notify();
                                            })),
                                    ),
                            ),
                    ),
            )
            // Footer
            .child(
                div()
                    .px(SPACE_MD)
                    .py(SPACE_SM)
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .items_center()
                    .gap(SPACE_XS)
                    // Back button (if plans exist)
                    .when(!self.plans.is_empty(), |el| {
                        el.child(
                            toolbar_button("settings-back", "icons/chevron-left.svg", "Back", &t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state = KruhState::PlanPicker;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        toolbar_button_primary("settings-scan", "icons/play.svg", "Scan & Continue", &t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                // Read model from input
                                this.config.model = this.model_input.read(cx).value().to_string();
                                // Read plans_dir from path input
                                let path = this.setup_path_input.read(cx).value().to_string();
                                if !path.is_empty() {
                                    this.plans_dir = path;
                                }
                                this.config.plans_dir = this.plans_dir.clone();

                                // Save config to layout node
                                if let Ok(config_json) = serde_json::to_value(&this.config) {
                                    let project_id = this.project_id.clone();
                                    let layout_path = this.layout_path.clone();
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.with_layout_node(&project_id, &layout_path, cx, |node| {
                                            if let crate::workspace::state::LayoutNode::App { app_config, .. } = node {
                                                *app_config = config_json;
                                                return true;
                                            }
                                            false
                                        });
                                    });
                                }

                                this.start_scan(cx);
                            })),
                    ),
            )
            // Agent dropdown overlay (rendered deferred so it floats above)
            .when(agent_open, |el| {
                if let Some(bounds) = agent_bounds {
                    let t = theme(cx);
                    let current = current_agent.clone();
                    el
                        // Full-screen transparent backdrop to close dropdown on click outside
                        .child(deferred(
                            div()
                                .id("agent-dropdown-backdrop")
                                .absolute()
                                .inset_0()
                                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                    this.agent_dropdown_open = false;
                                    cx.notify();
                                    cx.stop_propagation();
                                }))
                        ).priority(0))
                        .child(dropdown_anchored_below(
                            bounds,
                            dropdown_overlay("agent-dropdown-list", &t)
                                .children(AGENTS.iter().map(|agent| {
                                    let name = agent.name;
                                    let is_selected = name == current.as_str();
                                    dropdown_option(
                                        format!("agent-opt-{name}"),
                                        name,
                                        is_selected,
                                        &t,
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.config.agent = name.to_string();
                                        this.agent_dropdown_open = false;
                                        cx.notify();
                                    }))
                                })),
                        ))
                } else {
                    el
                }
            })
    }

    // ── Editor ────────────────────────────────────────────────────────

    fn render_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_issue = self.editor_target == Some(EditTarget::Issue);

        let file_label: String = match &self.editor_target {
            Some(EditTarget::Status) => "STATUS.md".into(),
            Some(EditTarget::Instructions) => "INSTRUCTIONS.md".into(),
            Some(EditTarget::Issue) => self
                .editor_file_path
                .as_ref()
                .and_then(|p| p.rsplit('/').next())
                .unwrap_or("issue.md")
                .to_string(),
            None => "Editor".into(),
        };

        let editor_el: AnyElement = if let Some(input) = &self.editor_input {
            div()
                .id("kruh-editor")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(SPACE_MD)
                .child(SimpleInput::new(input).text_size(TEXT_MD))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(TEXT_MD)
                        .text_color(rgb(t.text_muted))
                        .child("No file loaded"),
                )
                .into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // File name bar
            .child(
                div()
                    .px(SPACE_MD)
                    .py(SPACE_XS)
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_secondary))
                    .flex()
                    .items_center()
                    .gap(SPACE_MD)
                    .child(
                        svg()
                            .path("icons/file.svg")
                            .size(ICON_SM)
                            .text_color(rgb(t.text_muted)),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(file_label),
                    ),
            )
            // Frontmatter overrides panel (issue files only)
            .when(is_issue, |el| {
                el.child(self.render_editor_frontmatter(cx))
            })
            // Editor area
            .child(editor_el)
            // Footer with icon buttons
            .child(
                div()
                    .px(SPACE_MD)
                    .py(SPACE_SM)
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_XS)
                    .child(
                        toolbar_button("ed-close", "icons/chevron-left.svg", "Close", &t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_editor(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        toolbar_button_primary("ed-save", "icons/save.svg", "Save", &t)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save_editor(cx);
                                cx.notify();
                            })),
                    ),
            )
    }

    /// Render the structured frontmatter overrides panel for INSTRUCTIONS.md editing.
    fn render_editor_frontmatter(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let fm_input = |input: &Option<Entity<SimpleInputState>>| -> AnyElement {
            if let Some(entity) = input {
                div()
                    .w(px(160.0))
                    .bg(rgb(t.bg_secondary))
                    .border_1()
                    .border_color(rgb(t.border))
                    .rounded(px(4.0))
                    .child(SimpleInput::new(entity).text_size(px(12.0)))
                    .into_any_element()
            } else {
                div().into_any_element()
            }
        };

        div()
            .border_b_1()
            .border_color(rgb(t.border))
            .p(SPACE_MD)
            .flex()
            .flex_col()
            .max_w(px(500.0))
            .child(settings_section_header("Overrides", &t))
            .child(
                settings_section_container(&t)
                    .child(
                        settings_row("fm-agent", "Agent", &t, true)
                            .child(fm_input(&self.editor_fm_agent)),
                    )
                    .child(
                        settings_row("fm-model", "Model", &t, true)
                            .child(fm_input(&self.editor_fm_model)),
                    )
                    .child(
                        settings_row("fm-max-iters", "Max Iterations", &t, true)
                            .child(fm_input(&self.editor_fm_max_iters)),
                    )
                    .child(
                        settings_row("fm-sleep", "Sleep (seconds)", &t, true)
                            .child(fm_input(&self.editor_fm_sleep)),
                    )
                    .child(
                        settings_row("fm-dangerous", "Dangerous", &t, false)
                            .child(fm_input(&self.editor_fm_dangerous)),
                    ),
            )
            .child(
                div()
                    .px(px(16.0))
                    .pt(SPACE_XS)
                    .text_size(px(11.0))
                    .text_color(rgb(t.text_muted))
                    .child("Leave empty to use global settings"),
            )
    }

    // ── Progress Bar ────────────────────────────────────────────────────

    fn render_progress_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let total = self.progress.total.max(1) as f32;
        let ratio = self.progress.done as f32 / total;
        let pct = (ratio * 100.0) as usize;

        let bar_color = if ratio < 0.25 {
            rgb(t.error)
        } else if ratio < 0.75 {
            rgb(t.warning)
        } else {
            rgb(t.success)
        };

        div()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .flex()
            .items_center()
            .gap(SPACE_MD)
            .child(
                div()
                    .text_size(TEXT_SM)
                    .child(format!("{}/{}", self.progress.done, self.progress.total)),
            )
            .child(
                div()
                    .flex_1()
                    .h(px(6.0))
                    .rounded(RADIUS_MD)
                    .bg(rgb(t.bg_secondary))
                    .child(
                        div()
                            .h_full()
                            .rounded(RADIUS_MD)
                            .bg(bar_color)
                            .w(relative(ratio)),
                    ),
            )
            .child(div().text_size(TEXT_SM).child(format!("{}%", pct)))
    }

    // ── Iteration Banner ────────────────────────────────────────────────

    fn render_iteration_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let elapsed = self.start_time.map(|t| t.elapsed()).unwrap_or_default();
        let mins = elapsed.as_secs() / 60;
        let secs = elapsed.as_secs() % 60;

        div()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .flex()
            .justify_between()
            .bg(rgb(t.bg_secondary))
            .child(
                div()
                    .text_size(TEXT_MD)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!(
                        "Iteration {}/{}",
                        self.iteration, self.config.max_iterations
                    )),
            )
            .child(
                div()
                    .text_size(TEXT_MD)
                    .text_color(rgb(t.text_muted))
                    .child(format!("{:02}:{:02}", mins, secs)),
            )
    }

    // ── Output Display ──────────────────────────────────────────────────

    fn render_output(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .id("kruh-output")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.output_scroll)
            .px(SPACE_MD)
            .py(SPACE_XS)
            .children(self.output_lines.iter().map(|line| {
                let text_color = if line.is_error {
                    rgb(t.error)
                } else {
                    rgb(t.text_primary)
                };
                div()
                    .text_size(TEXT_SM)
                    .font_family("monospace")
                    .text_color(text_color)
                    .child(strip_ansi(&line.text))
            }))
    }

    // ── Diff Display ────────────────────────────────────────────────────

    fn render_diff(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .border_t_1()
            .border_color(rgb(t.border))
            .when_some(self.diff_stat.as_ref(), |el, stat| {
                el.children(stat.lines().map(|line| {
                    let color = if line.contains('+') && !line.contains('-') {
                        rgb(t.diff_added_fg)
                    } else if line.contains('-') && !line.contains('+') {
                        rgb(t.diff_removed_fg)
                    } else {
                        rgb(t.text_primary)
                    };
                    div()
                        .text_size(TEXT_SM)
                        .font_family("monospace")
                        .text_color(color)
                        .child(line.to_string())
                }))
            })
    }

    // ── Controls ────────────────────────────────────────────────────────

    fn render_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let is_paused = self.paused;
        let is_step = self.step_mode;
        let is_waiting = self.state == KruhState::WaitingForStep;
        let is_completed = self.state == KruhState::Completed;

        let state_label = match self.state {
            KruhState::Running => "Running",
            KruhState::Paused => "Paused",
            KruhState::WaitingForStep => "Step",
            KruhState::Completed => "Done",
            _ => "",
        };

        let state_color = match self.state {
            KruhState::Running => t.success,
            KruhState::Paused => t.warning,
            KruhState::WaitingForStep => t.warning,
            KruhState::Completed => t.text_muted,
            _ => t.text_muted,
        };

        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_XS)
            .px(SPACE_MD)
            .py(SPACE_SM)
            .border_t_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            // Pause/Resume button
            .when(!is_completed, |el| {
                let (icon, label) = if is_paused {
                    ("icons/play.svg", "Resume")
                } else {
                    ("icons/pause.svg", "Pause")
                };
                el.child(
                    toolbar_button("ctrl-pause", icon, label, &t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.paused = !this.paused;
                            this.state = if this.paused {
                                KruhState::Paused
                            } else {
                                KruhState::Running
                            };
                            cx.notify();
                        })),
                )
            })
            // Skip button
            .when(!is_completed, |el| {
                el.child(
                    toolbar_button("ctrl-skip", "icons/skip-forward.svg", "Skip", &t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.skip_requested = true;
                            cx.notify();
                        })),
                )
            })
            // Step mode toggle
            .when(!is_completed, |el| {
                el.child(
                    toolbar_button_toggle("ctrl-step", "icons/step-forward.svg", "Step", is_step, &t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.step_mode = !this.step_mode;
                            cx.notify();
                        })),
                )
            })
            // Continue button (when waiting for step)
            .when(is_waiting, |el| {
                el.child(
                    toolbar_button_primary("ctrl-continue", "icons/play.svg", "Continue", &t)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.step_advance_requested = true;
                            cx.notify();
                        })),
                )
            })
            // Separator before quit
            .when(!is_completed, |el| {
                el.child(toolbar_separator(&t))
            })
            // Quit button
            .child(
                toolbar_button("ctrl-quit", "icons/close.svg", if is_completed { "Close" } else { "Quit" }, &t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.quit_requested = true;
                        cx.notify();
                    })),
            )
            // Spacer
            .child(div().flex_1().min_w(SPACE_SM))
            // Pass/Fail counters
            .child(
                div()
                    .flex()
                    .gap(SPACE_MD)
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .gap(SPACE_XS)
                            .items_center()
                            .child(
                                svg()
                                    .path("icons/check.svg")
                                    .size(ICON_SM)
                                    .text_color(rgb(t.success)),
                            )
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{}", self.pass_count)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(SPACE_XS)
                            .items_center()
                            .child(
                                svg()
                                    .path("icons/close.svg")
                                    .size(ICON_SM)
                                    .text_color(rgb(t.error)),
                            )
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("{}", self.fail_count)),
                            ),
                    )
                    // State label
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(state_color))
                            .child(state_label),
                    ),
            )
    }

    // ── Running View ────────────────────────────────────────────────────

    fn render_running_view(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.render_progress_bar(cx))
            .child(self.render_iteration_banner(cx))
            .child(self.render_output(cx))
            .when(self.diff_stat.is_some(), |el| {
                el.child(self.render_diff(cx))
            })
            .child(self.render_controls(cx))
    }

    // ── Keyboard Handler ────────────────────────────────────────────────

    pub fn handle_key_event(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();

        match self.state {
            KruhState::PlanPicker => match key {
                "up" => {
                    if self.selected_plan_index > 0 {
                        self.selected_plan_index -= 1;
                        cx.notify();
                    }
                }
                "down" => {
                    if self.selected_plan_index + 1 < self.plans.len() {
                        self.selected_plan_index += 1;
                        cx.notify();
                    }
                }
                "enter" => {
                    let idx = self.selected_plan_index;
                    self.select_plan(idx, cx);
                }
                "s" => {
                    self.state = KruhState::Settings;
                    cx.notify();
                }
                _ => {}
            },

            KruhState::TaskBrowser => match key {
                "up" => {
                    if self.selected_issue_index > 0 {
                        self.selected_issue_index -= 1;
                        cx.notify();
                    }
                }
                "down" => {
                    if self.selected_issue_index + 1 < self.issues.len() {
                        self.selected_issue_index += 1;
                        cx.notify();
                    }
                }
                "enter" => {
                    let idx = self.selected_issue_index;
                    self.open_issue_editor(idx, window, cx);
                }
                "r" => {
                    self.start_loop_from_plan(window, cx);
                }
                "backspace" | "escape" => {
                    self.navigate_to_plan_picker(cx);
                }
                "s" => {
                    self.state = KruhState::Settings;
                    cx.notify();
                }
                "e" => {
                    self.open_editor(EditTarget::Status, window, cx);
                }
                "i" => {
                    self.open_editor(EditTarget::Instructions, window, cx);
                }
                _ => {}
            },

            KruhState::Editing => match key {
                "escape" => {
                    self.close_editor(cx);
                }
                "s" if event.keystroke.modifiers.platform => {
                    self.save_editor(cx);
                    cx.notify();
                }
                _ => {}
            },

            KruhState::Settings => match key {
                "escape" | "backspace" => {
                    // Don't navigate back if an input has focus — let it handle keys
                    let input_focused = self
                        .model_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window)
                        || self
                            .setup_path_input
                            .read(cx)
                            .focus_handle(cx)
                            .is_focused(window);
                    if input_focused {
                        return;
                    }
                    if !self.plans.is_empty() {
                        if self.selected_plan.is_some() {
                            self.state = KruhState::TaskBrowser;
                        } else {
                            self.state = KruhState::PlanPicker;
                        }
                        cx.notify();
                    }
                }
                _ => {}
            },

            KruhState::Running | KruhState::Paused | KruhState::WaitingForStep
            | KruhState::Completed => match key {
                "p" => {
                    self.paused = !self.paused;
                    self.state = if self.paused {
                        KruhState::Paused
                    } else {
                        KruhState::Running
                    };
                    cx.notify();
                }
                "s" => {
                    self.skip_requested = true;
                    cx.notify();
                }
                "q" => {
                    self.quit_requested = true;
                    cx.notify();
                }
                "t" => {
                    self.step_mode = !self.step_mode;
                    cx.notify();
                }
                "enter" => {
                    if self.state == KruhState::WaitingForStep {
                        self.step_advance_requested = true;
                        cx.notify();
                    }
                }
                _ => {}
            },

            KruhState::Scanning => {}
        }
    }
}

// ── Toolbar Button Helpers ────────────────────────────────────────────

/// Compact icon button with label for toolbars. Shows icon + text, hover highlight.
fn toolbar_button(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(SPACE_SM)
        .py(SPACE_XS)
        .rounded(RADIUS_STD)
        .flex()
        .items_center()
        .gap(SPACE_XS)
        .text_size(TEXT_SM)
        .text_color(rgb(t.text_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .child(
            svg()
                .path(icon)
                .size(ICON_SM)
                .text_color(rgb(t.text_muted)),
        )
        .child(label.into())
}

/// Primary action toolbar button with accent color.
fn toolbar_button_primary(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &ThemeColors,
) -> Stateful<Div> {
    div()
        .id(id)
        .cursor_pointer()
        .px(SPACE_MD)
        .py(SPACE_XS)
        .rounded(RADIUS_STD)
        .flex()
        .items_center()
        .gap(SPACE_XS)
        .text_size(TEXT_SM)
        .font_weight(FontWeight::SEMIBOLD)
        .bg(rgb(t.button_primary_bg))
        .text_color(rgb(t.button_primary_fg))
        .hover(|s| s.bg(rgb(t.button_primary_hover)))
        .child(
            svg()
                .path(icon)
                .size(ICON_SM)
                .text_color(rgb(t.button_primary_fg)),
        )
        .child(label.into())
}

/// Toggle-style toolbar button that shows active state.
fn toolbar_button_toggle(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
    t: &ThemeColors,
) -> Stateful<Div> {
    let mut btn = div()
        .id(id)
        .cursor_pointer()
        .px(SPACE_SM)
        .py(SPACE_XS)
        .rounded(RADIUS_STD)
        .flex()
        .items_center()
        .gap(SPACE_XS)
        .text_size(TEXT_SM);

    if active {
        btn = btn
            .bg(rgb(t.bg_hover))
            .text_color(rgb(t.text_primary));
    } else {
        btn = btn
            .text_color(rgb(t.text_secondary))
            .hover(|s| s.bg(rgb(t.bg_hover)));
    }

    let icon_color = if active { t.text_primary } else { t.text_muted };

    btn.child(
        svg()
            .path(icon)
            .size(ICON_SM)
            .text_color(rgb(icon_color)),
    )
    .child(label.into())
}

/// Thin vertical separator for toolbars.
fn toolbar_separator(t: &ThemeColors) -> Div {
    div()
        .w(px(1.0))
        .h(px(16.0))
        .mx(SPACE_XS)
        .bg(rgb(t.border))
}

// ── Settings View Component Helpers ──────────────────────────────────

/// Section header (uppercase, muted) matching settings panel style.
fn settings_section_header(title: &str, t: &ThemeColors) -> Div {
    div()
        .px(px(16.0))
        .py(px(8.0))
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(rgb(t.text_muted))
        .child(title.to_uppercase())
}

/// Bordered container for a group of settings rows.
fn settings_section_container(t: &ThemeColors) -> Div {
    div()
        .mx(px(16.0))
        .mb(px(12.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(rgb(t.border))
}

/// Single settings row with label on left and control on right.
fn settings_row(id: impl Into<SharedString>, label: &str, t: &ThemeColors, has_border: bool) -> Stateful<Div> {
    let row = div()
        .id(ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.0))
                .text_color(rgb(t.text_primary))
                .child(label.to_string()),
        );

    if has_border {
        row.border_b_1().border_color(rgb(t.border))
    } else {
        row
    }
}

/// Small +/- stepper button.
fn settings_stepper_button(id: impl Into<SharedString>, label: &str, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(24.0))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .hover(|s| s.bg(rgb(t.bg_hover)))
        .text_size(px(14.0))
        .text_color(rgb(t.text_secondary))
        .child(label.to_string())
}

/// Monospace value display box for steppers.
fn settings_value_display(value: String, width: f32, t: &ThemeColors) -> Div {
    div()
        .w(px(width))
        .h(px(24.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .bg(rgb(t.bg_secondary))
        .text_size(px(13.0))
        .font_family("monospace")
        .text_color(rgb(t.text_primary))
        .child(value)
}

/// Toggle switch (40x22 pill with sliding dot).
fn settings_toggle_switch(id: impl Into<SharedString>, enabled: bool, t: &ThemeColors) -> Stateful<Div> {
    div()
        .id(ElementId::Name(id.into()))
        .cursor_pointer()
        .w(px(40.0))
        .h(px(22.0))
        .rounded(px(11.0))
        .bg(if enabled { rgb(t.border_active) } else { rgb(t.bg_secondary) })
        .flex()
        .items_center()
        .child(
            div()
                .w(px(18.0))
                .h(px(18.0))
                .rounded_full()
                .bg(rgb(t.text_primary))
                .ml(if enabled { px(20.0) } else { px(2.0) }),
        )
}

/// Remove ANSI escape sequences from a string.
fn strip_ansi(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we find the terminating character (letter)
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}
