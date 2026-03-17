use crate::git;
use crate::workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use okena_core::api::ApiGitStatus;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::GitStatus;

/// How often to poll git status (seconds)
const GIT_POLL_INTERVAL: u64 = 5;
/// How many git poll cycles between PR URL checks (~60s)
const PR_POLL_EVERY_N_CYCLES: u64 = 12;

/// Centralized git status poller.
///
/// Polls git status for all visible (non-remote) projects every 5 seconds.
/// Polls PR URLs less frequently (~60 seconds).
/// Pushes changes to:
/// - Local UI via `cx.notify()` (ProjectColumn observes this entity)
/// - Remote clients via `tokio::sync::watch` channel (WS stream handler)
pub struct GitStatusWatcher {
    workspace: Entity<Workspace>,
    statuses: HashMap<String, Option<GitStatus>>,
    /// Cached PR info keyed by project ID
    pr_infos: HashMap<String, Option<super::PrInfo>>,
    /// Watch channel sender for remote WS push
    remote_tx: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
}

impl GitStatusWatcher {
    pub fn new(
        workspace: Entity<Workspace>,
        remote_tx: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut watcher = Self {
            workspace,
            statuses: HashMap::new(),
            pr_infos: HashMap::new(),
            remote_tx,
        };
        watcher.spawn_refresh(cx);
        watcher
    }

    /// Get cached git status for a project.
    pub fn get(&self, project_id: &str) -> Option<&GitStatus> {
        self.statuses.get(project_id).and_then(|s| s.as_ref())
    }

    /// Spawn the async polling loop.
    fn spawn_refresh(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut cycle: u64 = 0;
            loop {
                // Collect visible non-remote projects
                let projects: Vec<(String, String)> = cx.update(|cx| {
                    let ws = workspace.read(cx);
                    ws.visible_projects()
                        .iter()
                        .filter(|p| !p.is_remote)
                        .map(|p| (p.id.clone(), p.path.clone()))
                        .collect()
                });

                let check_prs = cycle % PR_POLL_EVERY_N_CYCLES == 0;

                // Phase 1: Fetch git status for all projects in parallel
                let status_futures: Vec<_> = projects.iter().map(|(id, path)| {
                    let id = id.clone();
                    let path = path.clone();
                    async move {
                        let status = smol::unblock(move || {
                            git::refresh_git_status(Path::new(&path))
                        }).await;
                        (id, status)
                    }
                }).collect();
                let mut new_statuses: HashMap<String, Option<GitStatus>> =
                    futures::future::join_all(status_futures).await.into_iter().collect();

                // Phase 2: Fetch PR info in parallel (slower, network calls) — only on PR poll cycles.
                // Runs after all statuses are updated so git status isn't delayed by PR checks.
                let new_pr_infos: HashMap<String, Option<super::PrInfo>> = if check_prs {
                    let pr_futures: Vec<_> = projects.iter().map(|(id, path)| {
                        let id = id.clone();
                        let path = path.clone();
                        async move {
                            let pr_info = smol::unblock(move || {
                                git::repository::get_pr_info(Path::new(&path))
                            }).await;
                            (id, pr_info)
                        }
                    }).collect();
                    futures::future::join_all(pr_futures).await.into_iter().collect()
                } else {
                    HashMap::new()
                };

                // Compare and update
                let should_continue = this.update(cx, |this, cx| {
                    // Merge PR info: update cache on PR poll cycles, keep old values otherwise
                    if check_prs {
                        this.pr_infos = new_pr_infos;
                    }

                    // Inject cached PR info into statuses
                    for (id, status) in new_statuses.iter_mut() {
                        if let Some(Some(status)) = status.as_mut().map(Some) {
                            status.pr_info = this.pr_infos.get(id).cloned().flatten();
                        }
                    }

                    let changed = this.statuses != new_statuses;
                    if changed {
                        this.statuses = new_statuses;
                        cx.notify();

                        // Push to remote watch channel
                        let api_statuses: HashMap<String, ApiGitStatus> = this.statuses.iter()
                            .filter_map(|(id, status)| {
                                status.as_ref().map(|s| (id.clone(), ApiGitStatus {
                                    branch: s.branch.clone(),
                                    lines_added: s.lines_added,
                                    lines_removed: s.lines_removed,
                                }))
                            })
                            .collect();
                        this.remote_tx.send_modify(|current| {
                            *current = api_statuses;
                        });
                    }
                    true
                }).unwrap_or(false);

                if !should_continue {
                    break;
                }

                cycle += 1;
                smol::Timer::after(Duration::from_secs(GIT_POLL_INTERVAL)).await;
            }
        }).detach();
    }
}
