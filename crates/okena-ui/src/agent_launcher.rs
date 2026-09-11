//! The one control for starting an agent session, wherever one can be started.
//!
//! Starting an agent used to take a different shape in every place that could
//! do it: a "Start work" button, a "Break down with agent" button, agent chips
//! above a "Start session" button, and an "Open session" button standing in for
//! all of them once something was running. The launcher is the single shape:
//! what would start, a round button per agent to start it right away, a way to
//! configure it first, and — once something is running — the sessions
//! themselves, with how they are doing and a way in.
//!
//! It knows nothing about tasks, specs or agents in particular. The caller
//! says what the options are and what clicking them does.

use crate::theme::{theme, with_alpha};
use crate::tokens::{ui_text_md, ui_text_ms};
use gpui::prelude::*;
use gpui::*;
use gpui_component::tooltip::Tooltip;
use gpui_component::{h_flex, v_flex};
use std::rc::Rc;

/// One way to start: an agent, or deliberately none.
#[derive(Clone, Debug)]
pub struct LaunchOption {
    /// Command handed to the daemon. Empty means start no agent — a plain
    /// shell, worktrees only, a scaffold — which each caller names in `label`.
    pub command: SharedString,
    pub label: SharedString,
    pub icon: SharedString,
    pub accent: u32,
}

impl LaunchOption {
    fn tooltip(&self) -> SharedString {
        if self.command.is_empty() {
            self.label.clone()
        } else {
            format!("Start with {}", self.label).into()
        }
    }
}

/// A session already running for whatever the launcher starts.
#[derive(Clone, Debug)]
pub struct LauncherSession {
    /// Handed back to `on_open`.
    pub id: SharedString,
    pub name: SharedString,
    /// "running", "waiting · 3m", "stopped" — pre-worded by the caller, which
    /// knows how it decided.
    pub activity: SharedString,
    pub activity_color: u32,
    /// The agent in it, when one is recognized: its icon and accent.
    pub agent: Option<LaunchOption>,
    /// What the agent last said it was doing.
    pub status: Option<SharedString>,
    /// Something wrong worth a word, e.g. that okena's MCP is not wired in.
    pub warning: Option<SharedString>,
}

/// How much chrome the launcher draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LauncherStyle {
    /// A card of its own, for a launcher standing among other content.
    #[default]
    Card,
    /// No border or fill, for a form footer that already frames it.
    Inline,
}

type CommandHandler = Rc<dyn Fn(&SharedString, &mut Window, &mut App)>;
type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct AgentLauncher {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    style: LauncherStyle,
    options: Vec<LaunchOption>,
    /// Command drawn as the default, so a quick start is not a guess.
    preferred: Option<SharedString>,
    sessions: Vec<LauncherSession>,
    /// Shown in place of the buttons while a start is in flight.
    busy: Option<SharedString>,
    /// Whether the start buttons stay once a session exists. Off by default:
    /// for most things a second session duplicates the first.
    launch_alongside_sessions: bool,
    on_launch: Option<CommandHandler>,
    on_configure: Option<(SharedString, ClickHandler)>,
    on_open: Option<CommandHandler>,
}

impl AgentLauncher {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            style: LauncherStyle::default(),
            options: Vec::new(),
            preferred: None,
            sessions: Vec::new(),
            busy: None,
            launch_alongside_sessions: false,
            on_launch: None,
            on_configure: None,
            on_open: None,
        }
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn style(mut self, style: LauncherStyle) -> Self {
        self.style = style;
        self
    }

    pub fn options(mut self, options: Vec<LaunchOption>) -> Self {
        self.options = options;
        self
    }

    pub fn preferred(mut self, command: Option<impl Into<SharedString>>) -> Self {
        self.preferred = command.map(Into::into);
        self
    }

    pub fn sessions(mut self, sessions: Vec<LauncherSession>) -> Self {
        self.sessions = sessions;
        self
    }

    pub fn busy(mut self, label: Option<impl Into<SharedString>>) -> Self {
        self.busy = label.map(Into::into);
        self
    }

    pub fn launch_alongside_sessions(mut self) -> Self {
        self.launch_alongside_sessions = true;
        self
    }

    /// Called with the chosen option's command.
    pub fn on_launch(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_launch = Some(Rc::new(handler));
        self
    }

    /// Offer a "configure first" button, described by `tooltip`.
    pub fn on_configure(
        mut self,
        tooltip: impl Into<SharedString>,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_configure = Some((tooltip.into(), Rc::new(handler)));
        self
    }

    /// Called with a session's id when it is clicked.
    pub fn on_open(
        mut self,
        handler: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open = Some(Rc::new(handler));
        self
    }

    /// A start in flight always says so, sessions or not: the one it is
    /// starting is not among them yet.
    fn shows_buttons(&self) -> bool {
        self.busy.is_some() || self.sessions.is_empty() || self.launch_alongside_sessions
    }

    fn render_buttons(&self, cx: &App) -> AnyElement {
        let t = theme(cx);
        if let Some(label) = self.busy.clone() {
            return h_flex()
                .flex_shrink_0()
                .h(px(BUTTON_SIZE))
                .items_center()
                .gap(px(6.0))
                .child(div().size(px(6.0)).rounded_full().bg(rgb(t.warning)))
                .child(
                    div()
                        .text_size(ui_text_md(cx))
                        .text_color(rgb(t.text_secondary))
                        .child(label),
                )
                .into_any_element();
        }

        let mut row = h_flex().flex_shrink_0().items_center().gap(px(6.0));

        if let Some((tooltip, handler)) = self.on_configure.clone() {
            row = row.child(
                div()
                    .id(SharedString::from(format!("{}-configure", self.id)))
                    .cursor_pointer()
                    .size(px(BUTTON_SIZE))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(rgb(t.border))
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .child(
                        svg()
                            .path("icons/settings.svg")
                            .size(px(ICON_SIZE))
                            .text_color(rgb(t.text_secondary)),
                    )
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        handler(event, window, cx);
                    }),
            );
            if !self.options.is_empty() {
                // Keeps "configure" from reading as one more agent.
                row = row.child(div().w(px(1.0)).h(px(16.0)).mx(px(2.0)).bg(rgb(t.border)));
            }
        }

        for option in &self.options {
            let preferred = self.preferred.as_ref() == Some(&option.command);
            let accent = option.accent;
            let command = option.command.clone();
            let tooltip = option.tooltip();
            let handler = self.on_launch.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "{}-launch-{}",
                        self.id,
                        if option.command.is_empty() {
                            "none"
                        } else {
                            option.command.as_ref()
                        }
                    )))
                    .cursor_pointer()
                    .size(px(BUTTON_SIZE))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(with_alpha(accent, 0.14))
                    .hover(move |s| s.bg(with_alpha(accent, 0.3)))
                    .border_1()
                    .border_color(if preferred {
                        with_alpha(accent, 0.7)
                    } else {
                        with_alpha(accent, 0.0)
                    })
                    .child(
                        svg()
                            .path(option.icon.clone())
                            .size(px(ICON_SIZE))
                            .text_color(rgb(accent)),
                    )
                    .tooltip(move |window, cx| {
                        let text = if preferred {
                            format!("{tooltip} (default)").into()
                        } else {
                            tooltip.clone()
                        };
                        Tooltip::new(text).build(window, cx)
                    })
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        if let Some(handler) = handler.as_ref() {
                            handler(&command, window, cx);
                        }
                    }),
            );
        }
        row.into_any_element()
    }

    fn render_session(&self, session: &LauncherSession, cx: &App) -> AnyElement {
        let t = theme(cx);
        let handler = self.on_open.clone();
        let id = session.id.clone();

        let mut meta = h_flex().min_w_0().items_center().gap(px(6.0));
        if let Some(agent) = session.agent.as_ref() {
            meta = meta.child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(3.0))
                    .child(
                        svg()
                            .path(agent.icon.clone())
                            .size(px(10.0))
                            .text_color(rgb(agent.accent)),
                    )
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(agent.label.clone()),
                    ),
            );
        }
        meta = meta.child(
            div()
                .flex_shrink_0()
                .text_size(ui_text_ms(cx))
                .text_color(rgb(session.activity_color))
                .child(session.activity.clone()),
        );
        if let Some(warning) = session.warning.clone() {
            meta = meta.child(
                div()
                    .flex_shrink_0()
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(format!("· {warning}")),
            );
        }

        h_flex()
            .id(SharedString::from(format!(
                "{}-session-{}",
                self.id, session.id
            )))
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .bg(rgb(t.bg_primary))
            .when(handler.is_some(), |d| {
                d.cursor_pointer().hover(|s| s.bg(rgb(t.bg_hover)))
            })
            .child(
                div()
                    .flex_shrink_0()
                    .size(px(8.0))
                    .rounded_full()
                    .bg(rgb(session.activity_color)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.0))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .child(session.name.clone()),
                    )
                    .child(meta)
                    .children(session.status.clone().map(|status| {
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_secondary))
                            .child(format!("\u{201c}{status}\u{201d}"))
                    })),
            )
            .when(handler.is_some(), |d| {
                d.child(
                    h_flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(2.0))
                        .text_size(ui_text_ms(cx))
                        .text_color(rgb(t.text_secondary))
                        .child("Open")
                        .child(
                            svg()
                                .path("icons/chevron-right.svg")
                                .size(px(10.0))
                                .text_color(rgb(t.text_secondary)),
                        ),
                )
            })
            .on_click(move |_, window, cx| {
                if let Some(handler) = handler.as_ref() {
                    cx.stop_propagation();
                    handler(&id, window, cx);
                }
            })
            .into_any_element()
    }
}

/// Every round button, so the row reads as one set.
const BUTTON_SIZE: f32 = 28.0;
const ICON_SIZE: f32 = 14.0;

impl RenderOnce for AgentLauncher {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme(cx);

        let header = h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(10.0))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(1.0))
                    .child(
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(t.text_primary))
                            .child(self.title.clone()),
                    )
                    .children(self.subtitle.clone().map(|subtitle| {
                        div()
                            .w_full()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(subtitle)
                    })),
            )
            .when(self.shows_buttons(), |d| d.child(self.render_buttons(cx)));

        let sessions: Vec<AnyElement> = self
            .sessions
            .iter()
            .map(|session| self.render_session(session, cx))
            .collect();

        v_flex()
            .id(self.id.clone())
            .w_full()
            .min_w_0()
            .gap(px(8.0))
            .when(self.style == LauncherStyle::Card, |d| {
                d.px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(rgb(t.border))
                    .bg(rgb(t.bg_secondary))
            })
            .child(header)
            .children(sessions)
    }
}
