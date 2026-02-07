use gpui::*;

/// Drag payload for project reordering
#[derive(Clone)]
pub(super) struct ProjectDrag {
    pub project_id: String,
    pub project_name: String,
}

/// Drag preview view
pub(super) struct ProjectDragView {
    pub name: String,
}

impl Render for ProjectDragView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(px(12.0))
            .text_color(rgb(0xffffff))
            .child(self.name.clone())
    }
}

/// Drag payload for folder reordering
#[derive(Clone)]
pub(super) struct FolderDrag {
    pub folder_id: String,
    pub folder_name: String,
}

/// Drag preview view for folders
pub(super) struct FolderDragView {
    pub name: String,
}

impl Render for FolderDragView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .bg(rgb(0x2d2d2d))
            .border_1()
            .border_color(rgb(0x404040))
            .rounded(px(4.0))
            .shadow_lg()
            .text_size(px(12.0))
            .text_color(rgb(0xffffff))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                svg()
                    .path("icons/folder.svg")
                    .size(px(12.0))
                    .text_color(rgb(0xcccccc))
            )
            .child(self.name.clone())
    }
}
