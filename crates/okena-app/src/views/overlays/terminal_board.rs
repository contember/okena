//! Terminal status board — a kanban over a project's (or every project's)
//! terminals. One column per board column from settings plus "No status";
//! a card is a terminal: its name, its project (in the all-projects scope)
//! and the agent status it reports, if any. Click a card to jump to the
//! pane; drag it onto a column to set its status. The status is daemon
//! state, so the drop dispatches `SetTerminalStatus` and the board follows
//! the next snapshot — no local write (see `okena-app/src/CLAUDE.md`).

use crate::keybindings::Cancel;
use crate::theme::theme;
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm, ui_text_xl};
use crate::views::components::{modal_backdrop, modal_content};
use crate::workspace::focus::FocusManager;
use crate::workspace::state::{WindowId, Workspace};
use gpui::prelude::*;
use gpui::*;
use okena_core::agent_status::{AgentLifecycle, AgentStatus};
use okena_core::theme::FolderColor;
use okena_terminal::TerminalsRegistry;
use okena_ui::button::button;
use okena_ui::icon_button::icon_button;
use okena_workspace::terminal_board::board_model;

/// Which terminals the board shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardScope {
    Project(String),
    All,
}

pub enum TerminalBoardEvent {
    Close,
    /// A card was dropped on a column (`None` = the "No status" column).
    SetStatus {
        project_id: String,
        terminal_id: String,
        status: Option<String>,
    },
}

impl okena_ui::overlay::CloseEvent for TerminalBoardEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}

/// Drag payload for a card.
#[derive(Clone)]
struct BoardCardDrag {
    project_id: String,
    terminal_id: String,
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
    terminal_id: String,
    name: String,
    project_name: String,
    folder_color: FolderColor,
    agent: Option<AgentStatus>,
}

struct ColumnView {
    id: Option<String>,
    label: String,
    cards: Vec<CardView>,
}

pub struct TerminalBoard {
    workspace: Entity<Workspace>,
    focus_manager: Entity<FocusManager>,
    window_id: WindowId,
    terminals: TerminalsRegistry,
    /// The project the board was opened for; `None` when opened globally.
    /// Keeps the "This project" toggle available after switching to All.
    opened_for: Option<String>,
    scope: BoardScope,
    focus_handle: FocusHandle,
}

impl EventEmitter<TerminalBoardEvent> for TerminalBoard {}

impl TerminalBoard {
    pub fn new(
        workspace: Entity<Workspace>,
        focus_manager: Entity<FocusManager>,
        window_id: WindowId,
        terminals: TerminalsRegistry,
        project_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Repaint on every workspace change: a drop lands as a daemon
        // snapshot, and the sidebar / menu may change statuses meanwhile.
        cx.observe(&workspace, |_this, _ws, cx| cx.notify())
            .detach();
        let scope = match &project_id {
            Some(id) => BoardScope::Project(id.clone()),
            None => BoardScope::All,
        };
        Self {
            workspace,
            focus_manager,
            window_id,
            terminals,
            opened_for: project_id,
            scope,
            focus_handle: cx.focus_handle(),
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(TerminalBoardEvent::Close);
    }

    fn jump_to(&mut self, project_id: &str, terminal_id: &str, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let window_id = self.window_id;
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.focus_terminal_by_id(fm, window_id, project_id, terminal_id, cx);
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
        let projects: Vec<&crate::workspace::state::ProjectData> = ws
            .data()
            .projects
            .iter()
            .filter(|p| match &self.scope {
                BoardScope::Project(id) => &p.id == id,
                BoardScope::All => true,
            })
            .collect();
        let registry = self.terminals.lock();
        board_model(&projects, &columns)
            .into_iter()
            .map(|column| ColumnView {
                id: column.column.as_ref().map(|c| c.id.clone()),
                label: column.label().to_string(),
                cards: column
                    .cards
                    .into_iter()
                    .filter_map(|card| {
                        let project = ws.project(&card.project_id)?;
                        let terminal = registry.get(&card.terminal_id);
                        let name = project.terminal_display_name(
                            &card.terminal_id,
                            terminal.and_then(|t| t.title()),
                        );
                        Some(CardView {
                            name,
                            project_name: project.name.clone(),
                            folder_color: ws.effective_folder_color(project),
                            agent: terminal.and_then(|t| t.agent_status()),
                            project_id: card.project_id,
                            terminal_id: card.terminal_id,
                        })
                    })
                    .collect(),
            })
            .collect()
    }

    fn render_scope_switch(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let Some(project_id) = self.opened_for.clone() else {
            return div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child("All projects")
                .into_any_element();
        };
        let project_name = self
            .workspace
            .read(cx)
            .project(&project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "This project".to_string());
        let this_selected = matches!(self.scope, BoardScope::Project(_));
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                button("terminal-board-scope-project", project_name, &t)
                    .when(this_selected, |b| b.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.scope = BoardScope::Project(project_id.clone());
                        cx.notify();
                    })),
            )
            .child(
                button("terminal-board-scope-all", "All projects", &t)
                    .when(!this_selected, |b| b.bg(rgb(t.bg_hover)))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.scope = BoardScope::All;
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    fn render_card(
        &self,
        card: CardView,
        show_project: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let drag = BoardCardDrag {
            project_id: card.project_id.clone(),
            terminal_id: card.terminal_id.clone(),
            name: card.name.clone(),
        };
        let (jump_project, jump_terminal) = (card.project_id.clone(), card.terminal_id.clone());
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

        div()
            .id(ElementId::Name(
                format!("board-card-{}-{}", card.project_id, card.terminal_id).into(),
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
                this.jump_to(&jump_project, &jump_terminal, cx);
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .min_w_0()
                    .when(show_project, |d| {
                        d.child(
                            div()
                                .size(px(7.0))
                                .rounded_full()
                                .bg(rgb(t.get_folder_color(card.folder_color)))
                                .flex_shrink_0(),
                        )
                    })
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
            .when(show_project, |d| {
                d.child(
                    div()
                        .text_size(ui_text_sm(cx))
                        .text_color(rgb(t.text_muted))
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(card.project_name),
                )
            })
            .children(agent_line)
            .into_any_element()
    }

    fn render_column(
        &self,
        column: ColumnView,
        show_project: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let slug = column.id.clone().unwrap_or_else(|| "none".to_string());
        let drop_status = column.id.clone();
        let count = column.cards.len();
        let cards: Vec<AnyElement> = column
            .cards
            .into_iter()
            .map(|card| self.render_card(card, show_project, cx))
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
                    cx.emit(TerminalBoardEvent::SetStatus {
                        project_id: drag.project_id.clone(),
                        terminal_id: drag.terminal_id.clone(),
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
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_secondary))
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

impl Render for TerminalBoard {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }
        let show_project = matches!(self.scope, BoardScope::All);
        let columns = self.columns(cx);
        let column_elements: Vec<AnyElement> = columns
            .into_iter()
            .map(|column| self.render_column(column, show_project, cx))
            .collect();

        modal_backdrop("terminal-board-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("TerminalBoard")
            .on_action(cx.listener(|this, _: &Cancel, _window, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.close(cx)),
            )
            .child(
                modal_content("terminal-board-modal", &t)
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
                                icon_button("terminal-board-close", "icons/close.svg", &t)
                                    .on_click(cx.listener(|this, _, _window, cx| this.close(cx))),
                            ),
                    )
                    .child(
                        div()
                            .id("terminal-board-columns")
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

impl_focusable!(TerminalBoard);
