//! Settings > General > "Status Columns": a small list editor for
//! the status board columns (rename in place, move up / down,
//! remove, add). Ids never change here, only names and order, so every
//! project keeps its status through an edit.

use crate::settings::settings_entity;
use crate::theme::theme;
use crate::ui::tokens::ui_text_md;
use crate::views::components::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use crate::workspace::settings::BoardColumn;
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::FolderColor;
use okena_ui::button::button;
use okena_ui::icon_button::icon_button;
use okena_ui::settings::{input_box, settings_input_row};

use super::SettingsPanel;

impl SettingsPanel {
    /// Keep one name input per column, by id. Inputs for columns that are
    /// gone drop (and with them their subscriptions); new columns get a fresh
    /// input seeded with the current name. Order follows the settings.
    fn sync_board_column_inputs(&mut self, columns: &[BoardColumn], cx: &mut Context<Self>) {
        let current: Vec<&str> = self
            .board_column_inputs
            .iter()
            .map(|(id, _)| id.as_str())
            .collect();
        let wanted: Vec<&str> = columns.iter().map(|c| c.id.as_str()).collect();
        if current == wanted {
            return;
        }
        let mut existing: std::collections::HashMap<String, Entity<SimpleInputState>> =
            std::mem::take(&mut self.board_column_inputs)
                .into_iter()
                .collect();
        self.board_column_inputs = columns
            .iter()
            .map(|column| {
                let input = existing.remove(&column.id).unwrap_or_else(|| {
                    let name = column.name.clone();
                    let input = cx.new(|cx| {
                        SimpleInputState::new(cx)
                            .placeholder("Column name…")
                            .default_value(name)
                    });
                    let id = column.id.clone();
                    cx.subscribe(&input, move |_this, entity, _: &InputChangedEvent, cx| {
                        let name = entity.read(cx).value().to_string();
                        settings_entity(cx).update(cx, |state, cx| {
                            state.rename_status_board_column(&id, name, cx);
                        });
                    })
                    .detach();
                    input
                });
                (column.id.clone(), input)
            })
            .collect();
    }

    pub(super) fn render_board_columns(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let columns = settings_entity(cx)
            .read(cx)
            .settings
            .status_board
            .columns
            .clone();
        self.sync_board_column_inputs(&columns, cx);
        let count = self.board_column_inputs.len();

        let rows: Vec<AnyElement> = self
            .board_column_inputs
            .iter()
            .enumerate()
            .map(|(index, (id, input))| {
                let id_up = id.clone();
                let id_down = id.clone();
                let id_remove = id.clone();
                let current_color = columns
                    .iter()
                    .find(|c| &c.id == id)
                    .map(|c| c.color)
                    .unwrap_or_default();
                let swatches: Vec<AnyElement> = FolderColor::all()
                    .iter()
                    .map(|&color| {
                        let id_color = id.clone();
                        let selected = color == current_color;
                        div()
                            .id(ElementId::Name(
                                format!("board-col-color-{id}-{color:?}").into(),
                            ))
                            .size(px(12.0))
                            .rounded(px(3.0))
                            .bg(rgb(t.get_folder_color(color)))
                            .cursor_pointer()
                            .when(selected, |d| d.border_2().border_color(rgb(t.text_primary)))
                            .when(!selected, |d| d.border_1().border_color(rgb(t.border)))
                            .hover(|s| s.opacity(0.8))
                            .on_click(cx.listener(move |_this, _, _window, cx| {
                                settings_entity(cx).update(cx, |state, cx| {
                                    state.set_status_board_column_color(&id_color, color, cx);
                                });
                            }))
                            .into_any_element()
                    })
                    .collect();
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        input_box(&t)
                            .flex_1()
                            .min_w_0()
                            .child(SimpleInput::new(input).text_size(ui_text_md(cx))),
                    )
                    .child(div().flex().items_center().gap(px(3.0)).children(swatches))
                    .child(
                        icon_button(
                            ElementId::Name(format!("board-col-up-{id}").into()),
                            "icons/chevron-up.svg",
                            &t,
                        )
                        .when(index == 0, |b| b.opacity(0.3))
                        .on_click(cx.listener(
                            move |_this, _, _window, cx| {
                                settings_entity(cx).update(cx, |state, cx| {
                                    state.move_status_board_column(&id_up, -1, cx);
                                });
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            ElementId::Name(format!("board-col-down-{id}").into()),
                            "icons/chevron-down.svg",
                            &t,
                        )
                        .when(index + 1 == count, |b| b.opacity(0.3))
                        .on_click(cx.listener(
                            move |_this, _, _window, cx| {
                                settings_entity(cx).update(cx, |state, cx| {
                                    state.move_status_board_column(&id_down, 1, cx);
                                });
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            ElementId::Name(format!("board-col-remove-{id}").into()),
                            "icons/trash.svg",
                            &t,
                        )
                        .on_click(cx.listener(
                            move |_this, _, _window, cx| {
                                settings_entity(cx).update(cx, |state, cx| {
                                    state.remove_status_board_column(&id_remove, cx);
                                });
                            },
                        )),
                    )
                    .into_any_element()
            })
            .collect();

        settings_input_row(
            "status-board-columns",
            "Status Columns",
            "Statuses a project or worktree can be sorted into (column header pill, project menu → Status, status lanes). Renaming keeps every status; removing a column leaves its projects unassigned.",
            &t,
            cx,
            true,
        )
        .children(rows)
        .child(
            div().child(
                button("status-board-add-column", "+ Add column", &t).on_click(cx.listener(
                    |_this, _, _window, cx| {
                        settings_entity(cx).update(cx, |state, cx| {
                            state.add_status_board_column("New column", cx);
                        });
                    },
                )),
            ),
        )
    }
}
