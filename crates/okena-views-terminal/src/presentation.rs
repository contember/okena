use gpui::*;

type Confirmation = Box<dyn FnOnce(&mut App)>;

/// A one-shot reveal intent. Only the target window's painted terminal can consume it.
struct PendingTerminalReveal {
    terminal_id: String,
    window: AnyWindowHandle,
    confirm: Confirmation,
}

#[derive(Default)]
struct TerminalReveals(Option<PendingTerminalReveal>);
impl Global for TerminalReveals {}

pub fn request_reveal(
    terminal_id: String,
    window: AnyWindowHandle,
    confirm: impl FnOnce(&mut App) + 'static,
    cx: &mut App,
) {
    if !cx.has_global::<TerminalReveals>() {
        cx.set_global(TerminalReveals::default());
    }
    let pending = &mut cx.global_mut::<TerminalReveals>().0;
    *pending = Some(PendingTerminalReveal {
        terminal_id,
        window,
        confirm: Box::new(confirm),
    });
}

pub(crate) fn painted(
    terminal_id: &str,
    focus: &FocusHandle,
    bounds: Bounds<Pixels>,
    cell_size: Size<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if !window.is_window_active()
        || !focus.is_focused(window)
        || bounds.size.width <= px(0.0)
        || bounds.size.height <= px(0.0)
        || !cx.has_global::<TerminalReveals>()
    {
        return;
    }
    let visible = bounds.intersect(&window.content_mask().bounds);
    if visible.size.width < cell_size.width || visible.size.height < cell_size.height {
        return;
    }
    let pending = &mut cx.global_mut::<TerminalReveals>().0;
    if pending.as_ref().is_some_and(|request| {
        request.terminal_id == terminal_id && request.window == window.window_handle()
    }) && let Some(request) = pending.take()
    {
        let focus = focus.clone();
        cx.defer(move |cx| {
            let still_focused = request
                .window
                .update(cx, |_, window, _| {
                    window.is_window_active() && focus.is_focused(window)
                })
                .unwrap_or(false);
            if still_focused {
                (request.confirm)(cx);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{painted, request_reveal};
    use crate::layout::terminal_pane::TerminalContent;
    use gpui::{
        AppContext, Bounds, Context, FocusHandle, InteractiveElement, IntoElement, ParentElement,
        Render, Styled, TestAppContext, Window, WindowHandle, canvas, div, px, size,
    };
    use okena_terminal::terminal::{Terminal, TerminalTransport};
    use std::sync::Arc;
    use std::{cell::Cell, rc::Rc};

    struct NullTransport;
    impl TerminalTransport for NullTransport {
        fn send_input(&self, _: &str, _: &[u8]) {}
        fn resize(&self, _: &str, _: u16, _: u16) {}
        fn uses_mouse_backend(&self) -> bool {
            false
        }
    }

    struct TerminalSurface {
        focus: FocusHandle,
        content: gpui::Entity<TerminalContent>,
        visible: bool,
    }
    impl Render for TerminalSurface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let root = div().size_full().track_focus(&self.focus);
            if self.visible {
                root.child(self.content.clone())
            } else {
                root
            }
        }
    }

    #[gpui::test]
    fn real_terminal_content_confirms_only_after_surface_is_presented(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(okena_ui::theme::GlobalThemeProvider(|_| {
                okena_ui::theme::DARK_THEME
            }));
            cx.set_global(okena_extensions::ExtensionSettingsStore::new(
                |_, _| None,
                |_, _, _| {},
            ));
        });
        let window = cx.add_window(|_, cx| {
            let workspace = cx.new(|_| {
                okena_workspace::state::Workspace::new(
                    okena_workspace::state::WorkspaceData::empty(),
                )
            });
            let broker = cx.new(|_| okena_workspace::request_broker::RequestBroker::new());
            let focus = cx.focus_handle();
            let content = cx.new(|cx| {
                TerminalContent::new(
                    focus.clone(),
                    None,
                    "p".into(),
                    vec![],
                    workspace,
                    broker,
                    cx,
                )
            });
            TerminalSurface {
                focus,
                content,
                visible: true,
            }
        });
        let read = Rc::new(Cell::new(false));
        cx.update(|cx| {
            let read = read.clone();
            request_reveal(
                "remote:a:t".into(),
                window.into(),
                move |_| read.set(true),
                cx,
            );
        });
        window
            .update(cx, |root, window, cx| {
                window.activate_window();
                window.focus(&root.focus, cx);
            })
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            !read.get(),
            "Starting terminal placeholder is not a successful reveal"
        );
        window
            .update(cx, |root, _, cx| {
                let terminal = Arc::new(Terminal::new(
                    "remote:a:t".into(),
                    okena_terminal::terminal::TerminalSize::default(),
                    Arc::new(NullTransport),
                    String::new(),
                ));
                terminal.process_output(b"completed result");
                root.content.update(cx, |content, cx| {
                    content.set_terminal(Some(terminal), cx);
                    cx.notify();
                });
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert!(read.get());
    }

    struct Surface {
        focus: FocusHandle,
        visible: bool,
        terminal: String,
    }
    impl Render for Surface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let root = div().size_full().track_focus(&self.focus);
            if !self.visible {
                return root.into_any_element();
            }
            let focus = self.focus.clone();
            let terminal = self.terminal.clone();
            root.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        painted(
                            &terminal,
                            &focus,
                            bounds,
                            size(px(8.0), px(16.0)),
                            window,
                            cx,
                        )
                    },
                )
                .size_full(),
            )
            .into_any_element()
        }
    }

    fn draw(cx: &mut TestAppContext, window: WindowHandle<Surface>) {
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn reveal_requires_painted_focused_surface_in_target_active_window(cx: &mut TestAppContext) {
        let target = cx.add_window(|_, cx| Surface {
            focus: cx.focus_handle(),
            visible: false,
            terminal: "remote:a:t".into(),
        });
        let other = cx.add_window(|_, cx| Surface {
            focus: cx.focus_handle(),
            visible: true,
            terminal: "remote:a:t".into(),
        });
        let read = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let read = read.clone();
            request_reveal(
                "remote:a:t".into(),
                target.into(),
                move |_| read.set(read.get() + 1),
                cx,
            );
        });
        target
            .update(cx, |surface, window, cx| {
                window.activate_window();
                window.focus(&surface.focus, cx);
            })
            .unwrap();
        draw(cx, target);
        assert_eq!(
            read.get(),
            0,
            "hidden / inactive tab / absent surface cannot acknowledge"
        );
        other
            .update(cx, |surface, window, cx| {
                window.activate_window();
                window.focus(&surface.focus, cx);
            })
            .unwrap();
        draw(cx, other);
        assert_eq!(read.get(), 0, "another window cannot consume the reveal");
        target
            .update(cx, |surface, _, cx| {
                surface.visible = true;
                cx.notify();
            })
            .unwrap();
        draw(cx, target);
        assert_eq!(read.get(), 0, "background window cannot consume the reveal");
        target
            .update(cx, |surface, window, cx| {
                window.activate_window();
                window.focus(&surface.focus, cx);
                window.refresh();
            })
            .unwrap();
        draw(cx, target);
        assert_eq!(read.get(), 1);
        draw(cx, target);
        assert_eq!(read.get(), 1, "confirmation is one-shot");
    }

    #[gpui::test]
    fn failed_and_zero_sized_reveals_leave_result_unread(cx: &mut TestAppContext) {
        let target = cx.add_window(|_, cx| Surface {
            focus: cx.focus_handle(),
            visible: true,
            terminal: "remote:b:t".into(),
        });
        let read = Rc::new(Cell::new(false));
        cx.update(|cx| {
            let read = read.clone();
            request_reveal(
                "remote:a:t".into(),
                target.into(),
                move |_| read.set(true),
                cx,
            );
        });
        target
            .update(cx, |surface, window, cx| {
                window.activate_window();
                window.focus(&surface.focus, cx);
                painted(
                    "remote:a:t",
                    &surface.focus,
                    Bounds::new(Default::default(), size(px(0.0), px(0.0))),
                    size(px(8.0), px(16.0)),
                    window,
                    cx,
                );
            })
            .unwrap();
        draw(cx, target);
        assert!(
            !read.get(),
            "a colliding raw ID or absent drawable area cannot acknowledge"
        );
    }
}
