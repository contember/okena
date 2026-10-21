//! What's New: the published release notes, rendered in the app.
//!
//! The notes come from GitHub Releases through the local daemon
//! (`daemon_client::fetch_release_notes`), newest first, up to this desktop's
//! version — the same one the post-update toast names. Each body is Markdown and is parsed once; fenced code blocks are
//! re-highlighted when the theme flips between dark and light.

use crate::keybindings::Cancel;
use crate::theme::{ThemeColors, theme};
use crate::ui::tokens::{ui_text_md, ui_text_ms, ui_text_sm};
use crate::views::components::{modal_backdrop, modal_content, modal_header};
use gpui::prelude::*;
use gpui::*;
use okena_ext_updater::{ReleaseNote, ReleaseNotes};
use okena_markdown::{MarkdownDocument, RenderedNode};
use okena_ui::code_block::code_block_container;

pub enum WhatsNewEvent {
    Close,
}

/// One release, parsed and ready to draw.
struct Section {
    note: ReleaseNote,
    header: String,
    doc: MarkdownDocument,
}

/// One row of the virtualised list: a release header or one Markdown
/// block of a release.
#[derive(Clone, Copy)]
enum Row {
    Header(usize),
    Block { section: usize, node: usize },
}

enum NotesState {
    Loading,
    Loaded {
        current_version: String,
        sections: Vec<Section>,
        rows: Vec<Row>,
        list: ListState,
    },
    Failed(String),
}

pub struct WhatsNewModal {
    focus_handle: FocusHandle,
    state: NotesState,
    /// Which theme the code blocks were last highlighted for.
    highlighted_dark: Option<bool>,
}

impl WhatsNewModal {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self::idle(cx);
        this.load(cx);
        this
    }

    /// A modal that does not talk to the daemon — tests feed it with
    /// [`Self::apply`].
    fn idle(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            state: NotesState::Loading,
            highlighted_dark: None,
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(running) = cx
            .try_global::<okena_ext_updater::GlobalUpdateInfo>()
            .map(|global| global.0.app_version())
        else {
            self.apply(Err("The running version is unknown".to_string()), cx);
            return;
        };
        self.state = NotesState::Loading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                okena_ext_updater::daemon_client::fetch_release_notes(&running)
            })
            .await
            .map_err(|error| error.to_string());
            let _ = this.update(cx, |this, cx| this.apply(result, cx));
        })
        .detach();
    }

    /// Take the daemon's answer (or its failure).
    pub fn apply(&mut self, result: Result<ReleaseNotes, String>, cx: &mut Context<Self>) {
        self.state = match result {
            Ok(notes) => {
                let sections: Vec<Section> = notes
                    .releases
                    .into_iter()
                    .map(|note| Section {
                        header: release_header(&note.version, &note.published_at),
                        doc: MarkdownDocument::parse(&note.body),
                        note,
                    })
                    .collect();
                let rows: Vec<Row> = sections
                    .iter()
                    .enumerate()
                    .flat_map(|(section, s)| {
                        std::iter::once(Row::Header(section)).chain(
                            (0..s.doc.node_count()).map(move |node| Row::Block { section, node }),
                        )
                    })
                    .collect();
                // Only the rows on screen (plus the overdraw) are laid out per
                // frame; twenty releases of Markdown would otherwise be
                // re-measured on every scroll tick.
                let list = ListState::new(rows.len(), ListAlignment::Top, px(400.0));
                NotesState::Loaded {
                    current_version: notes.current_version,
                    sections,
                    rows,
                    list,
                }
            }
            Err(error) => NotesState::Failed(error),
        };
        // Freshly parsed documents carry no highlighting yet.
        self.highlighted_dark = None;
        cx.notify();
    }

    /// The headers shown, newest first; empty while loading or failed.
    pub fn section_headers(&self) -> Vec<String> {
        match &self.state {
            NotesState::Loaded { sections, .. } => {
                sections.iter().map(|s| s.header.clone()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The error the failed state shows, with its Retry button.
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            NotesState::Failed(error) => Some(error.as_str()),
            _ => None,
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(WhatsNewEvent::Close);
    }

    fn ensure_highlighted(&mut self, is_dark: bool) {
        if self.highlighted_dark == Some(is_dark) {
            return;
        }
        if let NotesState::Loaded { sections, .. } = &mut self.state {
            for section in sections.iter_mut() {
                section.doc.highlight_code_blocks(is_dark);
            }
        }
        self.highlighted_dark = Some(is_dark);
    }

    fn render_block(doc: &MarkdownDocument, idx: usize, t: &ThemeColors, cx: &App) -> AnyElement {
        let (above, below) = doc.node_spacing(idx);
        let body = match doc.render_node(idx, t, cx, None) {
            Some(RenderedNode::Simple { div, .. }) => div.into_any_element(),
            Some(RenderedNode::CodeBlock { language, lines }) => {
                code_block_container(language.as_deref(), t, cx)
                    .child(
                        div()
                            .px(px(14.0))
                            .py(px(10.0))
                            .flex()
                            .flex_col()
                            .children(lines.into_iter().map(|line| line.div)),
                    )
                    .into_any_element()
            }
            Some(RenderedNode::Table { header, rows }) => div()
                .flex()
                .flex_col()
                .children(header.map(|h| h.div))
                .children(rows.into_iter().map(|row| row.div))
                .into_any_element(),
            None => div().into_any_element(),
        };
        div()
            .px(px(20.0))
            .pt(above)
            .pb(below)
            .child(body)
            .into_any_element()
    }

    /// One list row, by index into `rows`.
    fn render_row(&self, idx: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let NotesState::Loaded { sections, rows, .. } = &self.state else {
            return div().into_any_element();
        };
        match rows.get(idx) {
            Some(Row::Header(section)) => {
                self.render_section_header(*section, &sections[*section], &t, cx)
            }
            Some(Row::Block { section, node }) => {
                Self::render_block(&sections[*section].doc, *node, &t, cx)
            }
            None => div().into_any_element(),
        }
    }

    fn render_section_header(
        &self,
        index: usize,
        section: &Section,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let url = section.note.release_url.clone();
        let link = div()
            .id(ElementId::Name(format!("whats-new-github-{index}").into()))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(5.0))
            .text_size(ui_text_sm(cx))
            .text_color(rgb(t.text_muted))
            .hover(|style| style.text_color(rgb(t.text_primary)))
            .child("Open on GitHub")
            .child(
                svg()
                    .path("icons/external-link.svg")
                    .size(px(12.0))
                    .text_color(rgb(t.text_muted)),
            )
            .on_click(move |_, _, _| okena_core::process::open_url(&url));

        div()
            .px(px(20.0))
            .pt(px(16.0))
            .pb(px(4.0))
            .when(index > 0, |d| d.mt(px(16.0)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.0))
                    .when(index > 0, |d| {
                        d.pt(px(20.0)).border_t_1().border_color(rgb(t.border))
                    })
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child(section.header.clone()),
                    )
                    .when(!section.note.release_url.is_empty(), |d| d.child(link)),
            )
            .into_any_element()
    }

    fn render_message(
        &self,
        text: impl Into<SharedString>,
        t: &ThemeColors,
        cx: &App,
    ) -> AnyElement {
        div()
            .py(px(32.0))
            .flex()
            .justify_center()
            .text_size(ui_text_md(cx))
            .text_color(rgb(t.text_muted))
            .child(text.into())
            .into_any_element()
    }

    fn render_failure(&self, error: &str, t: &ThemeColors, cx: &mut Context<Self>) -> AnyElement {
        div()
            .py(px(24.0))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .text_size(ui_text_md(cx))
                    .text_color(rgb(t.text_primary))
                    .child("Couldn't load the release notes"),
            )
            .child(
                div()
                    .max_w(px(460.0))
                    .text_size(ui_text_sm(cx))
                    .text_color(rgb(t.text_muted))
                    .child(error.to_string()),
            )
            .child(
                div()
                    .id("whats-new-retry")
                    .cursor_pointer()
                    .px(px(10.0))
                    .h(px(28.0))
                    .flex()
                    .items_center()
                    .rounded(px(4.0))
                    .bg(rgb(t.bg_hover))
                    .hover(|style| style.bg(rgb(t.border)))
                    .text_size(ui_text_ms(cx))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(t.text_primary))
                    .child("Retry")
                    .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
            )
            .into_any_element()
    }
}

/// `v0.34.0 · 9 Oct 2026`, or just `v0.34.0` when the date is not an ISO
/// timestamp GitHub would send.
pub fn release_header(version: &str, published_at: &str) -> String {
    match short_date(published_at) {
        Some(date) => format!("v{version} · {date}"),
        None => format!("v{version}"),
    }
}

fn short_date(published_at: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = published_at.get(..10)?;
    let mut parts = date.split('-');
    let year: u32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(format!("{day} {} {year}", MONTHS[month - 1]))
}

impl EventEmitter<WhatsNewEvent> for WhatsNewModal {}

impl Render for WhatsNewModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let focus_handle = self.focus_handle.clone();
        if !focus_handle.is_focused(window) {
            window.focus(&focus_handle, cx);
        }
        self.ensure_highlighted(t.is_dark());

        let subtitle = match &self.state {
            NotesState::Loaded {
                current_version, ..
            } => format!("You're running Okena v{current_version}"),
            _ => "Release notes for recent Okena versions".to_string(),
        };

        let max_height = window.viewport_size().height * 0.7;

        // A list needs a definite height to scroll in, so the modal takes
        // its full height once there is something to scroll; the short
        // states stay as tall as their message.
        let (body, fixed_height): (AnyElement, bool) = match &self.state {
            NotesState::Loading => (self.render_message("Loading release notes…", &t, cx), false),
            NotesState::Failed(error) => (self.render_failure(error, &t, cx), false),
            NotesState::Loaded { sections, .. } if sections.is_empty() => {
                (self.render_message("No release notes yet", &t, cx), false)
            }
            NotesState::Loaded {
                list: list_state, ..
            } => {
                let view = cx.entity().clone();
                let rows = list(list_state.clone(), move |idx, _window, cx| {
                    view.update(cx, |this, cx| this.render_row(idx, cx))
                })
                .size_full()
                .into_any_element();
                (rows, true)
            }
        };

        modal_backdrop("whats-new-backdrop", &t)
            .track_focus(&focus_handle)
            .key_context("WhatsNewModal")
            .items_center()
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.close(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.close(cx)),
            )
            .child(
                modal_content("whats-new-modal", &t)
                    .w(px(560.0))
                    .max_h(max_height)
                    .when(fixed_height, |d| d.h(max_height))
                    .child(modal_header(
                        "What's New",
                        Some(subtitle),
                        &t,
                        cx,
                        cx.listener(|this, _, _, cx| this.close(cx)),
                    ))
                    .child(div().flex_1().min_h_0().pb(px(8.0)).child(body)),
            )
    }
}

impl_focusable!(WhatsNewModal);

#[cfg(test)]
mod tests {
    // Named imports: `use super::*` would pull in gpui's `test` macro and
    // shadow the one these tests need.
    use super::{WhatsNewModal, release_header};
    use gpui::AppContext as _;
    use okena_ext_updater::{ReleaseNote, ReleaseNotes};

    #[test]
    fn header_shows_the_version_and_a_short_date() {
        assert_eq!(
            release_header("0.34.0", "2026-10-09T15:49:55Z"),
            "v0.34.0 · 9 Oct 2026"
        );
        assert_eq!(
            release_header("0.33.0", "2026-01-31T00:00:00Z"),
            "v0.33.0 · 31 Jan 2026"
        );
    }

    #[test]
    fn header_falls_back_to_the_version_on_garbage() {
        assert_eq!(release_header("0.34.0", ""), "v0.34.0");
        assert_eq!(release_header("0.34.0", "yesterday"), "v0.34.0");
        assert_eq!(release_header("0.34.0", "2026-13-01T00:00:00Z"), "v0.34.0");
    }

    fn note(version: &str, body: &str) -> ReleaseNote {
        ReleaseNote {
            version: version.to_string(),
            published_at: "2026-10-09T15:49:55Z".to_string(),
            release_url: format!("https://github.com/contember/okena/releases/tag/v{version}"),
            body: body.to_string(),
        }
    }

    #[gpui::test]
    fn loaded_notes_become_sections_in_the_daemon_order(cx: &mut gpui::TestAppContext) {
        let modal = cx.new(WhatsNewModal::idle);
        modal.update(cx, |modal, cx| {
            modal.apply(
                Ok(ReleaseNotes {
                    current_version: "0.35.0".to_string(),
                    releases: vec![
                        note("0.35.0", "## What's new\n\n- lanes"),
                        note("0.34.0", "## Fixes\n\n- colours"),
                    ],
                }),
                cx,
            );
        });
        modal.read_with(cx, |modal, _| {
            assert_eq!(
                modal.section_headers(),
                ["v0.35.0 · 9 Oct 2026", "v0.34.0 · 9 Oct 2026"]
            );
            assert_eq!(modal.error(), None);
        });
    }

    #[gpui::test]
    fn a_failed_load_keeps_the_error_for_the_retry_screen(cx: &mut gpui::TestAppContext) {
        let modal = cx.new(WhatsNewModal::idle);
        modal.update(cx, |modal, cx| {
            modal.apply(Err("daemon away".to_string()), cx)
        });
        modal.read_with(cx, |modal, _| {
            assert_eq!(modal.error(), Some("daemon away"));
            assert!(modal.section_headers().is_empty());
        });
    }
}
