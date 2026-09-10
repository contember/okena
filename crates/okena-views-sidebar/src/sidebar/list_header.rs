//! The sidebar's list header — one row for choosing a list and acting on it.
//!
//! This used to be two stacked rows: PROJECTS/AGENTS tabs above an "Overview"
//! row carrying that list's controls. Two headers for one list read as chrome
//! rather than structure, and their differing padding never lined up. One row
//! now holds all of it: the list selector, the way to see everything in that
//! list, an overflow menu for how it is arranged, and one button to add to it.
//!
//! Both lists wear the same shape. What differs is only what each puts behind
//! the overflow menu and the add button.

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;
use gpui_component::tooltip::Tooltip;
use okena_ui::theme::theme;
use okena_ui::tokens::ui_text_ms;

use super::{Sidebar, SidebarHeaderMenu, SidebarList};

/// One header icon's identity and state.
///
/// Bundled because five positional arguments to a button helper is where
/// transposing two of them stops being caught by the compiler.
struct HeaderIcon {
    id: &'static str,
    icon: &'static str,
    tooltip: &'static str,
    active: bool,
    /// Set only on a button that anchors a menu: the menu has to know where the
    /// button ended up.
    capture_bounds: Option<Entity<Sidebar>>,
}
use okena_workspace::state::WindowId;

impl Sidebar {
    /// Whether the main area is showing everything in the current list.
    ///
    /// "Nothing focused" is the shared half; the agents list adds its own flag,
    /// because with it off the grid is showing the projects overview instead.
    fn overview_is_active(&self, cx: &App) -> bool {
        if self.focus_manager.read(cx).focused_project_id().is_some() {
            return false;
        }
        match self.list {
            SidebarList::Projects => self
                .workspace
                .read(cx)
                .active_folder_filter(self.window_id)
                .is_none(),
            SidebarList::Agents => self
                .workspace
                .read(cx)
                .data()
                .window(self.window_id)
                .is_some_and(|w| w.agents_overview),
        }
    }

    /// Show everything in the current list in the main area.
    fn open_overview(&mut self, cx: &mut Context<Self>) {
        // A harness view covering the grid would make this look like it did
        // nothing, the same reason selecting a project leaves one.
        self.leave_harness_view(cx);
        let window_id = self.window_id;
        let agents = self.list == SidebarList::Agents;
        let workspace = self.workspace.clone();
        self.focus_manager.update(cx, |fm, cx| {
            workspace.update(cx, |ws, cx| {
                ws.set_focused_project(fm, None, cx);
                // The two overviews select different things and must not be
                // able to contradict each other.
                ws.set_agents_overview(window_id, agents, cx);
                if !agents {
                    ws.set_folder_filter(window_id, None, cx);
                }
            });
            cx.notify();
        });
        cx.notify();
    }

    /// One list tab.
    fn list_tab(
        &self,
        label: &'static str,
        mode: SidebarList,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.list == mode;
        div()
            .id(ElementId::Name(label.into()))
            .cursor_pointer()
            .px(px(6.0))
            .py(px(2.0))
            .rounded(px(4.0))
            .when(!selected, |d| d.hover(|s| s.bg(rgb(t.bg_hover))))
            .text_size(ui_text_ms(cx))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(if selected {
                t.text_primary
            } else {
                t.text_muted
            }))
            .child(label)
            .on_click(cx.listener(move |this, _, _window, cx| {
                if this.list == mode {
                    return;
                }
                this.list = mode;
                // The cursor indexes into the list that is going away.
                this.cursor_index = None;
                // An open menu belongs to the tab that is leaving; its contents
                // and its anchor both change.
                this.header_menu = None;
                // The agents overview filters the grid to sessions, so it must
                // not survive onto the projects tab, where it would hide every
                // repo. Invisible while a project is focused, since focus wins.
                if mode == SidebarList::Projects {
                    let window_id = this.window_id;
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_agents_overview(window_id, false, cx);
                    });
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// A square icon button in the header.
    fn header_icon(
        &self,
        spec: HeaderIcon,
        on_click: impl Fn(&mut Sidebar, &ClickEvent, &mut Window, &mut Context<Sidebar>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let HeaderIcon {
            id,
            icon,
            tooltip,
            active,
            capture_bounds,
        } = spec;
        let t = theme(cx);
        div()
            .id(id)
            .cursor_pointer()
            .size(px(22.0))
            .flex_shrink_0()
            .rounded(px(4.0))
            .when(active, |d| d.bg(rgb(t.bg_hover)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .flex()
            .items_center()
            .justify_center()
            .child(svg().path(icon).size(px(13.0)).text_color(rgb(if active {
                t.text_primary
            } else {
                t.text_secondary
            })))
            // Menus anchor to the button they came from, so the button has to
            // report where it ended up.
            .children(capture_bounds.map(|entity| {
                canvas(
                    move |bounds, _window, app| {
                        entity.update(app, |this, _cx| {
                            this.overflow_button_bounds = bounds;
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            }))
            .tooltip(move |window, cx| Tooltip::new(tooltip).build(window, cx))
            .on_click(cx.listener(move |this, event, window, cx| {
                cx.stop_propagation();
                on_click(this, event, window, cx);
            }))
            .into_any_element()
    }

    /// The list header: selector, overview, overflow menu, add.
    pub(super) fn render_list_header(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let agents = self.list == SidebarList::Agents;
        let overview_active = self.overview_is_active(cx);
        let overflow_open = self.header_menu == Some(SidebarHeaderMenu::Overflow);
        let create_open = self.header_menu == Some(SidebarHeaderMenu::Create);
        let entity = cx.entity().clone();

        // No `relative()` here: the menu is deferred and anchored to the
        // window, so a positioned ancestor would only skew it.
        div()
            .child(
                h_flex()
                    .h(px(32.0))
                    .w_full()
                    .items_center()
                    .gap(px(4.0))
                    .pl(px(12.0))
                    .pr(px(8.0))
                    .child(self.list_tab("PROJECTS", SidebarList::Projects, cx))
                    .child(self.list_tab("AGENTS", SidebarList::Agents, cx))
                    .child(div().flex_1().min_w_0())
                    .child(self.header_icon(
                        HeaderIcon {
                            id: "list-overview",
                            icon: "icons/select-all.svg",
                            tooltip: "Show everything in the main area",
                            active: overview_active,
                            capture_bounds: None,
                        },
                        |this, _, _window, cx| this.open_overview(cx),
                        cx,
                    ))
                    .child(self.header_icon(
                        HeaderIcon {
                            id: "list-overflow",
                            icon: "icons/more-horizontal.svg",
                            tooltip: "View options",
                            active: overflow_open,
                            capture_bounds: Some(entity),
                        },
                        |this, _, _window, cx| {
                            this.header_menu =
                                if this.header_menu == Some(SidebarHeaderMenu::Overflow) {
                                    None
                                } else {
                                    Some(SidebarHeaderMenu::Overflow)
                                };
                            cx.notify();
                        },
                        cx,
                    ))
                    // One add button on both tabs. Projects opens a menu, since
                    // there are two things to create; agents go straight to the
                    // dialog, since there is one.
                    .child(self.render_add_button(agents, create_open, cx)),
            )
            .child(self.render_header_menu(cx))
    }

    /// The header's add button.
    fn render_add_button(
        &mut self,
        agents: bool,
        create_open: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let entity = cx.entity().clone();
        div()
            .id("list-add")
            .cursor_pointer()
            .size(px(22.0))
            .flex_shrink_0()
            .rounded(px(4.0))
            .when(create_open, |d| d.bg(rgb(t.bg_hover)))
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/plus.svg")
                    .size(px(14.0))
                    .text_color(rgb(t.text_secondary)),
            )
            .children((!agents).then(|| {
                canvas(
                    move |bounds, _window, app| {
                        entity.update(app, |this, _cx| {
                            this.create_button_bounds = bounds;
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full()
            }))
            .tooltip(move |window, cx| {
                Tooltip::new(if agents {
                    "New agent session"
                } else {
                    "New project or folder"
                })
                .build(window, cx)
            })
            .on_click(cx.listener(move |this, _, _window, cx| {
                cx.stop_propagation();
                if agents {
                    this.request_broker.update(cx, |broker, cx| {
                        broker.push_overlay_request(
                            okena_workspace::requests::OverlayRequest::NewAgentDialog,
                            cx,
                        );
                    });
                } else {
                    this.header_menu = if this.header_menu == Some(SidebarHeaderMenu::Create) {
                        None
                    } else {
                        Some(SidebarHeaderMenu::Create)
                    };
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    /// A labelled row in the overflow menu, with a check when it is on.
    fn menu_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        idle_icon: &'static str,
        on_click: impl Fn(&mut Sidebar, &mut Context<Sidebar>) + 'static,
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
            this.header_menu = None;
            on_click(this, cx);
            cx.notify();
        }))
        .into_any_element()
    }

    /// The header's open menu, anchored under the button that opened it.
    fn render_header_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let window_id = self.window_id;
        let Some(menu) = self.header_menu else {
            return div().size_0().into_any_element();
        };
        let bounds = match menu {
            SidebarHeaderMenu::Create => self.create_button_bounds,
            SidebarHeaderMenu::Overflow => self.overflow_button_bounds,
        };
        let position = point(
            bounds.origin.x,
            bounds.origin.y + bounds.size.height + px(4.0),
        );

        let panel = match menu {
            SidebarHeaderMenu::Create => okena_ui::menu::context_menu_panel("list-create-menu", &t)
                .min_w(px(180.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.header_menu = None;
                    cx.notify();
                }))
                .child(
                    okena_ui::menu::menu_item(
                        "create-project-menu-item",
                        "icons/plus.svg",
                        "New project",
                        &t,
                    )
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.header_menu = None;
                        this.request_broker.update(cx, |broker, cx| {
                            broker.push_overlay_request(
                                okena_workspace::requests::OverlayRequest::AddProjectDialog,
                                cx,
                            );
                        });
                        cx.notify();
                    })),
                )
                .child(
                    okena_ui::menu::menu_item(
                        "create-folder-menu-item",
                        "icons/folder.svg",
                        "New folder",
                        &t,
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.header_menu = None;
                        this.create_folder(window, cx);
                    })),
                )
                .into_any_element(),
            SidebarHeaderMenu::Overflow => self.render_overflow_menu(window_id, cx),
        };

        // Deferred and anchored to the window rather than placed inside the
        // header: the captured bounds are window-absolute, so positioning them
        // against a `relative()` ancestor offsets the menu by that ancestor's
        // own origin — and the sidebar clips anything wider than itself.
        // `snap_to_window` keeps it on screen when the button is near an edge.
        deferred(
            anchored()
                .position(position)
                .anchor(Anchor::TopLeft)
                .snap_to_window()
                .child(div().occlude().child(panel)),
        )
        .with_priority(1)
        .into_any_element()
    }

    /// The overflow menu's contents, which depend on the list it belongs to.
    fn render_overflow_menu(&mut self, window_id: WindowId, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rows = self
            .workspace
            .read(cx)
            .grid_layout_mode(window_id)
            .is_rows();
        let mut panel = okena_ui::menu::context_menu_panel("list-overflow-menu", &t)
            .min_w(px(220.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.header_menu = None;
                cx.notify();
            }))
            .child(okena_ui::menu::menu_section("Order", &t));

        match self.list {
            SidebarList::Projects => {
                let activity = self
                    .workspace
                    .read(cx)
                    .data()
                    .window(window_id)
                    .is_some_and(|w| w.project_sort_mode.is_activity());
                let attention = self
                    .workspace
                    .read(cx)
                    .data()
                    .window(window_id)
                    .is_some_and(|w| w.show_attention_section);
                panel = panel
                    .child(self.menu_toggle(
                        "order-manual",
                        "Manual order",
                        !activity,
                        "icons/folder.svg",
                        move |this, cx| {
                            if activity {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.toggle_project_sort_mode(window_id, cx);
                                });
                            }
                        },
                        cx,
                    ))
                    .child(self.menu_toggle(
                        "order-activity",
                        "By activity",
                        activity,
                        "icons/bell.svg",
                        move |this, cx| {
                            if !activity {
                                this.workspace.update(cx, |ws, cx| {
                                    ws.toggle_project_sort_mode(window_id, cx);
                                });
                            }
                        },
                        cx,
                    ));
                // Only meaningful in the manual view — the activity view has a
                // needs-attention tier of its own.
                if !activity {
                    panel =
                        panel
                            .child(okena_ui::menu::menu_separator(&t))
                            .child(self.menu_toggle(
                                "attention-section",
                                "Needs attention section",
                                attention,
                                "icons/bell.svg",
                                move |this, cx| {
                                    this.workspace.update(cx, |ws, cx| {
                                        ws.toggle_show_attention_section(window_id, cx);
                                    });
                                },
                                cx,
                            ));
                }
            }
            SidebarList::Agents => {
                let by_activity = self
                    .workspace
                    .read(cx)
                    .data()
                    .window(window_id)
                    .map(|w| w.agent_sort_mode)
                    .unwrap_or_default()
                    .is_activity();
                let showing_info = self.workspace.read(cx).agents_show_info(window_id);
                panel = panel
                    .child(self.menu_toggle(
                        "agent-order-activity",
                        "By activity",
                        by_activity,
                        "icons/arrow-up-down.svg",
                        move |this, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.set_agent_sort_mode(
                                    window_id,
                                    okena_workspace::state::AgentSortMode::Activity,
                                    cx,
                                );
                            });
                        },
                        cx,
                    ))
                    .child(self.menu_toggle(
                        "agent-order-name",
                        "By name",
                        !by_activity,
                        "icons/arrow-up-down.svg",
                        move |this, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.set_agent_sort_mode(
                                    window_id,
                                    okena_workspace::state::AgentSortMode::Name,
                                    cx,
                                );
                            });
                        },
                        cx,
                    ))
                    .child(okena_ui::menu::menu_separator(&t))
                    .child(self.menu_toggle(
                        "agent-show-info",
                        "Show session info",
                        showing_info,
                        "icons/panel-right.svg",
                        move |this, cx| {
                            this.workspace.update(cx, |ws, cx| {
                                ws.set_agents_show_info(window_id, !showing_info, cx);
                            });
                        },
                        cx,
                    ));
            }
        }

        panel
            .child(okena_ui::menu::menu_section("Layout", &t))
            .child(self.menu_toggle(
                "layout-columns",
                "Side by side",
                !rows,
                "icons/split-vertical.svg",
                move |this, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_grid_layout_mode(
                            window_id,
                            okena_workspace::state::ProjectLayoutMode::Columns,
                            cx,
                        );
                    });
                },
                cx,
            ))
            .child(self.menu_toggle(
                "layout-rows",
                "Stacked",
                rows,
                "icons/split-horizontal.svg",
                move |this, cx| {
                    this.workspace.update(cx, |ws, cx| {
                        ws.set_grid_layout_mode(
                            window_id,
                            okena_workspace::state::ProjectLayoutMode::Rows,
                            cx,
                        );
                    });
                },
                cx,
            ))
            .into_any_element()
    }
}
