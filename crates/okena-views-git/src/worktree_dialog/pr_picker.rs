//! "From PR" tab of the worktree dialog: a filter input over open pull
//! requests. Typing filters the newest PRs instantly, then — after a short
//! pause — searches GitHub, which reaches PRs older than the first page
//! (including a direct lookup when the text is a PR number).

use std::time::Duration;

use okena_core::api::{ActionRequest, WorktreePullRequest};
use okena_core::theme::ThemeColors;
use okena_files::theme::theme;
use okena_transport::remote_action::RemoteActionClient;
use okena_ui::input::input_container;
use okena_ui::tokens::{ui_text_md, ui_text_ms};

use super::list_selection::ListSelection;
use crate::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};

use gpui::prelude::*;
use gpui::*;
use gpui_component::h_flex;

/// How many PRs one request returns.
const PAGE_SIZE: usize = 20;
/// Pause after the last keystroke before GitHub is searched.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(300);

/// The newest open PRs, fetched once when the tab is first shown.
enum BaseList {
    NotRequested,
    Loading,
    Loaded(Vec<WorktreePullRequest>),
    Failed(String),
}

/// A finished GitHub search, tagged with the query it answers so a result for
/// text the user has since changed is never shown.
struct SearchResult {
    query: String,
    result: Result<Vec<WorktreePullRequest>, String>,
}

pub(super) struct PrPicker {
    client: RemoteActionClient,
    project_id: String,
    pub(super) input: Entity<SimpleInputState>,
    base: BaseList,
    search: Option<SearchResult>,
    /// The pending debounce + request for the current query. Replacing it
    /// drops (cancels) the previous one, so only the latest query lands.
    search_task: Option<Task<()>>,
    selection: ListSelection<WorktreePullRequest, u32>,
    _input_subscription: Subscription,
}

impl PrPicker {
    pub(super) fn new(
        client: RemoteActionClient,
        project_id: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("Filter by #number, title or branch...")
                .icon("icons/search.svg")
        });
        let input_subscription = cx.subscribe(&input, |this, _, _: &InputChangedEvent, cx| {
            this.query_changed(cx);
        });
        Self {
            client,
            project_id,
            input,
            base: BaseList::NotRequested,
            search: None,
            search_task: None,
            selection: ListSelection::new(|pr: &WorktreePullRequest| pr.number),
            _input_subscription: input_subscription,
        }
    }

    /// Fetch the newest PRs the first time the tab is shown.
    pub(super) fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.base, BaseList::NotRequested) {
            return;
        }
        self.base = BaseList::Loading;
        cx.notify();
        let request = self.request(String::new());
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(request).await;
            let _ = this.update(cx, |this, cx| {
                this.base = match result {
                    Ok(prs) => BaseList::Loaded(prs),
                    Err(error) => BaseList::Failed(error),
                };
                this.refresh_items(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The branch to create the worktree from, or why there is none yet.
    pub(super) fn picked_branch(&self) -> Result<String, &'static str> {
        match self.selection.selected() {
            Some(pr) => Ok(pr.branch.clone()),
            None if self.search_task.is_some() => {
                Err("Still searching GitHub — wait for results or pick a pull request")
            }
            None => Err("Please select a pull request"),
        }
    }

    pub(super) fn move_up(&mut self, cx: &mut Context<Self>) {
        self.selection.move_up();
        cx.notify();
    }

    pub(super) fn move_down(&mut self, cx: &mut Context<Self>) {
        self.selection.move_down();
        cx.notify();
    }

    fn query(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    /// New text: show local matches right away (nothing auto-selected yet, so
    /// Enter can't grab a local prefix match while the real hit is still on
    /// its way), then search GitHub once typing pauses.
    fn query_changed(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.selection.clear();
        self.search_task = None;
        if query.is_empty() {
            self.search = None;
        } else {
            let request = self.request(query.clone());
            self.search_task = Some(cx.spawn(async move |this, cx| {
                smol::Timer::after(SEARCH_DEBOUNCE).await;
                let result = smol::unblock(request).await;
                let _ = this.update(cx, |this, cx| {
                    this.search = Some(SearchResult { query, result });
                    this.search_task = None;
                    this.refresh_items(cx);
                    if !this.selection.has_selection() {
                        this.selection.select_first();
                    }
                    cx.notify();
                });
            }));
        }
        self.refresh_items(cx);
        cx.notify();
    }

    fn refresh_items(&mut self, cx: &App) {
        let query = self.query(cx);
        let base = match &self.base {
            BaseList::Loaded(prs) => prs.as_slice(),
            _ => &[],
        };
        let found = self
            .search
            .as_ref()
            .filter(|search| search.query == query)
            .and_then(|search| search.result.as_ref().ok())
            .map(Vec::as_slice);
        self.selection.set_items(shown_prs(&query, base, found));
    }

    /// A blocking daemon call listing PRs for `query`, to run off the UI thread.
    fn request(
        &self,
        query: String,
    ) -> impl FnOnce() -> Result<Vec<WorktreePullRequest>, String> + Send + 'static {
        let client = self.client.clone();
        let project_id = self.project_id.clone();
        move || {
            client
                .post_action(ActionRequest::GitListPullRequests {
                    project_id,
                    limit: PAGE_SIZE,
                    query,
                })
                .and_then(|value| value.ok_or_else(|| "Missing pull request list".to_string()))
                .and_then(|value| {
                    serde_json::from_value::<Vec<WorktreePullRequest>>(value)
                        .map_err(|error| format!("Invalid pull request list: {error}"))
                })
        }
    }

    /// The line shown under (or instead of) the list, if any.
    fn status_line(&self, query: &str) -> Option<String> {
        if self.search_task.is_some() {
            return Some("Searching GitHub…".to_string());
        }
        if let Some(SearchResult {
            result: Err(error), ..
        }) = self.search.as_ref().filter(|search| search.query == query)
        {
            return Some(error.clone());
        }
        if !self.selection.items().is_empty() {
            return None;
        }
        Some(match (&self.base, query.is_empty()) {
            (BaseList::NotRequested | BaseList::Loading, _) => "Loading PRs...".to_string(),
            (BaseList::Failed(error), true) => error.clone(),
            (_, true) => "No open pull requests".to_string(),
            (_, false) => "No open pull requests match".to_string(),
        })
    }

    fn render_row(
        &self,
        pr: &WorktreePullRequest,
        t: ThemeColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let number = pr.number;
        div()
            .id(ElementId::Name(format!("pr-{number}").into()))
            .px(px(12.0))
            .py(px(6.0))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .cursor_pointer()
            .when(self.selection.is_selected(pr), |d| {
                d.bg(rgb(t.bg_selection))
            })
            .hover(|s| s.bg(rgb(t.bg_hover)))
            .on_click(cx.listener(move |this, _, _window, cx| {
                this.selection.select(number);
                cx.notify();
            }))
            .child(
                h_flex()
                    .gap(px(6.0))
                    .items_center()
                    .child(
                        div()
                            .text_size(ui_text_ms(cx))
                            .text_color(rgb(t.text_muted))
                            .child(format!("#{number}")),
                    )
                    .child(
                        div()
                            .text_size(ui_text_md(cx))
                            .text_color(rgb(t.text_primary))
                            .flex_1()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .child(pr.title.clone()),
                    ),
            )
            .child(
                div()
                    .pl(px(28.0))
                    .text_size(ui_text_ms(cx))
                    .text_color(rgb(t.text_muted))
                    .child(pr.branch.clone()),
            )
    }
}

/// What the list shows for `query`: GitHub's results for it (when they have
/// arrived) followed by any of the newest PRs that match locally but that
/// search missed (search ignores branch names); before results arrive, just
/// the local matches.
fn shown_prs(
    query: &str,
    base: &[WorktreePullRequest],
    found: Option<&[WorktreePullRequest]>,
) -> Vec<WorktreePullRequest> {
    let found = found.unwrap_or_default();
    let local = base
        .iter()
        .filter(|pr| matches_locally(pr, query))
        .filter(|pr| !found.iter().any(|hit| hit.number == pr.number));
    found.iter().chain(local).cloned().collect()
}

/// Case-insensitive: a number (with or without `#`) matches as a prefix of the
/// PR number; any text matches inside the title or branch.
fn matches_locally(pr: &WorktreePullRequest, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let digits = query.strip_prefix('#').unwrap_or(query);
    if !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && pr.number.to_string().starts_with(digits)
    {
        return true;
    }
    let query = query.to_lowercase();
    pr.title.to_lowercase().contains(&query) || pr.branch.to_lowercase().contains(&query)
}

impl Render for PrPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let query = self.query(cx);
        let input_focused = self.input.read(cx).focus_handle(cx).is_focused(window);
        let status = self.status_line(&query);
        let rows: Vec<AnyElement> = self
            .selection
            .items()
            .iter()
            .map(|pr| self.render_row(pr, t, cx).into_any_element())
            .collect();

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                input_container(&t, Some(input_focused))
                    .child(SimpleInput::new(&self.input).text_size(ui_text_md(cx))),
            )
            .child(
                div()
                    .id("pr-list-scroll")
                    .flex()
                    .flex_col()
                    .max_h(px(200.0))
                    .overflow_y_scroll()
                    .children(rows)
                    .when_some(status, |d, status| {
                        d.child(
                            div()
                                .p(px(12.0))
                                .text_size(ui_text_md(cx))
                                .text_color(rgb(t.text_muted))
                                .child(status),
                        )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{matches_locally, shown_prs};
    use okena_core::api::WorktreePullRequest;

    fn pr(number: u32, title: &str, branch: &str) -> WorktreePullRequest {
        WorktreePullRequest {
            number,
            title: title.into(),
            branch: branch.into(),
        }
    }

    fn numbers(prs: &[WorktreePullRequest]) -> Vec<u32> {
        prs.iter().map(|pr| pr.number).collect()
    }

    #[test]
    fn number_matches_as_prefix_with_or_without_hash() {
        let item = pr(1234, "Fix login", "fix/login");
        assert!(matches_locally(&item, "12"));
        assert!(matches_locally(&item, "#123"));
        assert!(!matches_locally(&item, "234"));
        assert!(!matches_locally(&item, "#9"));
    }

    #[test]
    fn text_matches_title_or_branch_ignoring_case() {
        let item = pr(7, "Fix Login redirect", "feat/oauth-v2");
        assert!(matches_locally(&item, "login"));
        assert!(matches_locally(&item, "OAUTH"));
        assert!(matches_locally(&item, "v2"));
        assert!(!matches_locally(&item, "logout"));
    }

    #[test]
    fn digits_also_match_inside_titles() {
        assert!(matches_locally(&pr(7, "Release 2026", "release"), "2026"));
    }

    #[test]
    fn empty_query_shows_the_whole_base_list() {
        let base = [pr(3, "a", "a"), pr(2, "b", "b")];
        assert_eq!(numbers(&shown_prs("", &base, None)), vec![3, 2]);
    }

    #[test]
    fn before_search_results_only_local_matches_show() {
        let base = [pr(120, "a", "a"), pr(99, "b", "b")];
        assert_eq!(numbers(&shown_prs("12", &base, None)), vec![120]);
    }

    #[test]
    fn search_results_lead_and_local_only_matches_follow_without_duplicates() {
        let base = [
            pr(120, "a", "a"),
            pr(50, "x", "branch-12"),
            pr(99, "b", "b"),
        ];
        let found = [pr(12, "old", "old"), pr(120, "a", "a")];
        assert_eq!(
            numbers(&shown_prs("12", &base, Some(&found))),
            vec![12, 120, 50]
        );
    }
}
