//! Whether the exact diff of the selected file can be shown, and what to say
//! while it cannot.

use super::super::DiffViewer;
use super::super::review::{LoadState, ReviewFileKey};
use gpui::prelude::*;
use gpui::*;
use okena_core::theme::ThemeColors;
use okena_ui::tokens::{ui_text_ms, ui_text_sm};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SmartDiffViewState {
    Idle,
    Loading,
    Failed(String),
    Empty,
    NoSelection,
    SourceIdle,
    SourceLoading,
    SourceFailed(String),
    DisplayLoading,
    Ready,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiffPhase {
    Idle,
    Loading,
    Failed(String),
    Empty,
    Files,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SourcePhase {
    Idle,
    Loading,
    Failed(String),
    Ready,
}

fn classify_smart_diff_state(
    diff: DiffPhase,
    selected_in_diff: bool,
    source: SourcePhase,
    display_ready: bool,
) -> SmartDiffViewState {
    match diff {
        DiffPhase::Idle => SmartDiffViewState::Idle,
        DiffPhase::Loading => SmartDiffViewState::Loading,
        DiffPhase::Failed(error) => SmartDiffViewState::Failed(error),
        DiffPhase::Empty => SmartDiffViewState::Empty,
        DiffPhase::Files if !selected_in_diff => SmartDiffViewState::NoSelection,
        DiffPhase::Files => match source {
            SourcePhase::Idle => SmartDiffViewState::SourceIdle,
            SourcePhase::Loading => SmartDiffViewState::SourceLoading,
            SourcePhase::Failed(error) => SmartDiffViewState::SourceFailed(error),
            SourcePhase::Ready if !display_ready => SmartDiffViewState::DisplayLoading,
            SourcePhase::Ready => SmartDiffViewState::Ready,
        },
    }
}

fn exact_display_ready(
    canonical: Option<&ReviewFileKey>,
    file_key: Option<&ReviewFileKey>,
    source_matches: bool,
    cache_ready: bool,
    display_ready: bool,
) -> bool {
    canonical.is_some() && canonical == file_key && source_matches && cache_ready && display_ready
}

impl DiffViewer {
    pub(crate) fn smart_diff_view_state(&self) -> SmartDiffViewState {
        let (phase, dataset) = match &self.smart_review.diff {
            LoadState::Idle => (DiffPhase::Idle, None),
            LoadState::Loading => (DiffPhase::Loading, None),
            LoadState::Failed(error) => (DiffPhase::Failed(error.clone()), None),
            LoadState::Ready(dataset) if dataset.files.is_empty() => (DiffPhase::Empty, None),
            LoadState::Ready(dataset) => (DiffPhase::Files, Some(dataset)),
        };
        let selected_in_diff = dataset.is_some_and(|dataset| {
            self.smart_review.selected_file.as_ref().is_some_and(|key| {
                dataset
                    .files
                    .iter()
                    .any(|file| file.old_path == key.old_path && file.new_path == key.new_path)
            })
        });
        let canonical = self.smart_review.selected_file.as_ref();
        let source = match &self.smart_review.file.source {
            LoadState::Idle => SourcePhase::Idle,
            LoadState::Loading => SourcePhase::Loading,
            LoadState::Failed(error) => SourcePhase::Failed(error.clone()),
            LoadState::Ready(source)
                if canonical.is_none_or(|key| {
                    self.smart_review.file.key.as_ref() != Some(key) || !key.matches_source(source)
                }) =>
            {
                SourcePhase::Failed("Exact source does not match canonical selection".into())
            }
            LoadState::Ready(_) => SourcePhase::Ready,
        };
        let source_matches = matches!(
            &self.smart_review.file.source,
            LoadState::Ready(source)
                if canonical.is_some_and(|key| key.matches_source(source))
        );
        let cache_ready =
            canonical.is_some_and(|key| self.smart_review.file.has_ready_cache(key, true));
        let display_ready = exact_display_ready(
            canonical,
            self.smart_review.file.key.as_ref(),
            source_matches,
            cache_ready,
            self.current_file.is_some(),
        );
        classify_smart_diff_state(phase, selected_in_diff, source, display_ready)
    }

    pub(crate) fn render_smart_diff_state(
        &self,
        state: SmartDiffViewState,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, detail) = match state {
            SmartDiffViewState::Idle => ("Diff is not loaded", None),
            SmartDiffViewState::Loading => ("Loading exact diff\u{2026}", None),
            SmartDiffViewState::Failed(error) => ("Exact diff failed", Some(error)),
            SmartDiffViewState::Empty => ("No changed files", None),
            SmartDiffViewState::NoSelection => ("No exact file selected", None),
            SmartDiffViewState::SourceIdle => ("Exact source is not loaded", None),
            SmartDiffViewState::SourceLoading => ("Loading exact source\u{2026}", None),
            SmartDiffViewState::SourceFailed(error) => ("Exact source failed", Some(error)),
            SmartDiffViewState::DisplayLoading => ("Preparing diff display\u{2026}", None),
            SmartDiffViewState::Ready => return div().into_any_element(),
        };
        render_review_state(title, detail.as_deref(), t, cx)
    }

    pub(crate) fn render_navigation_unavailable(
        &self,
        t: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        self.review_navigation.unavailable.as_ref().map(|error| {
            div()
                .h(px(30.0))
                .px(px(16.0))
                .flex()
                .items_center()
                .border_b_1()
                .border_color(rgb(t.border))
                .text_size(ui_text_ms(cx))
                .text_color(rgb(t.term_yellow))
                .child(format!("Evidence unavailable: {error}"))
                .into_any_element()
        })
    }
}

fn render_review_state(
    title: &str,
    detail: Option<&str>,
    t: &ThemeColors,
    cx: &mut Context<DiffViewer>,
) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_size(ui_text_sm(cx))
                .text_color(rgb(t.text_muted))
                .child(title.to_string())
                .when_some(detail, |d, detail| {
                    d.child(
                        div()
                            .mt(px(4.0))
                            .text_size(ui_text_ms(cx))
                            .child(detail.to_string()),
                    )
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{
        DiffPhase, ReviewFileKey, SmartDiffViewState, SourcePhase, classify_smart_diff_state,
        exact_display_ready,
    };

    #[test]
    fn smart_diff_local_states_cover_dataset_source_and_display() {
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Idle, false, SourcePhase::Idle, false),
            SmartDiffViewState::Idle
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Loading, false, SourcePhase::Idle, false),
            SmartDiffViewState::Loading
        );
        assert_eq!(
            classify_smart_diff_state(
                DiffPhase::Failed("diff".into()),
                false,
                SourcePhase::Idle,
                false
            ),
            SmartDiffViewState::Failed("diff".into())
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Empty, false, SourcePhase::Idle, false),
            SmartDiffViewState::Empty
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, false, SourcePhase::Idle, false),
            SmartDiffViewState::NoSelection
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, true, SourcePhase::Loading, false),
            SmartDiffViewState::SourceLoading
        );
        assert_eq!(
            classify_smart_diff_state(
                DiffPhase::Files,
                true,
                SourcePhase::Failed("source".into()),
                false
            ),
            SmartDiffViewState::SourceFailed("source".into())
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, true, SourcePhase::Ready, false),
            SmartDiffViewState::DisplayLoading
        );
        assert_eq!(
            classify_smart_diff_state(DiffPhase::Files, true, SourcePhase::Ready, true),
            SmartDiffViewState::Ready
        );
    }

    #[test]
    fn exact_diff_ready_rejects_a_stale_file_key() {
        let canonical = ReviewFileKey {
            old_path: Some("a.rs".into()),
            new_path: Some("a.rs".into()),
        };
        let stale = ReviewFileKey {
            old_path: Some("b.rs".into()),
            new_path: Some("b.rs".into()),
        };
        assert!(!exact_display_ready(
            Some(&canonical),
            Some(&stale),
            true,
            true,
            true
        ));
        assert!(exact_display_ready(
            Some(&canonical),
            Some(&canonical),
            true,
            true,
            true
        ));
    }
}
