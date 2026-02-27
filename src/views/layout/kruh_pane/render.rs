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
use super::types::{EditTarget, KruhState, LoopInstance, LoopPhase, LoopState, PlanInfo};
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
            KruhState::LoopOverview => self.render_loop_overview(window, cx).into_any_element(),
        };

        div()
            .flex_1()
            .w_full()
            .min_h_0()
            .overflow_hidden()
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
            || self.state == KruhState::LoopOverview;

        if !has_context {
            return div();
        }

        let mut header = div()
            .flex_shrink_0()
            .flex()
            .items_center()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .gap(SPACE_MD)
            .border_b_1()
            .border_color(rgb(t.border));

        if self.state == KruhState::LoopOverview {
            // Show focused loop's plan name
            if let Some(l) = self.focused_loop() {
                header = header.child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(rgb(t.text_muted))
                        .child(l.plan.name.clone()),
                );
            }
            // Badge: N loops
            let running_count = self.active_loops.iter()
                .filter(|l| l.state != LoopState::Completed)
                .count();
            let total = self.active_loops.len();
            header = header.child(
                badge(&format!("{}/{} loops", running_count, total), &t),
            );
        } else {
            // Breadcrumb: plan name
            if let Some(name) = self.selected_plan.as_ref().map(|p| p.name.clone()) {
                header = header.child(
                    div()
                        .text_size(TEXT_SM)
                        .text_color(rgb(t.text_muted))
                        .child(name),
                );
            }
        }

        header = header
            // Spacer
            .child(div().flex_1())
            // Agent/model label
            .child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(rgb(t.text_muted))
                    .child(format!("{} / {}", self.config.agent, self.config.model)),
            );

        header
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
            .child(with_scrollbar(
                "plan-list",
                &self.plan_scroll,
                div()
                    .p(SPACE_LG)
                    .flex()
                    .flex_col()
                    .gap(SPACE_SM)
                    .children(plan_cards),
                &t,
            ))
            .child(
                div()
                    .flex_shrink_0()
                    .px(SPACE_MD)
                    .py(SPACE_SM)
                    .border_t_1()
                    .border_color(rgb(t.border))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(SPACE_XS)
                    // Show "Running" button when loops are active
                    .when(!self.active_loops.is_empty(), |el| {
                        let running = self.active_loops.iter()
                            .filter(|l| l.state != LoopState::Completed)
                            .count();
                        let label = if running > 0 {
                            format!("Running ({})", running)
                        } else {
                            "Loops".to_string()
                        };
                        el.child(
                            toolbar_button_primary("pp-loops", "icons/play.svg", &label, &t)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state = KruhState::LoopOverview;
                                    cx.notify();
                                })),
                        )
                    })
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
                            )
                            .child(div().w(SPACE_MD))
                            .child(kbd("a", &t))
                            .child(
                                div()
                                    .text_size(TEXT_SM)
                                    .text_color(rgb(t.text_muted))
                                    .child("Run All"),
                            ),
                    )
                    .child(
                        toolbar_button_primary("pp-run-all", "icons/play.svg", "Run All", &t)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.start_all_loops(window, cx);
                            })),
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
            .child(with_scrollbar(
                "issue-list",
                &self.issue_scroll,
                div()
                    .px(SPACE_LG)
                    .py(SPACE_SM)
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(issue_rows),
                &t,
            ))
            // Footer: icon button toolbar
            .child(
                div()
                    .flex_shrink_0()
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
            .child(with_scrollbar(
                "settings-scroll",
                &self.settings_scroll,
                div()
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
                &t,
            ))
            // Footer
            .child(
                div()
                    .flex_shrink_0()
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
            let mut inner_content = div()
                .flex()
                .flex_col()
                .p(SPACE_MD);

            // Frontmatter inside the scroll area so it scrolls with the text
            if is_issue {
                inner_content = inner_content.child(
                    div()
                        .flex_shrink_0()
                        .child(self.render_editor_frontmatter(cx)),
                );
            }

            inner_content = inner_content
                .child(
                    div()
                        .flex_shrink_0()
                        .child(SimpleInput::new(input).text_size(TEXT_MD)),
                );

            with_scrollbar("kruh-editor", &self.editor_scroll, inner_content, &t)
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
                    .flex_shrink_0()
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
            // Editor area
            .child(editor_el)
            // Footer with icon buttons
            .child(
                div()
                    .flex_shrink_0()
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

    // ── Loop Overview ─────────────────────────────────────────────────

    fn render_loop_overview(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx);

        // Build loop cards
        let loop_cards: Vec<_> = self.active_loops.iter().enumerate().map(|(i, l)| {
            self.render_loop_card(i, l, i == self.focused_loop_index, cx).into_any_element()
        }).collect();

        let all_done = self.all_loops_completed();

        // Focused loop detail
        let detail: AnyElement = if let Some(l) = self.focused_loop() {
            self.render_loop_detail(l, cx).into_any_element()
        } else {
            div().into_any_element()
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            // Loop cards strip
            .child(
                div()
                    .id("loop-cards")
                    .max_h(px(160.0))
                    .overflow_y_scroll()
                    .track_scroll(&self.loop_cards_scroll)
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(SPACE_XS)
                            .px(SPACE_MD)
                            .py(SPACE_XS)
                            .children(loop_cards),
                    ),
            )
            // Focused loop detail
            .child(detail)
            // Controls
            .child(self.render_overview_controls(all_done, cx))
    }

    fn render_loop_card(
        &self,
        index: usize,
        l: &LoopInstance,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = theme(cx);
        let total = l.progress.total.max(1) as f32;
        let ratio = l.progress.done as f32 / total;

        let state_color = match l.state {
            LoopState::Running => rgb(t.success),
            LoopState::Paused | LoopState::WaitingForStep => rgb(t.warning),
            LoopState::Completed => rgb(t.text_muted),
        };

        let state_label = match l.state {
            LoopState::Running => "Running",
            LoopState::Paused => "Paused",
            LoopState::WaitingForStep => "Step",
            LoopState::Completed => "Done",
        };

        let mut card = div()
            .id(ElementId::Name(format!("loop-card-{index}").into()))
            .cursor_pointer()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .rounded(RADIUS_STD)
            .border_1()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .min_w(px(140.0));

        if focused {
            card = card
                .border_color(rgb(t.border_active))
                .bg(rgb(t.bg_secondary));
        } else {
            card = card
                .border_color(rgb(t.border))
                .hover(|s| s.bg(rgb(t.bg_hover)));
        }

        card = card.on_click(cx.listener(move |this, _, _, cx| {
            this.focused_loop_index = index;
            cx.notify();
        }));

        // Plan name + state
        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_MD)
                .child(
                    div()
                        .text_size(TEXT_SM)
                        .font_weight(FontWeight::SEMIBOLD)
                        .overflow_x_hidden()
                        .text_ellipsis()
                        .child(l.plan.name.clone()),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(state_color)
                        .child(state_label),
                ),
        );

        // Iteration + pass/fail
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_SM)
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.text_muted))
                        .child(format!("iter {}", l.iteration)),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.success))
                        .child(format!("{}\u{2713}", l.pass_count)),
                )
                .child(
                    div()
                        .text_size(px(10.0))
                        .text_color(rgb(t.error))
                        .child(format!("{}\u{2717}", l.fail_count)),
                ),
        );

        // Mini progress bar
        card = card.child(
            div()
                .h(px(3.0))
                .w_full()
                .rounded(px(2.0))
                .bg(rgb(t.bg_hover))
                .child(
                    div()
                        .h_full()
                        .rounded(px(2.0))
                        .bg(rgb(t.success))
                        .w(relative(ratio)),
                ),
        );

        card
    }

    fn render_loop_detail(&self, l: &LoopInstance, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.render_instance_progress_bar(l, cx))
            .child(self.render_instance_iteration_banner(l, cx))
            .child(self.render_instance_phase_indicator(l, cx))
            .child(self.render_instance_output(l, cx))
            .when(l.diff_stat.is_some(), |el| {
                el.child(self.render_instance_diff(l, cx))
            })
    }

    fn render_overview_controls(&self, all_done: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focused = self.focused_loop();
        let is_paused = focused.map(|l| l.paused).unwrap_or(false);
        let is_step = focused.map(|l| l.step_mode).unwrap_or(false);
        let is_waiting = focused.map(|l| l.state == LoopState::WaitingForStep).unwrap_or(false);
        let is_completed = focused.map(|l| l.state == LoopState::Completed).unwrap_or(false);
        let focused_idx = self.focused_loop_index;

        let multi_loop = self.active_loops.len() > 1;

        let state_label = if all_done {
            "All Done"
        } else {
            match focused.map(|l| &l.state) {
                Some(LoopState::Running) => "Running",
                Some(LoopState::Paused) => "Paused",
                Some(LoopState::WaitingForStep) => "Step",
                Some(LoopState::Completed) => "Done",
                None => "",
            }
        };

        let state_color = if all_done {
            t.text_muted
        } else {
            match focused.map(|l| &l.state) {
                Some(LoopState::Running) => t.success,
                Some(LoopState::Paused) | Some(LoopState::WaitingForStep) => t.warning,
                _ => t.text_muted,
            }
        };

        div()
            .flex_shrink_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_XS)
            .px(SPACE_MD)
            .py(SPACE_SM)
            .border_t_1()
            .border_color(rgb(t.border))
            .bg(rgb(t.bg_secondary))
            // Back to plans list (non-destructive — loops keep running)
            .child(
                toolbar_button("ov-back", "icons/chevron-left.svg", "Back", &t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.selected_plan = None;
                        this.state = KruhState::PlanPicker;
                        cx.notify();
                    })),
            )
            .child(toolbar_separator(&t))
            // Per-loop Pause/Resume
            .when(!is_completed && focused.is_some(), |el| {
                let (icon, label) = if is_paused {
                    ("icons/play.svg", "Resume")
                } else {
                    ("icons/pause.svg", "Pause")
                };
                el.child(
                    toolbar_button("ov-pause", icon, label, &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(l) = this.active_loops.get_mut(focused_idx) {
                                l.paused = !l.paused;
                                l.state = if l.paused {
                                    LoopState::Paused
                                } else {
                                    LoopState::Running
                                };
                            }
                            cx.notify();
                        })),
                )
            })
            // Per-loop Skip
            .when(!is_completed && focused.is_some(), |el| {
                el.child(
                    toolbar_button("ov-skip", "icons/skip-forward.svg", "Skip", &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(l) = this.active_loops.get_mut(focused_idx) {
                                l.skip_requested = true;
                            }
                            cx.notify();
                        })),
                )
            })
            // Per-loop Step toggle
            .when(!is_completed && focused.is_some(), |el| {
                el.child(
                    toolbar_button_toggle("ov-step", "icons/step-forward.svg", "Step", is_step, &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(l) = this.active_loops.get_mut(focused_idx) {
                                l.step_mode = !l.step_mode;
                            }
                            cx.notify();
                        })),
                )
            })
            // Continue button (when waiting for step)
            .when(is_waiting, |el| {
                el.child(
                    toolbar_button_primary("ov-continue", "icons/play.svg", "Continue", &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(l) = this.active_loops.get_mut(focused_idx) {
                                l.step_advance_requested = true;
                            }
                            cx.notify();
                        })),
                )
            })
            // Separator
            .child(toolbar_separator(&t))
            // Quit focused loop (only when multiple loops — otherwise the global button handles it)
            .when(multi_loop && focused.is_some() && !is_completed, |el| {
                el.child(
                    toolbar_button("ov-quit-one", "icons/close.svg", "Quit", &t)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(l) = this.active_loops.get_mut(focused_idx) {
                                l.quit_requested = true;
                            }
                            cx.notify();
                        })),
                )
            })
            // Quit All / Stop / Back — single button that adapts
            .child({
                let (icon, label) = if all_done {
                    ("icons/chevron-left.svg", "Back")
                } else if multi_loop {
                    ("icons/close.svg", "Quit All")
                } else {
                    ("icons/close.svg", "Quit")
                };
                toolbar_button("ov-quit-all", icon, label, &t)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.close_loops(cx);
                    }))
            })
            // Spacer
            .child(div().flex_1().min_w(SPACE_SM))
            // Status
            .child(
                div()
                    .flex()
                    .gap(SPACE_MD)
                    .items_center()
                    .child(
                        div()
                            .text_size(TEXT_SM)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(state_color))
                            .child(state_label),
                    ),
            )
    }

    // ── Instance-level renderers (read from LoopInstance) ───────────────

    fn render_instance_progress_bar(&self, l: &LoopInstance, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let total = l.progress.total.max(1) as f32;
        let ratio = l.progress.done as f32 / total;
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
                    .child(format!("{}/{}", l.progress.done, l.progress.total)),
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

    fn render_instance_iteration_banner(&self, l: &LoopInstance, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let elapsed = l.start_time.map(|t| t.elapsed()).unwrap_or_default();
        let mins = elapsed.as_secs() / 60;
        let secs = elapsed.as_secs() % 60;

        let iter_elapsed = l.iteration_start_time.map(|t| t.elapsed()).unwrap_or_default();
        let iter_mins = iter_elapsed.as_secs() / 60;
        let iter_secs = iter_elapsed.as_secs() % 60;

        div()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .flex()
            .flex_col()
            .gap(px(2.0))
            .bg(rgb(t.bg_secondary))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .child(
                        div()
                            .text_size(TEXT_MD)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!(
                                "Iteration {}/{}",
                                l.iteration, l.config.max_iterations
                            )),
                    )
                    .child(
                        div()
                            .text_size(TEXT_MD)
                            .text_color(rgb(t.text_muted))
                            .child(format!("{:02}:{:02}", mins, secs)),
                    ),
            )
            .when_some(l.current_issue_name.clone(), |el, name| {
                el.child(
                    div()
                        .flex()
                        .justify_between()
                        .child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(rgb(t.text_muted))
                                .child(format!("Working on: {}", name)),
                        )
                        .child(
                            div()
                                .text_size(TEXT_SM)
                                .text_color(rgb(t.text_muted))
                                .child(format!("({}:{:02})", iter_mins, iter_secs)),
                        ),
                )
            })
    }

    fn render_instance_phase_indicator(&self, l: &LoopInstance, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let phase = &l.loop_phase;
        if *phase == LoopPhase::Idle {
            return div();
        }

        let dot_color = match phase {
            LoopPhase::AgentRunning => rgb(t.success),
            LoopPhase::Sleeping(_) | LoopPhase::WaitingForExit => rgb(t.warning),
            _ => rgb(t.term_blue),
        };

        div()
            .px(SPACE_MD)
            .py(px(3.0))
            .flex()
            .items_center()
            .gap(SPACE_SM)
            .child(
                div()
                    .size(px(6.0))
                    .rounded(px(3.0))
                    .bg(dot_color),
            )
            .child(
                div()
                    .text_size(TEXT_SM)
                    .text_color(rgb(t.text_muted))
                    .child(phase.to_string()),
            )
    }

    fn render_instance_output(&self, l: &LoopInstance, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        with_scrollbar(
            "loop-output",
            &l.output_scroll,
            div()
                .px(SPACE_MD)
                .py(SPACE_XS)
                .children(l.output_lines.iter().map(|line| {
                    let is_iteration_marker = line.text.starts_with("--- Iteration ");

                    if is_iteration_marker {
                        div()
                            .mt(SPACE_MD)
                            .pt(SPACE_SM)
                            .border_t_1()
                            .border_color(rgb(t.border))
                            .text_size(TEXT_SM)
                            .font_family("monospace")
                            .text_color(rgb(t.text_muted))
                            .child(strip_ansi(&line.text))
                    } else {
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
                    }
                })),
            &t,
        )
    }

    fn render_instance_diff(&self, l: &LoopInstance, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .px(SPACE_MD)
            .py(SPACE_XS)
            .border_t_1()
            .border_color(rgb(t.border))
            .when_some(l.diff_stat.as_ref(), |el, stat| {
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


    // ── Keyboard Handler ────────────────────────────────────────────────

    pub fn handle_key_event(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();

        match self.state {
            KruhState::LoopOverview => match key {
                "left" | "tab" if event.keystroke.modifiers.shift => {
                    if !self.active_loops.is_empty() {
                        if self.focused_loop_index > 0 {
                            self.focused_loop_index -= 1;
                        } else {
                            self.focused_loop_index = self.active_loops.len() - 1;
                        }
                        cx.notify();
                    }
                }
                "right" | "tab" => {
                    if !self.active_loops.is_empty() {
                        self.focused_loop_index = (self.focused_loop_index + 1) % self.active_loops.len();
                        cx.notify();
                    }
                }
                "p" => {
                    if let Some(l) = self.active_loops.get_mut(self.focused_loop_index) {
                        l.paused = !l.paused;
                        l.state = if l.paused {
                            LoopState::Paused
                        } else {
                            LoopState::Running
                        };
                    }
                    cx.notify();
                }
                "s" => {
                    if let Some(l) = self.active_loops.get_mut(self.focused_loop_index) {
                        l.skip_requested = true;
                    }
                    cx.notify();
                }
                "q" => {
                    if event.keystroke.modifiers.shift {
                        // Shift+Q: quit all
                        self.close_loops(cx);
                    } else {
                        // q: quit focused loop
                        if let Some(l) = self.active_loops.get_mut(self.focused_loop_index) {
                            l.quit_requested = true;
                        }
                        cx.notify();
                    }
                }
                "t" => {
                    if let Some(l) = self.active_loops.get_mut(self.focused_loop_index) {
                        l.step_mode = !l.step_mode;
                    }
                    cx.notify();
                }
                "enter" => {
                    if let Some(l) = self.active_loops.get_mut(self.focused_loop_index) {
                        if l.state == LoopState::WaitingForStep {
                            l.step_advance_requested = true;
                        }
                    }
                    cx.notify();
                }
                "escape" => {
                    if self.all_loops_completed() {
                        self.close_loops(cx);
                    }
                }
                _ => {}
            },

            KruhState::PlanPicker => match key {
                "a" => {
                    self.start_all_loops(window, cx);
                }
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

// ── Scrollbar Helpers ────────────────────────────────────────────────

/// Wrap a scroll view with a thin scrollbar thumb overlay.
/// Returns a flex_1 div containing the scroll container (absolute, fills parent)
/// and a canvas-painted scrollbar thumb.
fn with_scrollbar(
    scroll_id: &str,
    scroll: &ScrollHandle,
    content: impl IntoElement,
    t: &ThemeColors,
) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .relative()
        .child(
            div()
                .id(SharedString::from(scroll_id.to_string()))
                .absolute()
                .inset_0()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(content),
        )
        .child(scrollbar_thumb(scroll, t))
}

/// Render a thin scrollbar thumb overlay using canvas painting.
/// Absolute-positioned; place inside a `relative()` container.
fn scrollbar_thumb(scroll: &ScrollHandle, t: &ThemeColors) -> impl IntoElement {
    let scroll = scroll.clone();
    let color = rgb(t.scrollbar);

    canvas(
        |_, _, _| {},
        move |bounds, _, window, _cx| {
            let max_y = f32::from(scroll.max_offset().height);
            if max_y < 1.0 {
                return;
            }
            let vh = f32::from(bounds.size.height);
            if vh < 1.0 {
                return;
            }
            let oy = -f32::from(scroll.offset().y);
            let ch = vh + max_y;

            let th = (vh / ch * vh).max(24.0);
            let st = vh - th;
            let ratio = (oy / max_y).clamp(0.0, 1.0);
            let ty = ratio * st;

            window.paint_quad(
                fill(
                    Bounds {
                        origin: point(
                            bounds.origin.x + bounds.size.width - px(5.0),
                            bounds.origin.y + px(ty),
                        ),
                        size: size(px(3.0), px(th)),
                    },
                    color,
                )
                .corner_radii(px(1.5)),
            );
        },
    )
    .absolute()
    .top_0()
    .bottom_0()
    .right_0()
    .w(px(8.0))
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
