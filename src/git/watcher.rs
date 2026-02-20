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

/// Centralized git status poller.
///
/// Polls git status for all visible (non-remote) projects every 5 seconds.
/// Pushes changes to:
/// - Local UI via `cx.notify()` (ProjectColumn observes this entity)
/// - Remote clients via `tokio::sync::watch` channel (WS stream handler)
pub struct GitStatusWatcher {
    workspace: Entity<Workspace>,
    statuses: HashMap<String, Option<GitStatus>>,
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

                // Fetch git status for each project (on blocking thread)
                let mut new_statuses: HashMap<String, Option<GitStatus>> = HashMap::new();
                for (id, path) in &projects {
                    let path = path.clone();
                    let status = smol::unblock(move || {
                        git::get_git_status(Path::new(&path))
                    }).await;
                    new_statuses.insert(id.clone(), status);
                }

                // Compare and update
                let should_continue = this.update(cx, |this, cx| {
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

                smol::Timer::after(Duration::from_secs(5)).await;
            }
        }).detach();
    }
}
