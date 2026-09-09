//! Pane and tab moves (same-project and cross-project).

// Move ops thread the workspace, focus manager, layout path, drop zone and cx
// as distinct positional inputs; a context struct would obscure more than it
// clarifies here.
#![allow(clippy::too_many_arguments)]

use crate::context::WorkspaceCx;
use crate::focus::FocusManager;
use crate::state::{DropZone, LayoutNode, SplitDirection, Workspace};

impl Workspace {
    /// Move a terminal pane to a new position relative to a target terminal.
    ///
    /// Extracts the source terminal from its current position and inserts it
    /// next to the target based on the drop zone (Top/Bottom/Left/Right/Center).
    /// Supports both same-project and cross-project moves.
    pub fn move_pane(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        source_terminal_id: &str,
        target_project_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut impl WorkspaceCx,
    ) {
        // Self-drop check
        if source_terminal_id == target_terminal_id {
            return;
        }

        if source_project_id == target_project_id {
            self.move_pane_same_project(
                focus_manager,
                source_project_id,
                source_terminal_id,
                target_terminal_id,
                zone,
                cx,
            );
        } else {
            self.move_pane_cross_project(
                focus_manager,
                source_project_id,
                source_terminal_id,
                target_project_id,
                target_terminal_id,
                zone,
                cx,
            );
        }
    }

    /// Same-project pane move (original logic).
    fn move_pane_same_project(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        source_terminal_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut impl WorkspaceCx,
    ) {
        let project = match self.project(project_id) {
            Some(p) => p,
            None => return,
        };
        let layout = match project.layout.as_ref() {
            Some(l) => l,
            None => return,
        };

        // Only-terminal check: don't move if it's the only terminal
        if layout.collect_terminal_ids().len() <= 1 {
            return;
        }

        // Find source path
        let source_path = match layout.find_terminal_path(source_terminal_id) {
            Some(p) => p,
            None => return,
        };

        // Clone source node before removal
        let source_node = match layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        if source_path.is_empty() {
            // Source is root — can't remove root
            return;
        }

        // Every step below runs on a working copy: each one can fail after the
        // source pane has left the tree, and a partial move would lose it.
        let new_focus_path = {
            let project = match self.project_mut(project_id) {
                Some(p) => p,
                None => return,
            };
            let layout = match project.layout.as_mut() {
                Some(l) => l,
                None => return,
            };

            let mut moved = layout.clone();
            if moved.remove_at_path(&source_path).is_none() {
                return;
            }

            // Re-find target path after removal (indices may have shifted)
            let target_path = match moved.find_terminal_path(target_terminal_id) {
                Some(p) => p,
                None => return,
            };

            // Get target node and replace it with wrapper
            let target_node = match moved.get_at_path(&target_path) {
                Some(node) => node.clone(),
                None => return,
            };

            let wrapper = Self::build_drop_zone_wrapper(source_node, target_node, zone);

            match moved.get_at_path_mut(&target_path) {
                Some(node) => *node = wrapper,
                None => return,
            }

            // Normalize to flatten nested same-direction splits
            moved.normalize();

            let focus = moved.find_terminal_path(source_terminal_id);
            *layout = moved;
            focus
        };

        self.notify_data(cx);

        // Update focus to moved terminal's new path
        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
        }
    }

    /// Cross-project pane move: extract terminal from source project, insert into target project.
    fn move_pane_cross_project(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        source_terminal_id: &str,
        target_project_id: &str,
        target_terminal_id: &str,
        zone: DropZone,
        cx: &mut impl WorkspaceCx,
    ) {
        // Find project indices (needed for split borrows)
        let src_idx = match self
            .data
            .projects
            .iter()
            .position(|p| p.id == source_project_id)
        {
            Some(i) => i,
            None => return,
        };
        let tgt_idx = match self
            .data
            .projects
            .iter()
            .position(|p| p.id == target_project_id)
        {
            Some(i) => i,
            None => return,
        };

        // Validate source has layout and terminal exists
        let src_layout = match self.data.projects[src_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        let source_path = match src_layout.find_terminal_path(source_terminal_id) {
            Some(p) => p,
            None => return,
        };
        let source_node = match src_layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        // Block if terminal is a service terminal
        if self.data.projects[src_idx]
            .service_terminals
            .values()
            .any(|id| id == source_terminal_id)
        {
            return;
        }

        // Build the destination before the source is touched: an insertion that
        // fails once the pane is already extracted would lose it.
        let tgt_layout = match self.data.projects[tgt_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        let (inserted_tgt_layout, new_focus_path) = {
            let mut inserted = tgt_layout.clone();
            let target_path = match inserted.find_terminal_path(target_terminal_id) {
                Some(p) => p,
                None => return,
            };
            let target_node = match inserted.get_at_path(&target_path) {
                Some(node) => node.clone(),
                None => return,
            };
            let wrapper = Self::build_drop_zone_wrapper(source_node, target_node, zone);
            match inserted.get_at_path_mut(&target_path) {
                Some(node) => *node = wrapper,
                None => return,
            }
            inserted.normalize();
            let focus = inserted.find_terminal_path(source_terminal_id);
            (inserted, focus)
        };

        // --- Extract from source ---
        let src_project = &mut self.data.projects[src_idx];
        if source_path.is_empty() {
            // Source is root — remove entire layout
            src_project.layout = None;
        } else if let Some(src_layout) = src_project.layout.as_mut() {
            if src_layout.remove_at_path(&source_path).is_none() {
                return;
            }
            src_layout.normalize();
        } else {
            // Layout was validated Some above; disappearing is a bug, not a crash.
            return;
        }

        // Migrate metadata from source to target
        let terminal_name = src_project.terminal_names.remove(source_terminal_id);
        let hidden_state = src_project.hidden_terminals.remove(source_terminal_id);

        // Cleanup orphaned source metadata
        let src_layout_ids: std::collections::HashSet<String> = src_project
            .layout
            .as_ref()
            .map(|l| l.collect_terminal_ids().into_iter().collect())
            .unwrap_or_default();
        src_project
            .terminal_names
            .retain(|id, _| src_layout_ids.contains(id));
        src_project
            .hidden_terminals
            .retain(|id, _| src_layout_ids.contains(id));

        // --- Insert into target ---
        let tgt_project = &mut self.data.projects[tgt_idx];

        if let Some(name) = terminal_name {
            tgt_project
                .terminal_names
                .insert(source_terminal_id.to_string(), name);
        }
        if let Some(hidden) = hidden_state {
            tgt_project
                .hidden_terminals
                .insert(source_terminal_id.to_string(), hidden);
        }

        tgt_project.layout = Some(inserted_tgt_layout);

        self.notify_data(cx);

        // Focus the moved terminal in the target project
        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, target_project_id.to_string(), new_path, cx);
        }
    }

    /// Build wrapper node for drop zone placement.
    fn build_drop_zone_wrapper(
        source_node: LayoutNode,
        target_node: LayoutNode,
        zone: DropZone,
    ) -> LayoutNode {
        match zone {
            DropZone::Top => LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0, 50.0],
                children: vec![source_node, target_node],
            },
            DropZone::Bottom => LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                sizes: vec![50.0, 50.0],
                children: vec![target_node, source_node],
            },
            DropZone::Left => LayoutNode::Split {
                direction: SplitDirection::Vertical,
                sizes: vec![50.0, 50.0],
                children: vec![source_node, target_node],
            },
            DropZone::Right => LayoutNode::Split {
                direction: SplitDirection::Vertical,
                sizes: vec![50.0, 50.0],
                children: vec![target_node, source_node],
            },
            DropZone::Center => LayoutNode::Tabs {
                children: vec![target_node, source_node],
                active_tab: 1,
            },
        }
    }

    /// A terminal already in `tabs`, used to find that group again once removal
    /// has shifted paths. `None` unless `tabs` really is a tab group.
    ///
    /// A direct child is preferred: a terminal nested in a child container would
    /// re-resolve to that container instead of the group the drop targeted.
    fn tab_group_reference_terminal(tabs: &LayoutNode, exclude: &str) -> Option<String> {
        let LayoutNode::Tabs { children, .. } = tabs else {
            return None;
        };
        children
            .iter()
            .find_map(|child| match child {
                LayoutNode::Terminal {
                    terminal_id: Some(id),
                    ..
                } if id != exclude => Some(id.clone()),
                _ => None,
            })
            .or_else(|| {
                tabs.collect_terminal_ids()
                    .into_iter()
                    .find(|id| id != exclude)
            })
    }

    /// Path of the innermost `Tabs` node containing `path` — the tab group the
    /// node at `path` belongs to.
    fn enclosing_tabs_path(layout: &LayoutNode, path: &[usize]) -> Option<Vec<usize>> {
        (0..path.len())
            .rev()
            .map(|len| &path[..len])
            .find(|prefix| matches!(layout.get_at_path(prefix), Some(LayoutNode::Tabs { .. })))
            .map(<[usize]>::to_vec)
    }

    /// Move a terminal into an existing tab group.
    ///
    /// Extracts the source terminal from its current position and inserts it
    /// into the Tabs container at `tabs_path` at the given `insert_index`
    /// (or appends if `None`). This avoids the nested-Tabs problem that
    /// `move_pane(Center)` would create when the target is already inside
    /// a tab group.
    ///
    /// A same-project removal may collapse the tree (a 2-child split dissolves),
    /// so that path re-locates the group through a reference terminal instead of
    /// trusting `tabs_path`; a cross-project removal cannot shift it.
    ///
    /// Supports cross-project moves when `target_project_id` differs from
    /// `source_project_id`.
    pub fn move_terminal_to_tab_group(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        terminal_id: &str,
        target_project_id: &str,
        tabs_path: &[usize],
        insert_index: Option<usize>,
        cx: &mut impl WorkspaceCx,
    ) {
        if source_project_id == target_project_id {
            self.move_terminal_to_tab_group_same_project(
                focus_manager,
                source_project_id,
                terminal_id,
                tabs_path,
                insert_index,
                cx,
            );
        } else {
            self.move_terminal_to_tab_group_cross_project(
                focus_manager,
                source_project_id,
                terminal_id,
                target_project_id,
                tabs_path,
                insert_index,
                cx,
            );
        }
    }

    /// Same-project tab group move (original logic).
    fn move_terminal_to_tab_group_same_project(
        &mut self,
        focus_manager: &mut FocusManager,
        project_id: &str,
        terminal_id: &str,
        tabs_path: &[usize],
        insert_index: Option<usize>,
        cx: &mut impl WorkspaceCx,
    ) {
        let project = match self.project(project_id) {
            Some(p) => p,
            None => return,
        };
        let layout = match project.layout.as_ref() {
            Some(l) => l,
            None => return,
        };

        // Find source path
        let source_path = match layout.find_terminal_path(terminal_id) {
            Some(p) => p,
            None => return,
        };

        // Don't move if source is already in the target tab group
        if !source_path.is_empty() {
            let source_parent = &source_path[..source_path.len() - 1];
            if source_parent == tabs_path {
                // Already in this tab group — treat as reorder or noop
                if let Some(idx) = insert_index {
                    let from = source_path[source_path.len() - 1];
                    if from != idx {
                        self.move_tab(project_id, tabs_path, from, idx, cx);
                    }
                }
                return;
            }
        }

        // Clone source node
        let source_node = match layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        if source_path.is_empty() {
            return; // Can't remove root
        }

        // Reject a destination that is not a tab group, and take a reference
        // terminal from it so the group can be found again after removal.
        let reference_tid = match layout
            .get_at_path(tabs_path)
            .and_then(|node| Self::tab_group_reference_terminal(node, terminal_id))
        {
            Some(id) => id,
            None => return,
        };

        // Mutate a working copy: the insertion below can still fail, and by then
        // the source pane has left the tree.
        let new_focus_path = {
            let project = match self.project_mut(project_id) {
                Some(p) => p,
                None => return,
            };
            let layout = match project.layout.as_mut() {
                Some(l) => l,
                None => return,
            };

            let mut moved = layout.clone();
            if moved.remove_at_path(&source_path).is_none() {
                return;
            }

            let ref_path = match moved.find_terminal_path(&reference_tid) {
                Some(p) => p,
                None => return,
            };
            let new_tabs_path = match Self::enclosing_tabs_path(&moved, &ref_path) {
                Some(p) => p,
                None => return,
            };

            let Some(LayoutNode::Tabs {
                children,
                active_tab,
            }) = moved.get_at_path_mut(&new_tabs_path)
            else {
                return;
            };
            let idx = insert_index.unwrap_or(children.len());
            let clamped = idx.min(children.len());
            children.insert(clamped, source_node);
            *active_tab = clamped;

            moved.normalize();
            let focus = moved.find_terminal_path(terminal_id);
            *layout = moved;
            focus
        };

        self.notify_data(cx);

        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, project_id.to_string(), new_path, cx);
        }
    }

    /// Cross-project tab group move: extract terminal from source project, insert into target tab group.
    fn move_terminal_to_tab_group_cross_project(
        &mut self,
        focus_manager: &mut FocusManager,
        source_project_id: &str,
        terminal_id: &str,
        target_project_id: &str,
        tabs_path: &[usize],
        insert_index: Option<usize>,
        cx: &mut impl WorkspaceCx,
    ) {
        let src_idx = match self
            .data
            .projects
            .iter()
            .position(|p| p.id == source_project_id)
        {
            Some(i) => i,
            None => return,
        };
        let tgt_idx = match self
            .data
            .projects
            .iter()
            .position(|p| p.id == target_project_id)
        {
            Some(i) => i,
            None => return,
        };

        // Validate source
        let src_layout = match self.data.projects[src_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        let source_path = match src_layout.find_terminal_path(terminal_id) {
            Some(p) => p,
            None => return,
        };
        let source_node = match src_layout.get_at_path(&source_path) {
            Some(node) => node.clone(),
            None => return,
        };

        // Block service terminals
        if self.data.projects[src_idx]
            .service_terminals
            .values()
            .any(|id| id == terminal_id)
        {
            return;
        }

        // Build the destination before the source is touched. Removing from
        // another project cannot shift these paths, so `tabs_path` still names
        // the tab group the drop targeted.
        let tgt_layout = match self.data.projects[tgt_idx].layout.as_ref() {
            Some(l) => l,
            None => return,
        };
        let (inserted_tgt_layout, new_focus_path) = {
            let mut inserted = tgt_layout.clone();
            let Some(LayoutNode::Tabs {
                children,
                active_tab,
            }) = inserted.get_at_path_mut(tabs_path)
            else {
                return;
            };
            let idx = insert_index.unwrap_or(children.len());
            let clamped = idx.min(children.len());
            children.insert(clamped, source_node);
            *active_tab = clamped;
            inserted.normalize();
            let focus = inserted.find_terminal_path(terminal_id);
            (inserted, focus)
        };

        // --- Extract from source ---
        let src_project = &mut self.data.projects[src_idx];
        if source_path.is_empty() {
            src_project.layout = None;
        } else if let Some(src_layout) = src_project.layout.as_mut() {
            if src_layout.remove_at_path(&source_path).is_none() {
                return;
            }
            src_layout.normalize();
        } else {
            // Layout was validated Some above; disappearing is a bug, not a crash.
            return;
        }

        // Migrate metadata
        let terminal_name = src_project.terminal_names.remove(terminal_id);
        let hidden_state = src_project.hidden_terminals.remove(terminal_id);

        // Cleanup orphaned source metadata
        let src_layout_ids: std::collections::HashSet<String> = src_project
            .layout
            .as_ref()
            .map(|l| l.collect_terminal_ids().into_iter().collect())
            .unwrap_or_default();
        src_project
            .terminal_names
            .retain(|id, _| src_layout_ids.contains(id));
        src_project
            .hidden_terminals
            .retain(|id, _| src_layout_ids.contains(id));

        // --- Insert into target ---
        let tgt_project = &mut self.data.projects[tgt_idx];

        if let Some(name) = terminal_name {
            tgt_project
                .terminal_names
                .insert(terminal_id.to_string(), name);
        }
        if let Some(hidden) = hidden_state {
            tgt_project
                .hidden_terminals
                .insert(terminal_id.to_string(), hidden);
        }

        tgt_project.layout = Some(inserted_tgt_layout);

        self.notify_data(cx);

        if let Some(new_path) = new_focus_path {
            self.set_focused_terminal(focus_manager, target_project_id.to_string(), new_path, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::context::WorkspaceCx;
    use crate::focus::FocusManager;
    use crate::settings::HooksConfig;
    use crate::state::{
        DropZone, LayoutNode, ProjectData, SplitDirection, Workspace, WorkspaceData,
    };
    use okena_core::theme::FolderColor;
    use okena_terminal::shell_config::ShellType;
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestCx;

    impl WorkspaceCx for TestCx {
        fn notify(&mut self) {}
        fn refresh_views(&mut self) {}
        fn hook_runner(&self) -> Option<okena_hooks::HookRunner> {
            None
        }
        fn hook_monitor(&self) -> Option<okena_hooks::HookMonitor> {
            None
        }
    }

    fn terminal(id: &str) -> LayoutNode {
        LayoutNode::Terminal {
            terminal_id: Some(id.to_string()),
            minimized: false,
            detached: false,
            shell_type: ShellType::Default,
            zoom_level: 1.0,
        }
    }

    fn split(children: Vec<LayoutNode>) -> LayoutNode {
        let sizes = vec![100.0 / children.len() as f32; children.len()];
        LayoutNode::Split {
            direction: SplitDirection::Vertical,
            sizes,
            children,
        }
    }

    fn tabs(children: Vec<LayoutNode>) -> LayoutNode {
        LayoutNode::Tabs {
            children,
            active_tab: 0,
        }
    }

    fn project(id: &str, layout: LayoutNode) -> ProjectData {
        ProjectData {
            id: id.to_string(),
            name: id.to_string(),
            path: "/tmp/test".to_string(),
            layout: Some(layout),
            terminal_names: HashMap::new(),
            hidden_terminals: HashMap::new(),
            worktree_info: None,
            worktree_ids: Vec::new(),
            folder_color: FolderColor::default(),
            hooks: HooksConfig::default(),
            connection_id: None,
            service_terminals: HashMap::new(),
            default_shell: None,
            hook_terminals: HashMap::new(),
            pinned: false,
            last_activity_at: None,
            is_creating: false,
            is_closing: false,
            creating_progress: None,
        }
    }

    fn workspace(projects: Vec<ProjectData>) -> Workspace {
        let mut data = WorkspaceData::empty();
        data.project_order = projects.iter().map(|p| p.id.clone()).collect();
        data.projects = projects;
        Workspace::new(data)
    }

    fn terminal_ids(ws: &Workspace, project_id: &str) -> Vec<String> {
        ws.project(project_id)
            .and_then(|p| p.layout.as_ref())
            .map(LayoutNode::collect_terminal_ids)
            .unwrap_or_default()
    }

    #[test]
    fn pane_move_to_a_stale_target_keeps_the_source_pane() {
        let mut ws = workspace(vec![project(
            "p1",
            split(vec![terminal("a"), terminal("b")]),
        )]);

        ws.move_pane(
            &mut FocusManager::new(),
            "p1",
            "a",
            "p1",
            "gone",
            DropZone::Right,
            &mut TestCx,
        );

        assert_eq!(terminal_ids(&ws, "p1"), vec!["a", "b"]);
    }

    #[test]
    fn tab_group_move_to_a_split_destination_keeps_the_source_pane() {
        let mut ws = workspace(vec![project(
            "p1",
            split(vec![
                split(vec![terminal("x"), terminal("y")]),
                terminal("z"),
            ]),
        )]);

        ws.move_terminal_to_tab_group(
            &mut FocusManager::new(),
            "p1",
            "z",
            "p1",
            &[0],
            None,
            &mut TestCx,
        );

        assert_eq!(terminal_ids(&ws, "p1"), vec!["x", "y", "z"]);
    }

    #[test]
    fn tab_group_move_targets_the_requested_group_not_a_nested_split() {
        // The tab group's first terminal sits inside a child split, so the
        // parent of that terminal is the split — not the group that was dropped on.
        let mut ws = workspace(vec![project(
            "p1",
            split(vec![
                tabs(vec![
                    split(vec![terminal("x"), terminal("y")]),
                    terminal("w"),
                ]),
                terminal("z"),
            ]),
        )]);

        ws.move_terminal_to_tab_group(
            &mut FocusManager::new(),
            "p1",
            "z",
            "p1",
            &[0],
            None,
            &mut TestCx,
        );

        let layout = ws.project("p1").unwrap().layout.clone().unwrap();
        match layout {
            LayoutNode::Tabs { children, .. } => {
                assert_eq!(children.len(), 3);
                assert_eq!(children[2], terminal("z"));
            }
            other => panic!("expected the requested tab group to gain a tab, got {other:?}"),
        }
    }

    #[test]
    fn cross_project_tab_group_move_to_a_split_destination_keeps_the_source_pane() {
        let mut ws = workspace(vec![
            project("p1", split(vec![terminal("a"), terminal("b")])),
            project("p2", split(vec![terminal("c"), terminal("d")])),
        ]);

        ws.move_terminal_to_tab_group(
            &mut FocusManager::new(),
            "p1",
            "a",
            "p2",
            &[],
            None,
            &mut TestCx,
        );

        assert_eq!(terminal_ids(&ws, "p1"), vec!["a", "b"]);
        assert_eq!(terminal_ids(&ws, "p2"), vec!["c", "d"]);
    }

    #[test]
    fn cross_project_tab_group_move_targets_the_requested_group() {
        let mut ws = workspace(vec![
            project("p1", split(vec![terminal("a"), terminal("b")])),
            project(
                "p2",
                tabs(vec![
                    split(vec![terminal("c"), terminal("d")]),
                    terminal("e"),
                ]),
            ),
        ]);

        ws.move_terminal_to_tab_group(
            &mut FocusManager::new(),
            "p1",
            "a",
            "p2",
            &[],
            Some(0),
            &mut TestCx,
        );

        assert_eq!(terminal_ids(&ws, "p1"), vec!["b"]);
        let layout = ws.project("p2").unwrap().layout.clone().unwrap();
        match layout {
            LayoutNode::Tabs { children, .. } => {
                assert_eq!(children.len(), 3);
                assert_eq!(children[0], terminal("a"));
            }
            other => panic!("expected the requested tab group to gain a tab, got {other:?}"),
        }
    }
}
