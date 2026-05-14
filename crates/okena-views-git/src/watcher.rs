use okena_git::{self as git, GitStatus};
use okena_workspace::state::Workspace;
use gpui::prelude::*;
use gpui::*;
use okena_core::api::ApiGitStatus;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// How often to poll git status (seconds)
const GIT_POLL_INTERVAL: u64 = 5;
/// How many git poll cycles between PR URL checks (~60s)
const PR_POLL_EVERY_N_CYCLES: u64 = 12;
/// How many git poll cycles between CI check polls when checks are pending (~15s)
const CI_PENDING_POLL_EVERY_N_CYCLES: u64 = 3;
/// How many git poll cycles between CI check polls when checks are settled (~60s)
const CI_SETTLED_POLL_EVERY_N_CYCLES: u64 = 12;

/// Centralized git status poller.
///
/// Polls git status for all locally visible and remotely subscribed (non-remote) projects every 5 seconds.
/// Polls PR URLs less frequently (~60 seconds).
/// Pushes changes to:
/// - Local UI via `cx.notify()` (ProjectColumn observes this entity)
/// - Remote clients via `tokio::sync::watch` channel (WS stream handler)
pub struct GitStatusWatcher {
    workspace: Entity<Workspace>,
    statuses: HashMap<String, Option<GitStatus>>,
    /// Cached PR info keyed by project ID
    pr_infos: HashMap<String, Option<okena_git::PrInfo>>,
    /// Cached CI check status keyed by project ID
    ci_checks: HashMap<String, Option<okena_git::CiCheckSummary>>,
    /// Whether any project has pending CI checks (drives adaptive polling)
    any_pending_ci: bool,
    /// Watch channel sender for remote WS push
    remote_tx: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
    /// Per-connection set of subscribed terminal IDs from remote clients
    remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
}

impl GitStatusWatcher {
    pub fn new(
        workspace: Entity<Workspace>,
        remote_tx: Arc<tokio::sync::watch::Sender<HashMap<String, ApiGitStatus>>>,
        remote_subscribed_terminals: Arc<RwLock<HashMap<u64, HashSet<String>>>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut watcher = Self {
            workspace,
            statuses: HashMap::new(),
            pr_infos: HashMap::new(),
            ci_checks: HashMap::new(),
            any_pending_ci: false,
            remote_tx,
            remote_subscribed_terminals,
        };
        watcher.spawn_branch_warmup(cx);
        watcher.spawn_refresh(cx);
        watcher
    }

    /// One-shot branch-only warmup for ALL non-remote projects, so consumers
    /// that read the global git cache (project switcher, sidebar worktree
    /// names, ...) see a branch for projects that aren't currently visible
    /// and therefore aren't polled by the steady-state loop.
    fn spawn_branch_warmup(&self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.spawn(async move |_, cx| {
            let paths: Vec<String> = cx.update(|cx| {
                workspace.read(cx).projects().iter()
                    .filter(|p| !p.is_remote)
                    .map(|p| p.path.clone())
                    .collect()
            });

            let futures = paths.into_iter().map(|path| {
                smol::unblock(move || git::warm_branch_cache(Path::new(&path)))
            });
            futures::future::join_all(futures).await;
        }).detach();
    }

    /// Get cached git status for a project.
    pub fn get(&self, project_id: &str) -> Option<&GitStatus> {
        self.statuses.get(project_id).and_then(|s| s.as_ref())
    }

    /// Spawn the async polling loop.
    fn spawn_refresh(&mut self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        let remote_subscribed_terminals = self.remote_subscribed_terminals.clone();

        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut cycle: u64 = 0;
            loop {
                // Collect locally visible + remotely subscribed non-remote projects.
                //
                // Multi-window: previously we restricted to main's visible set,
                // but a project may be shown ONLY in an extra (per PRD rule
                // 3b-ii, new projects added from window N are hidden in all
                // other windows including main). Polling only main's visible
                // projects left those extras without branch / diff-stat data.
                // Poll the full local project list — git status is fast and
                // bounded, and any window that ever surfaces a project gets
                // its data.
                let projects: Vec<(String, String)> = cx.update(|cx| {
                    let ws = workspace.read(cx);

                    let mut project_ids: HashSet<String> = ws.projects()
                        .iter()
                        .filter(|p| !p.is_remote)
                        .map(|p| p.id.clone())
                        .collect();

                    // Add projects with remotely subscribed terminals
                    if let Ok(remote_terminals) = remote_subscribed_terminals.read() {
                        for terminal_ids in remote_terminals.values() {
                            for tid in terminal_ids {
                                if let Some(p) = ws.find_project_for_terminal(tid) {
                                    if !p.is_remote {
                                        project_ids.insert(p.id.clone());
                                    }
                                }
                            }
                        }
                    }

                    // Resolve to (id, path) pairs
                    ws.projects()
                        .iter()
                        .filter(|p| project_ids.contains(&p.id))
                        .map(|p| (p.id.clone(), p.path.clone()))
                        .collect()
                });

                let check_prs = cycle % PR_POLL_EVERY_N_CYCLES == 0;
                let ci_poll_interval = if this.update(cx, |this, _| this.any_pending_ci).unwrap_or(false) {
                    CI_PENDING_POLL_EVERY_N_CYCLES
                } else {
                    CI_SETTLED_POLL_EVERY_N_CYCLES
                };
                let check_ci = cycle % ci_poll_interval == 0;

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
                let new_pr_infos: HashMap<String, Option<okena_git::PrInfo>> = if check_prs {
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

                // Phase 3: Fetch CI check status — adaptive interval based on pending state.
                // Only for projects that have a known PR.
                let new_ci_checks: HashMap<String, Option<okena_git::CiCheckSummary>> = if check_ci {
                    let pr_infos_snapshot: HashMap<String, Option<okena_git::PrInfo>> = if check_prs {
                        // Use freshly fetched PR info
                        new_pr_infos.clone()
                    } else {
                        // Use cached PR info
                        this.update(cx, |this, _| this.pr_infos.clone()).unwrap_or_default()
                    };
                    let ci_futures: Vec<_> = projects.iter()
                        .filter(|(id, _)| pr_infos_snapshot.get(id).map(|p| p.is_some()).unwrap_or(false))
                        .map(|(id, path)| {
                            let id = id.clone();
                            let path = path.clone();
                            async move {
                                let checks = smol::unblock(move || {
                                    git::repository::get_ci_checks(Path::new(&path))
                                }).await;
                                (id, checks)
                            }
                        }).collect();
                    futures::future::join_all(ci_futures).await.into_iter().collect()
                } else {
                    HashMap::new()
                };

                // Compare and update
                let should_continue = this.update(cx, |this, cx| {
                    // Merge PR info: update cache on PR poll cycles, keep old values otherwise
                    if check_prs {
                        this.pr_infos = new_pr_infos;
                    }

                    // Merge CI checks: update cache on CI poll cycles
                    if check_ci {
                        for (id, checks) in new_ci_checks {
                            this.ci_checks.insert(id, checks);
                        }
                        this.any_pending_ci = this.ci_checks.values()
                            .any(|c| c.as_ref().map(|s| s.status.is_pending()).unwrap_or(false));
                    }

                    // Inject cached PR info + CI checks into statuses
                    for (id, status) in new_statuses.iter_mut() {
                        if let Some(Some(status)) = status.as_mut().map(Some) {
                            if let Some(mut pr) = this.pr_infos.get(id).cloned().flatten() {
                                pr.ci_checks = this.ci_checks.get(id).cloned().flatten();
                                status.pr_info = Some(pr);
                            }
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
