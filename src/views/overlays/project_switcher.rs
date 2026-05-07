//! Project switcher overlay for quick project navigation.
//!
//! Provides keyboard-driven project switching with:
//! - Enter: Focus the selected project
//! - Space: Toggle project overview visibility
//! - Type to filter projects

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::ui::tokens::{ui_text, ui_text_ms};
use crate::views::components::list_overlay::FilterResult;
use crate::views::components::{
    badge, handle_list_overlay_key, keyboard_hints_footer, modal_backdrop, modal_content,
    modal_header, search_input_area, ListOverlayAction, ListOverlayConfig, ListOverlayState,
};
use crate::workspace::state::{ProjectData, Workspace};
use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use okena_ui::empty_state::empty_state;
use okena_ui::selectable_list::selectable_list_item;
use std::collections::HashSet;

/// Events emitted by the ProjectSwitcher overlay.
#[derive(Clone)]
pub enum ProjectSwitcherEvent {
    /// Close the overlay
    Close,
    /// Focus a specific project (makes it the only visible one)
    FocusProject(String),
    /// Toggle visibility of a project
    ToggleVisibility(String),
}

impl EventEmitter<ProjectSwitcherEvent> for ProjectSwitcher {}

/// Project switcher overlay for quick project navigation.
pub struct ProjectSwitcher {
    focus_handle: FocusHandle,
    state: ListOverlayState<ProjectData>,
    /// Snapshot of `main_window.hidden_project_ids` taken at construction
    /// time. The visibility eye-icon derives from membership here.
    hidden_project_ids: HashSet<String>,
}

impl ProjectSwitcher {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        // Get all projects from workspace, sorted by recency, with effective colors resolved
        let ws = workspace.read(cx);
        let projects: Vec<ProjectData> = ws
            .projects_by_recency()
            .into_iter()
            .cloned()
            .map(|mut p| {
                p.folder_color = ws.effective_folder_color(&p);
                p
            })
            .collect();
        let hidden_project_ids = ws.data().main_window.hidden_project_ids.clone();

        let config = ListOverlayConfig::new("Switch Project")
            .subtitle("Type to search, Enter to focus, Space to toggle visibility")
            .searchable("Type to filter projects...")
            .size(500.0, 500.0)
            .empty_message("No projects found")
            .keyboard_hints(vec![
                ("Enter", "focus"),
                ("Space", "toggle visibility"),
                ("Esc", "close"),
            ])
            .key_context("ProjectSwitcher");

        let state = ListOverlayState::new(projects, config, cx);
        let focus_handle = state.focus_handle.clone();

        Self {
            focus_handle,
            state,
            hidden_project_ids,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(ProjectSwitcherEvent::Close);
    }

    fn focus_selected(&self, cx: &mut Context<Self>) {
        if let Some(project) = self.state.selected_item() {
            cx.emit(ProjectSwitcherEvent::FocusProject(project.id.clone()));
        }
    }

    fn toggle_visibility_selected(&self, cx: &mut Context<Self>) {
        if let Some(project) = self.state.selected_item() {
            cx.emit(ProjectSwitcherEvent::ToggleVisibility(project.id.clone()));
        }
    }

    fn filter_projects(&mut self) {
        let filtered = ranked_project_filter(&self.state.items, &self.state.search_query);
        self.state.set_filtered(filtered);
    }

    fn render_project_row(
        &self,
        display_index: usize,
        project_index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let t = theme(cx);
        let project = &self.state.items[project_index];
        let is_selected = display_index == self.state.selected_index;
        let name = project.name.clone();
        let path = project.path.clone();
        let show_in_overview = project_visibility(project, &self.hidden_project_ids);
        let is_worktree = project.worktree_info.is_some();
        let folder_color = t.get_folder_color(project.folder_color);
        let branch = crate::git::get_git_status(std::path::Path::new(&project.path))
            .and_then(|s| s.branch);

        selectable_list_item(
            ElementId::Name(format!("project-{}", display_index).into()),
            is_selected,
            &t,
        )
        .gap(px(12.0))
        .py(px(10.0))
        .border_b_1()
        .border_color(rgb(t.border))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _window, cx| {
                this.state.selected_index = display_index;
                this.focus_selected(cx);
            }),
        )
        .child(
            // Folder icon with project color
            div()
                .w(px(20.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path("icons/folder.svg")
                        .size(px(16.0))
                        .text_color(rgb(folder_color)),
                ),
        )
        .child(
            // Project info
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .overflow_hidden()
                .child(
                    h_flex()
                        .gap(px(8.0))
                        .child(
                            div()
                                .text_size(ui_text(13.0, cx))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(t.text_primary))
                                .child(name),
                        )
                        .when(is_worktree, |d| d.child(badge("worktree", &t)))
                        .when_some(branch, |d, b| {
                            d.child(
                                h_flex()
                                    .gap(px(4.0))
                                    .items_center()
                                    .px(px(6.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(rgb(t.bg_secondary))
                                    .child(
                                        svg()
                                            .path("icons/git-branch.svg")
                                            .size(px(10.0))
                                            .text_color(rgb(t.term_green)),
                                    )
                                    .child(
                                        div()
                                            .text_size(ui_text_ms(cx))
                                            .text_color(rgb(t.text_secondary))
                                            .child(b),
                                    ),
                            )
                        }),
                )
                .child(
                    div()
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_muted))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(path),
                ),
        )
        .child(
            // Visibility indicator
            div()
                .w(px(20.0))
                .h(px(20.0))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(if show_in_overview {
                            "icons/eye.svg"
                        } else {
                            "icons/eye-off.svg"
                        })
                        .size(px(14.0))
                        .text_color(if show_in_overview {
                            rgb(t.text_secondary)
                        } else {
                            rgb(t.text_muted)
                        }),
                ),
        )
    }
}

fn ranked_project_filter(items: &[ProjectData], query: &str) -> Vec<FilterResult> {
    if query.is_empty() {
        return (0..items.len()).map(FilterResult::new).collect();
    }

    let query_lower = query.to_lowercase();
    let mut scored: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, project)| {
            project_match_score(project, &query_lower).map(|score| (index, score))
        })
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored
        .into_iter()
        .map(|(index, _)| FilterResult::new(index))
        .collect()
}

/// Pure visibility projection for the eye-icon column. A project is
/// "shown in overview" iff it is absent from the per-window hidden set.
fn project_visibility(project: &ProjectData, hidden_project_ids: &HashSet<String>) -> bool {
    !hidden_project_ids.contains(&project.id)
}

fn project_match_score(project: &ProjectData, query: &str) -> Option<i32> {
    let name = project.name.to_lowercase();
    let path = project.path.to_lowercase();
    let base_dir = std::path::Path::new(&project.path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_lowercase();

    let primary_score = [
        text_match_score(&name, query, 700, 420, 220),
        if base_dir == name {
            None
        } else {
            text_match_score(&base_dir, query, 650, 380, 200)
        },
    ]
    .into_iter()
    .flatten()
    .max()
    .unwrap_or(0);

    let best_segment_score = path
        .split(['/', '\\'])
        .filter(|segment| !segment.is_empty())
        .enumerate()
        .filter_map(|(depth, segment)| {
            text_match_score(segment, query, 240, 140, 70).map(|score| {
                let nested_bonus = ((depth as i32 + 1) * 18).min(90);
                let leaf_bonus = if segment == base_dir { 40 } else { 0 };
                score + nested_bonus + leaf_bonus
            })
        })
        .max()
        .unwrap_or(0);

    let path_score = if path.contains(query) {
        let tail_bias = path
            .rfind(query)
            .map(|index| ((index as i32) * 40) / path.len().max(1) as i32)
            .unwrap_or(0);
        30 + tail_bias
    } else {
        0
    };

    let total_score = primary_score + best_segment_score + path_score;
    (total_score > 0).then_some(total_score)
}

fn text_match_score(
    text: &str,
    query: &str,
    exact_bonus: i32,
    prefix_bonus: i32,
    contains_bonus: i32,
) -> Option<i32> {
    if !text.contains(query) {
        return None;
    }

    let closeness_bonus = (24 - text.len().saturating_sub(query.len()) as i32).max(0);
    let score = if text == query {
        exact_bonus
    } else if text.starts_with(query) {
        prefix_bonus
    } else {
        contains_bonus
    } + closeness_bonus;

    Some(score)
}

impl Render for ProjectSwitcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        let search_query = self.state.search_query.clone();
        let config_width = self.state.config.width;
        let config_max_height = self.state.config.max_height;
        let config_title = self.state.config.title.clone();
        let config_subtitle = self.state.config.subtitle.clone();
        let search_placeholder = self
            .state
            .config
            .search_placeholder
            .clone()
            .unwrap_or_default();
        let empty_message = self.state.config.empty_message.clone();

        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }

        modal_backdrop("project-switcher-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("ProjectSwitcher")
            .items_start()
            .pt(px(80.0))
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| {
                this.close(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                match handle_list_overlay_key(&mut this.state, event, &[("space", "toggle")]) {
                    ListOverlayAction::Close => this.close(cx),
                    ListOverlayAction::SelectPrev | ListOverlayAction::SelectNext => {
                        this.state.scroll_to_selected();
                        cx.notify();
                    }
                    ListOverlayAction::Confirm => this.focus_selected(cx),
                    ListOverlayAction::QueryChanged => {
                        this.filter_projects();
                        cx.notify();
                    }
                    ListOverlayAction::Custom(action) if action == "toggle" => {
                        this.toggle_visibility_selected(cx);
                    }
                    _ => {}
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("project-switcher-modal", &t)
                    .w(px(config_width))
                    .max_h(px(config_max_height))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(modal_header(
                        config_title,
                        config_subtitle,
                        &t,
                        cx,
                        cx.listener(|this, _, _window, cx| this.close(cx)),
                    ))
                    .child(search_input_area(&search_query, &search_placeholder, &t))
                    .child(
                        // Project list
                        div()
                            .id("project-list")
                            .flex_1()
                            .overflow_y_scroll()
                            .track_scroll(&self.state.scroll_handle)
                            .children(self.state.filtered.iter().enumerate().map(
                                |(display_idx, filter_result)| {
                                    self.render_project_row(display_idx, filter_result.index, cx)
                                },
                            ))
                            .when(self.state.is_empty(), |d| {
                                d.child(empty_state(empty_message.clone(), &t, cx))
                            }),
                    )
                    .child(keyboard_hints_footer(
                        &[
                            ("Enter", "focus"),
                            ("Space", "toggle visibility"),
                            ("Esc", "close"),
                        ],
                        &t,
                    )),
            )
    }
}

impl_focusable!(ProjectSwitcher);

#[cfg(test)]
mod tests {
    use super::{project_match_score, project_visibility, ranked_project_filter};
    use crate::terminal::shell_config::ShellType;
    use crate::theme::FolderColor;
    use crate::workspace::settings::HooksConfig;
    use crate::workspace::state::{HookTerminalEntry, LayoutNode, ProjectData, WorktreeMetadata};
    use std::collections::{HashMap, HashSet};

    fn make_project(name: &str, path: &str) -> ProjectData {
        ProjectData {
            id: name.to_string(),
            name: name.to_string(),
            path: path.to_string(),
            layout: None::<LayoutNode>,
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None::<WorktreeMetadata>,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            is_remote: false,
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None::<ShellType>,
            hook_terminals: HashMap::<String, HookTerminalEntry>::new(),
        }
    }

    #[test]
    fn prefers_project_name_over_generic_path_match() {
        let target = make_project("roj", "/home/matej21/projects/oss/roj");
        let generic = make_project("alpha", "/home/matej21/projects/oss/projects-alpha");

        let target_score = project_match_score(&target, "roj").unwrap();
        let generic_score = project_match_score(&generic, "roj").unwrap();

        assert!(target_score > generic_score);
    }

    #[test]
    fn prefers_more_nested_segment_matches() {
        let nested = make_project("alpha", "/home/matej21/projects/oss/roj");
        let shallow = make_project("alpha", "/roj/worktrees/demo");

        let nested_score = project_match_score(&nested, "roj").unwrap();
        let shallow_score = project_match_score(&shallow, "roj").unwrap();

        assert!(nested_score > shallow_score);
    }

    /// Regression: visibility indicator must derive from the per-window
    /// hidden set. With the legacy `ProjectData.show_in_overview` field
    /// removed entirely, this test pins the post-deletion contract.
    #[test]
    fn project_visibility_reads_from_hidden_set() {
        let project = make_project("p1", "/p1");
        let hidden: HashSet<String> = ["p1".to_string()].into_iter().collect();
        assert!(
            !project_visibility(&project, &hidden),
            "membership in hidden set must read as not-visible",
        );

        let other = make_project("p2", "/p2");
        assert!(
            project_visibility(&other, &hidden),
            "absent from hidden set must read as visible",
        );
    }

    #[test]
    fn ranked_filter_sorts_best_match_first() {
        let items = vec![
            make_project("alpha", "/home/matej21/projects/oss/projects-alpha"),
            make_project("roj", "/home/matej21/projects/oss/roj"),
            make_project("beta", "/home/matej21/projects/oss/other"),
        ];

        let filtered = ranked_project_filter(&items, "roj");

        assert_eq!(filtered.len(), 3);
        assert_eq!(filtered[0].index, 1);
        assert_eq!(filtered[1].index, 0);
    }
}
