//! Status board — a kanban over a project and its worktrees (or over every
//! project). One column per board column from settings plus "No status"; a
//! card is a project: its name, colour, how many terminals it holds and the
//! most urgent agent status among them. Click a card to focus that project;
//! drag it onto a column to set its status. The status is daemon state, so
//! the drop dispatches `SetProjectStatus` and the board follows the next
//! snapshot — no local write (see `okena-app/src/CLAUDE.md`).

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use crate::views::components::{modal_backdrop, modal_content};
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{ProjectData, Workspace};
use gpui::prelude::*;
use gpui::*;
use okena_core::agent_status::{AgentLifecycle, AgentStatus};
use okena_core::theme::FolderColor;
use okena_terminal::TerminalsRegistry;
use okena_ui::button::button;
use okena_ui::icon_button::icon_button;
use okena_workspace::status_board::{board_family, board_model};

/// Which projects the board shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardScope {
    /// A parent project and its worktrees (the id is the parent's).
    Family(String),
    All,
}

pub enum StatusBoardEvent {
    Close,
    /// A card was dropped on a column (`None` = the "No status" column).
    SetStatus {
        project_id: String,
        status: Option<String>,
    },
}

impl okena_ui::overlay::CloseEvent for StatusBoardEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

/// Drag payload for a card.
#[derive(Clone)]
struct BoardCardDrag {
    project_id: String,
    name: String,
}

struct BoardCardDragView {
    name: String,
}

impl Render for BoardCardDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .px(px(10.0))
            .py(px(6.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border_active))
            .rounded(px(6.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_primary))
            .child(self.name.clone())
    }
}

/// One card, resolved from workspace + terminal runtime for rendering.
struct CardView {
    project_id: String,
    name: String,
    /// The parent's name for a worktree in the all-projects scope.
    parent_name: Option<String>,
    folder_color: FolderColor,
    is_worktree: bool,
    terminal_count: usize,
    agent: Option<AgentStatus>,
}

struct ColumnView {
    id: Option<String>,
    label: String,
    color: FolderColor,
    cards: Vec<CardView>,
}

pub struct StatusBoard {
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    terminals: TerminalsRegistry,
    /// The family root the board was opened for; `None` when opened globally.
    /// Keeps the family toggle available after switching to All.
    opened_for: Option<String>,
    scope: BoardScope,
    focus_handle: FocusHandle,
}

impl EventEmitter<StatusBoardEvent> for StatusBoard {}

/// The most urgent of the agent statuses a project's terminals report:
/// blocked beats done beats working beats idle (the AGENTS list order).
fn most_urgent(statuses: impl Iterator<Item = AgentStatus>) -> Option<AgentStatus> {
    fn rank(lifecycle: &AgentLifecycle) -> u8 {
        match lifecycle {
            AgentLifecycle::Blocked => 0,
            AgentLifecycle::Done => 1,
            AgentLifecycle::Working => 2,
            AgentLifecycle::Idle => 3,
        }
    }
    statuses.min_by_key(|s| rank(&s.lifecycle))
}

impl StatusBoard {
    pub fn new(
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        terminals: TerminalsRegistry,
        project_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Repaint on every workspace change: a drop lands as a daemon
        // snapshot, and the sidebar / menu may change statuses meanwhile.
        cx.observe(&workspace, |_this, _ws, cx| cx.notify())
            .detach();
        // A worktree opens its parent's board.
        let opened_for = project_id.and_then(|id| {
            let ws = workspace.read(cx);
            board_family(&ws.data().projects, &id).map(|(root, _)| root)
        });
        let scope = match &opened_for {
            Some(root) => BoardScope::Family(root.clone()),
            None => BoardScope::All,
        };
        Self {
            workspace,
            focus_manager,
            terminals,
            opened_for,
            scope,
            focus_handle: cx.focus_handle(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(StatusBoardEvent::Close);
    }

    fn focus_project(&mut self, project_id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let project_id = project_id.to_string();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_project(fm, Some(project_id), cx);
            });
            cx.notify();
        });
        self.close(cx);
    }

    /// Columns and cards for the current scope, fully resolved so rendering
    /// holds no workspace borrow and no terminal-registry lock.
    fn columns(&self, cx: &App) -> Vec<ColumnView> {
        let ws = self.workspace.read(cx);
        let columns = okena_ui::board::board_columns(cx);
        let all = &ws.data().projects;
        let projects: Vec<&ProjectData> = match &self.scope {
            BoardScope::Family(root) => board_family(all, root)
                .map(|(_, family)| family)
                .unwrap_or_default(),
            BoardScope::All => all.iter().collect(),
        };
        let registry = self.terminals.lock();
        board_model(&projects, &columns)
            .into_iter()
            .map(|column| ColumnView {
                id: column.column.as_ref().map(|c| c.id.clone()),
                label: column.label().to_string(),
                color: column.column.as_ref().map(|c| c.color).unwrap_or_default(),
                cards: column
                    .cards
                    .into_iter()
                    .filter_map(|card| {
                        let project = ws.project(&card.project_id)?;
                        let terminal_ids = project
                            .layout
                            .as_ref()
                            .map(|l| l.collect_terminal_ids())
                            .unwrap_or_default();
                        let agent = most_urgent(
                            terminal_ids
                                .iter()
                                .filter_map(|tid| registry.get(tid))
                                .filter_map(|t| t.agent_status()),
                        );
                        let parent_name = project
                            .worktree_info
                            .as_ref()
                            .and_then(|wt| ws.project(&wt.parent_project_id))
                            .map(|p| p.name.clone());
                        Some(CardView {
                            name: project.name.clone(),
                            parent_name,
                            folder_color: ws.effective_folder_color(project),
                            is_worktree: project.worktree_info.is_some(),
                            terminal_count: terminal_ids.len(),
                            agent,
                            project_id: card.project_id,
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    fn render_scope_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(root_id) = self.opened_for.clone() else {
            return div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child("All projects")
                .into_any_element();
        };
        let root_name = self
            .workspace
            .read(cx)
            .project(&root_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "This project".to_string());
        let family_selected = matches!(self.scope, BoardScope::Family(_));
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                button("status-board-scope-family", root_name, &t)
                    .when(family_selected, |b| b.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.scope = BoardScope::Family(root_id.clone());
                        cx.notify();
                    })),
            )
            .child(
                button("status-board-scope-all", "All projects", &t)
                    .when(!family_selected, |b| b.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.scope = BoardScope::All;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_card(&self, card: CardView, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let drag = BoardCardDrag {
            project_id: card.project_id.clone(),
            name: card.name.clone(),
        };
        let focus_id = card.project_id.clone();
        let agent_line = card.agent.as_ref().map(|agent| {
            let color = match agent.lifecycle {
                AgentLifecycle::Working => t.term_yellow,
                AgentLifecycle::Blocked => t.term_red,
                AgentLifecycle::Done => t.term_green,
                AgentLifecycle::Idle => t.text_muted,
            };
            let text = agent
                .custom
                .clone()
                .unwrap_or_else(|| format!("{:?}", agent.lifecycle).to_lowercase());
            div()
                .flex()
                .items_center()
                .gap(px(5.0))
                .min_w_0()
                .child(
                    div()
                        .size(px(7.0))
                        .rounded_full()
                        .bg(rgb(color))
                        .flex_shrink_0(),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(text),
                )
        });
        let subtitle = match (&card.parent_name, card.terminal_count) {
            (Some(parent), n) => {
                format!("{parent} · {n} terminal{}", if n == 1 { "" } else { "s" })
            }
            (None, n) if card.is_worktree => {
                format!("{n} terminal{}", if n == 1 { "" } else { "s" })
            }
            (None, n) => format!("main · {n} terminal{}", if n == 1 { "" } else { "s" }),
        };

        div()
            .id(ElementId::Name(
                format!("board-card-{}", card.project_id).into(),
            ))
            .flex()
            .flex_col()
            .gap(px(3.0))
            .px(px(10.0))
            .py(px(7.0))
            .rounded(px(5.0))
            .bg(rgb(t.bg_primary))
            .border_1()
            .border_color(rgb(t.border))
            .cursor_pointer()
            .hover(|s| s.border_color(rgb(t.border_active)))
            .on_drag(drag, move |drag, _position, _window, cx| {
                cx.new(|_| BoardCardDragView {
                    name: drag.name.clone(),
                })
            })
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.focus_project(&focus_id, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .min_w_0()
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded_full()
                            .bg(rgb(t.get_folder_color(card.folder_color)))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .child(card.name),
                    ),
            )
            .child(
                div()
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(subtitle),
            )
            .children(agent_line)
            .into_any_element()
    }

    fn render_column(&self, column: ColumnView, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let slug = column.id.clone().unwrap_or_else(|| "none".to_string());
        let drop_status = column.id.clone();
        let count = column.cards.len();
        let accent = match column.color {
            FolderColor::Default => None,
            color => Some(t.get_folder_color(color)),
        };
        let cards: Vec<AnyElement> = column
            .cards
            .into_iter()
            .map(|card| self.render_card(card, cx))
            .collect();

        div()
            .id(ElementId::Name(format!("board-column-{slug}").into()))
            .w(px(240.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h_0()
            .rounded(px(6.0))
            .bg(rgb(t.bg_secondary))
            .border_1()
            .border_color(rgb(t.border))
            .drag_over::<BoardCardDrag>(move |style, _, _, _| {
                style.border_color(rgb(t.border_active))
            })
            .on_drop(
                cx.listener(move |_this, drag: &BoardCardDrag, _window, cx| {
                    cx.emit(StatusBoardEvent::SetStatus {
                        project_id: drag.project_id.clone(),
                        status: drop_status.clone(),
                    });
                }),
            )
            .child(
                div()
                    .h(px(30.0))
                    .px(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .border_b_1()
                    .border_color(rgb(t.border))
                    .children(accent.map(|hex| {
                        div()
                            .size(px(8.0))
                            .rounded_full()
                            .bg(rgb(hex))
                            .flex_shrink_0()
                    }))
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(accent.unwrap_or(t.text_secondary)))
                            .child(column.label.to_uppercase()),
                    )
                    .child(
                        div()
                            .text_size(ui_text_sm(cx))
                            .text_color(rgb(t.text_muted))
                            .child(count.to_string()),
                    ),
            )
            .child(
                div()
                    .id(ElementId::Name(format!("board-column-cards-{slug}").into()))
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .children(cards)
                    // Room to drop below the last card.
                    .child(div().min_h(px(24.0)).flex_1()),
            )
            .into_any_element()
    }
}

impl Render for StatusBoard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }
        let columns = self.columns(cx);
        let column_elements: Vec<AnyElement> = columns
            .into_iter()
            .map(|column| self.render_column(column, cx))
            .collect();

        modal_backdrop("status-board-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("StatusBoard")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("status-board-modal", &t)
                    .w(relative(0.9))
                    .max_w(px(1280.0))
                    .h(relative(0.8))
                    .flex()
                    .flex_col()
                    .on_mouse_down(MouseButton::Left, |_, _window, cx| cx.stop_propagation())
                    .child(
                        div()
                            .px(px(16.0))
                            .py(px(10.0))
                            .flex()
                            .items_center()
                            .gap(px(12.0))
                            .border_b_1()
                            .border_color(rgb(t.border))
                            .child(
                                div()
                                    .text_size(ui_text_xl(cx))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(t.text_primary))
                                    .child("Status board"),
                            )
                            .child(self.render_scope_switch(cx))
                            .child(div().flex_1())
                            .child(
                                icon_button("status-board-close", "icons/close.svg", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| this.close(cx))),
                            ),
                    )
                    .child(
                        div()
                            .id("status-board-columns")
                            .flex_1()
                            .min_h_0()
                            .overflow_x_scroll()
                            .p(px(12.0))
                            .flex()
                            .gap(px(10.0))
                            .children(column_elements),
                    ),
            )
    }
}

impl_focusable!(StatusBoard);

#[cfg(test)]
mod tests {
    use super::most_urgent;
    use okena_core::agent_status::{AgentLifecycle, AgentStatus};

    #[test]
    fn most_urgent_prefers_blocked_then_done_then_working() {
        let s = AgentStatus::new;
        let pick = most_urgent(
            [
                s(AgentLifecycle::Idle),
                s(AgentLifecycle::Working),
                s(AgentLifecycle::Done),
            ]
            .into_iter(),
        )
        .unwrap();
        assert_eq!(pick.lifecycle, AgentLifecycle::Done);
        let pick =
            most_urgent([s(AgentLifecycle::Working), s(AgentLifecycle::Blocked)].into_iter())
                .unwrap();
        assert_eq!(pick.lifecycle, AgentLifecycle::Blocked);
        assert!(most_urgent(std::iter::empty()).is_none());
    }
}
