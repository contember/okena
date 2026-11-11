//! Bulk presentation changes; never dispatch terminal or daemon actions.
use super::{GroupKind, Sidebar, SidebarCursorItem, SidebarProjectInfo};
use gpui::Context;
use okena_workspace::state::{ProjectData, WorkspaceData};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

/// Use cached terminal flags only, matching the activity ordering without OS process reads.
pub(super) fn terminal_activity(
    ids: &[String],
    terminals: &HashMap<String, Arc<okena_terminal::terminal::Terminal>>,
) -> (bool, bool) {
    ids.iter()
        .filter_map(|id| terminals.get(id))
        .fold((false, false), |(attention, running), t| {
            (
                attention || t.has_bell() || t.has_notification(),
                running || (t.had_user_input() && !t.is_waiting_for_input()),
            )
        })
}

fn folder_for<'a>(data: &'a WorkspaceData, project: &ProjectData) -> Option<&'a str> {
    let parent = project
        .worktree_info
        .as_ref()
        .map(|w| w.parent_project_id.as_str());
    data.folders
        .iter()
        .find(|f| {
            f.project_ids
                .iter()
                .any(|id| id == &project.id || Some(id.as_str()) == parent)
        })
        .map(|f| f.id.as_str())
}

fn selected(
    pinned: bool,
    attention: bool,
    running: bool,
    folder: Option<&str>,
    filter: Option<&str>,
) -> bool {
    (pinned || attention || running) && filter.is_none_or(|f| folder == Some(f))
}

fn restore_index(item: &SidebarCursorItem, items: &[SidebarCursorItem]) -> Option<usize> {
    items.iter().position(|candidate| candidate == item).or_else(|| {
        let id = match item {
            SidebarCursorItem::GroupHeader { project_id, .. } |
            SidebarCursorItem::Terminal { project_id, .. } |
            SidebarCursorItem::Service { project_id, .. } |
            SidebarCursorItem::Hook { project_id, .. } => project_id,
            _ => return None,
        };
        // Prefer the main checkout row over its family header.
        items.iter().position(|candidate| matches!(candidate, SidebarCursorItem::WorktreeProject { project_id } if project_id == id))
            .or_else(|| items.iter().position(|candidate| matches!(candidate, SidebarCursorItem::Project { project_id } if project_id == id)))
    })
}

impl Sidebar {
    pub(super) fn set_active_projects_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let items = self.build_cursor_items(cx);
        let selection = self
            .pending_bulk_cursor
            .clone()
            .or_else(|| self.cursor_index.and_then(|i| items.get(i).cloned()));
        let (targets, parents, folders) = {
            let ws = self.workspace.read(cx);
            let filter = ws.active_folder_filter(self.window_id).map(String::as_str);
            let services = self.collect_project_services(ws, cx);
            let terminals = self.terminals.lock();
            let mut targets = HashSet::new();
            let mut parents = HashSet::new();
            let mut folders = HashSet::new();
            for project in &ws.data().projects {
                let info = SidebarProjectInfo::from_project(project, ws, self.window_id);
                let (attention, running) = terminal_activity(&info.terminal_ids, &terminals);
                let folder = folder_for(ws.data(), project);
                let has_services = services
                    .get(&project.id)
                    .is_some_and(|items| !items.is_empty());
                if !selected(project.pinned, attention, running, folder, filter)
                    || (info.terminal_ids.is_empty()
                        && info.hook_terminals.is_empty()
                        && !has_services)
                {
                    continue;
                }
                targets.insert(project.id.clone());
                if let Some(w) = &project.worktree_info {
                    parents.insert(w.parent_project_id.clone());
                }
                if !project.worktree_ids.is_empty() {
                    parents.insert(project.id.clone());
                }
                if let Some(folder) = folder {
                    folders.insert(folder.to_owned());
                }
            }
            (targets, parents, folders)
        };
        if targets.is_empty() {
            return;
        }
        self.pending_bulk_cursor = selection;
        for id in targets {
            if expanded {
                self.expanded_projects.insert(id.clone());
                self.collapsed_groups.remove(&(id, GroupKind::Terminals));
            } else {
                self.expanded_projects.remove(&id);
            }
        }
        if expanded {
            self.collapsed_worktrees.retain(|id| !parents.contains(id));
            self.workspace.update(cx, |ws, cx| {
                for folder in folders {
                    ws.set_folder_collapsed(self.window_id, &folder, false, cx);
                }
            });
        }
        if !self.is_activity_sort_mode(cx) {
            let items = self.build_cursor_items(cx);
            self.restore_bulk_cursor(&items);
        }
        cx.notify();
    }

    pub(super) fn restore_bulk_cursor(&mut self, items: &[SidebarCursorItem]) {
        if let Some(item) = self.pending_bulk_cursor.take() {
            self.cursor_index = restore_index(&item, items);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{folder_for, restore_index, selected};
    use crate::sidebar::SidebarCursorItem;
    use okena_workspace::state::WorkspaceData;

    #[gpui::test]
    fn bulk_expansion_preserves_neighbors_and_groups(cx: &mut gpui::TestAppContext) {
        use super::{GroupKind, Sidebar};
        use gpui::AppContext as _;
        use okena_workspace::{
            focus::FocusManager,
            request_broker::RequestBroker,
            state::{LayoutNode, WindowId, Workspace},
        };
        let mut data: WorkspaceData = serde_json::from_value(serde_json::json!({
            "version": 1,
            "projects": [
                {"id":"parent", "name":"parent", "path":"/parent", "worktree_ids":["child"]},
                {"id":"child", "name":"child", "path":"/child", "pinned":true, "worktree_info":{"parent_project_id":"parent"}},
                {"id":"recent", "name":"recent", "path":"/recent"}
            ],
            "project_order":["folder", "recent"],
            "folders":[{"id":"folder", "name":"folder", "project_ids":["parent"]}]
        })).unwrap();
        for project in &mut data.projects {
            let mut layout = LayoutNode::new_terminal();
            if let LayoutNode::Terminal { terminal_id, .. } = &mut layout {
                *terminal_id = Some(format!("terminal-{}", project.id));
            }
            project.layout = Some(layout);
        }
        data.main_window
            .folder_collapsed
            .insert("folder".into(), true);
        let sidebar = cx.update(|cx| {
            let workspace = cx.new(|_| Workspace::new(data));
            let focus = cx.new(|_| FocusManager::new());
            let broker = cx.new(|_| RequestBroker::new());
            cx.new(|cx| {
                Sidebar::new(
                    WindowId::Main,
                    workspace,
                    focus,
                    broker,
                    Default::default(),
                    cx,
                )
            })
        });
        sidebar.update(cx, |sidebar, cx| {
            sidebar.collapsed_worktrees.insert("parent".into());
            sidebar.expanded_projects.insert("recent".into());
            for group in [GroupKind::Terminals, GroupKind::Services, GroupKind::Hooks] {
                sidebar.collapsed_groups.insert(("child".into(), group));
            }
            sidebar.set_active_projects_expanded(true, cx);
            sidebar.set_active_projects_expanded(true, cx);
            assert!(sidebar.expanded_projects.contains("child"));
            assert!(sidebar.expanded_projects.contains("recent"));
            assert!(!sidebar.expanded_projects.contains("parent"));
            assert!(!sidebar.collapsed_worktrees.contains("parent"));
            assert!(
                !sidebar
                    .workspace
                    .read(cx)
                    .is_folder_collapsed(WindowId::Main, "folder")
            );
            assert!(!sidebar.is_group_collapsed("child", &GroupKind::Terminals));
            assert!(sidebar.is_group_collapsed("child", &GroupKind::Services));
            assert!(sidebar.is_group_collapsed("child", &GroupKind::Hooks));
            let items = sidebar.build_cursor_items(cx);
            sidebar.cursor_index = items.iter().position(|item| matches!(item, SidebarCursorItem::Terminal { project_id, .. } if project_id == "child"));
            assert!(sidebar.cursor_index.is_some());
            sidebar.set_active_projects_expanded(false, cx);
            sidebar.set_active_projects_expanded(false, cx);
            assert!(!sidebar.expanded_projects.contains("child"));
            let items = sidebar.build_cursor_items(cx);
            assert!(matches!(&items[sidebar.cursor_index.unwrap()], SidebarCursorItem::WorktreeProject { project_id } if project_id == "child"));
            assert!(sidebar.expanded_projects.contains("recent"));
            assert!(!sidebar.collapsed_worktrees.contains("parent"));
            assert!(sidebar.is_group_collapsed("child", &GroupKind::Services));
        });
    }

    #[test]
    fn active_tiers_respect_filter() {
        for flags in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
            (true, true, true),
        ] {
            assert!(selected(flags.0, flags.1, flags.2, Some("f"), None));
            assert!(selected(flags.0, flags.1, flags.2, Some("f"), Some("f")));
            assert!(!selected(
                flags.0,
                flags.1,
                flags.2,
                Some("f"),
                Some("other")
            ));
            assert!(!selected(flags.0, flags.1, flags.2, None, Some("f")));
        }
        assert!(!selected(false, false, false, Some("f"), None));
    }

    #[test]
    fn worktree_inherits_folder_without_stripping_remote_id() {
        let data: WorkspaceData = serde_json::from_value(serde_json::json!({
            "version": 1,
            "project_order": [],
            "projects": [{"id":"remote:child", "name":"child", "path":"/child", "worktree_info":{"parent_project_id":"remote:parent"}}],
            "folders":[{"id":"folder", "name":"folder", "project_ids":["remote:parent"]}]
        })).unwrap();
        assert_eq!(folder_for(&data, &data.projects[0]), Some("folder"));
    }

    #[test]
    fn cursor_preserves_identity_then_falls_back_to_checkout() {
        let terminal = SidebarCursorItem::Terminal {
            project_id: "p".into(),
            terminal_id: "t".into(),
        };
        let parent = SidebarCursorItem::Project {
            project_id: "p".into(),
        };
        let checkout = SidebarCursorItem::WorktreeProject {
            project_id: "p".into(),
        };
        assert_eq!(
            restore_index(
                &terminal,
                &[parent.clone(), checkout.clone(), terminal.clone()]
            ),
            Some(2)
        );
        assert_eq!(
            restore_index(&terminal, &[parent.clone(), checkout]),
            Some(1)
        );
        assert_eq!(restore_index(&terminal, &[parent]), Some(0));
        assert_eq!(restore_index(&terminal, &[]), None);
    }
}
