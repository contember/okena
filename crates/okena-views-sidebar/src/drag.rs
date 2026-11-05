use gpui::*;
use okena_ui::tokens::ui_text_md;

/// Drag payload for project reordering
#[derive(Clone)]
pub struct ProjectDrag {
    pub project_id: String,
    pub project_name: String,
}

/// Drag preview view
pub struct ProjectDragView {
    pub name: String,
}

impl Render for ProjectDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .child(self.name.clone())
    }
}

/// Drag payload for a family member: reorders worktrees within a parent
/// and, with the status board on, moves a member from status to status.
#[derive(Clone)]
pub struct WorktreeDrag {
    /// The dragged project: a worktree, or the parent itself when
    /// `main_checkout`.
    pub worktree_id: String,
    pub parent_id: String,
    pub worktree_name: String,
    /// The parent's own checkout: it only changes status, it has no place
    /// in the worktree order.
    pub main_checkout: bool,
}

/// Drag preview view for worktree items
pub struct WorktreeDragView {
    pub name: String,
}

impl Render for WorktreeDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                svg()
                    .path("icons/git-branch.svg")
                    .size(px(12.0))
                    .text_color(rgb(0xcccccc)),
            )
            .child(self.name.clone())
    }
}

/// Drag payload for folder reordering
#[derive(Clone)]
pub struct FolderDrag {
    pub folder_id: String,
    pub folder_name: String,
}

/// Drag preview view for folders
pub struct FolderDragView {
    pub name: String,
}

impl Render for FolderDragView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(ui_text_md(cx))
            .text_color(rgb(0xffffff))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(12.0))
                    .text_color(rgb(0xcccccc)),
            )
            .child(self.name.clone())
    }
}
