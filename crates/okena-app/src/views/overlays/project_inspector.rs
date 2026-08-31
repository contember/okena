use std::sync::Arc;

use gpui::*;
use okena_ui::modal::fullscreen_panel;
use okena_ui::theme::theme;

use super::diff_viewer::{CommitNavigation, DiffViewer, DiffViewerEvent};
use super::file_viewer::{
    FileTarget, FileViewer, FileViewerConfig, FileViewerEvent, FileViewerScope,
};
use crate::views::overlays::diff_viewer::provider::GitProvider;

#[derive(Clone)]
pub struct ProjectInspectorContext {
    pub project_id: String,
    pub file_scope: FileViewerScope,
    pub git_provider: Arc<dyn GitProvider>,
}

#[derive(Clone)]
enum InspectorScreen {
    File(Entity<FileViewer>),
    Diff(Entity<DiffViewer>),
}

impl InspectorScreen {
    fn view(&self) -> AnyView {
        match self {
            Self::File(viewer) => viewer.clone().into(),
            Self::Diff(viewer) => viewer.clone().into(),
        }
    }

    fn set_detached(&self, detached: bool, cx: &mut App) {
        match self {
            Self::File(viewer) => viewer.update(cx, |viewer, cx| viewer.set_detached(detached, cx)),
            Self::Diff(viewer) => viewer.update(cx, |viewer, cx| viewer.set_detached(detached, cx)),
        }
    }

    fn release_image_assets(&self, cx: &mut App) {
        if let Self::File(viewer) = self {
            viewer.update(cx, |viewer, cx| viewer.release_all_image_assets(cx));
        }
    }
}

pub struct ProjectInspector {
    focus_handle: FocusHandle,
    context: ProjectInspectorContext,
    file_config: FileViewerConfig,
    main_file_viewer: Entity<FileViewer>,
    screens: Vec<InspectorScreen>,
    is_detached: bool,
}

impl ProjectInspector {
    pub fn new(
        context: ProjectInspectorContext,
        file_config: FileViewerConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let main_file_viewer =
            cx.new(|cx| FileViewer::new_browse(context.file_scope.clone(), file_config, cx));
        let mut inspector = Self {
            focus_handle: cx.focus_handle(),
            context,
            file_config,
            main_file_viewer: main_file_viewer.clone(),
            screens: vec![InspectorScreen::File(main_file_viewer.clone())],
            is_detached: false,
        };
        inspector.subscribe_file_viewer(&main_file_viewer, cx);
        inspector
    }

    pub fn project_id(&self) -> &str {
        &self.context.project_id
    }

    pub fn current_view(&self) -> AnyView {
        self.screens
            .last()
            .map(InspectorScreen::view)
            .unwrap_or_else(|| self.main_file_viewer.clone().into())
    }

    pub fn show_browse(
        &mut self,
        context: ProjectInspectorContext,
        file_config: FileViewerConfig,
        cx: &mut Context<Self>,
    ) {
        self.update_context(context, file_config, cx);
        self.show_main_file_viewer(cx);
    }

    pub fn show_file(
        &mut self,
        context: ProjectInspectorContext,
        file_config: FileViewerConfig,
        target: FileTarget,
        cx: &mut Context<Self>,
    ) {
        self.update_context(context, file_config, cx);
        self.main_file_viewer
            .update(cx, |viewer, cx| viewer.open_target(target, cx));
        self.show_main_file_viewer(cx);
    }

    pub fn show_diff(
        &mut self,
        context: ProjectInspectorContext,
        file_config: FileViewerConfig,
        select_file: Option<String>,
        mode: Option<okena_core::types::DiffMode>,
        commit_nav: CommitNavigation,
        cx: &mut Context<Self>,
    ) {
        self.context = context;
        self.file_config = file_config;
        self.clear_screens(cx);
        let viewer = self.new_diff_viewer(select_file, mode, commit_nav, false, cx);
        self.screens.push(InspectorScreen::Diff(viewer));
        cx.emit(ProjectInspectorEvent::ScreenChanged);
        cx.notify();
    }

    pub fn set_detached(&mut self, detached: bool, cx: &mut Context<Self>) {
        if self.is_detached == detached {
            return;
        }
        self.is_detached = detached;
        self.main_file_viewer
            .update(cx, |viewer, cx| viewer.set_detached(detached, cx));
        for screen in &self.screens {
            screen.set_detached(detached, cx);
        }
        cx.notify();
    }

    pub fn release_all_image_assets(&self, cx: &mut App) {
        self.main_file_viewer
            .update(cx, |viewer, cx| viewer.release_all_image_assets(cx));
        for screen in &self.screens {
            if !matches!(screen, InspectorScreen::File(viewer) if viewer == &self.main_file_viewer)
            {
                screen.release_image_assets(cx);
            }
        }
    }

    fn update_context(
        &mut self,
        context: ProjectInspectorContext,
        file_config: FileViewerConfig,
        cx: &mut Context<Self>,
    ) {
        self.context = context;
        self.file_config = file_config;
        self.main_file_viewer.update(cx, |viewer, cx| {
            viewer.update_config(file_config.font_size, file_config.is_dark, cx);
            viewer.set_can_go_back(false, cx);
            viewer.set_detached(self.is_detached, cx);
            if viewer.is_scope(&self.context.file_scope.project_fs) {
                viewer.set_blame_visible(file_config.blame_visible, cx);
            } else {
                viewer.rebind_scope(
                    self.context.file_scope.clone(),
                    file_config.blame_visible,
                    None,
                    Default::default(),
                    cx,
                );
            }
        });
    }

    fn show_main_file_viewer(&mut self, cx: &mut Context<Self>) {
        self.clear_screens(cx);
        self.screens
            .push(InspectorScreen::File(self.main_file_viewer.clone()));
        cx.notify();
    }

    fn push_file(&mut self, target: FileTarget, cx: &mut Context<Self>) {
        let path = target.relative_path.clone();
        let viewer = cx
            .new(|cx| FileViewer::new(self.context.file_scope.clone(), self.file_config, path, cx));
        viewer.update(cx, |viewer, cx| {
            viewer.set_can_go_back(true, cx);
            viewer.set_detached(self.is_detached, cx);
            viewer.open_target(target, cx);
        });
        self.subscribe_file_viewer(&viewer, cx);
        self.screens.push(InspectorScreen::File(viewer));
        cx.emit(ProjectInspectorEvent::ScreenChanged);
        cx.notify();
    }

    fn push_diff(
        &mut self,
        select_file: Option<String>,
        mode: Option<okena_core::types::DiffMode>,
        cx: &mut Context<Self>,
    ) {
        let viewer = self.new_diff_viewer(select_file, mode, CommitNavigation::default(), true, cx);
        self.screens.push(InspectorScreen::Diff(viewer));
        cx.emit(ProjectInspectorEvent::ScreenChanged);
        cx.notify();
    }

    fn new_diff_viewer(
        &mut self,
        select_file: Option<String>,
        mode: Option<okena_core::types::DiffMode>,
        commit_nav: CommitNavigation,
        can_go_back: bool,
        cx: &mut Context<Self>,
    ) -> Entity<DiffViewer> {
        let provider = self.context.git_provider.clone();
        let viewer =
            cx.new(|cx| DiffViewer::new(provider, select_file, mode, commit_nav, can_go_back, cx));
        viewer.update(cx, |viewer, cx| viewer.set_detached(self.is_detached, cx));
        self.subscribe_diff_viewer(&viewer, cx);
        viewer
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.screens.len() > 1 {
            if let Some(screen) = self.screens.pop() {
                screen.release_image_assets(cx);
            }
            cx.emit(ProjectInspectorEvent::ScreenChanged);
            cx.notify();
        } else {
            cx.emit(ProjectInspectorEvent::Close);
        }
    }

    fn clear_screens(&mut self, cx: &mut Context<Self>) {
        for screen in self.screens.drain(..) {
            if !matches!(&screen, InspectorScreen::File(viewer) if viewer == &self.main_file_viewer)
            {
                screen.release_image_assets(cx);
            }
        }
    }

    fn subscribe_file_viewer(&mut self, viewer: &Entity<FileViewer>, cx: &mut Context<Self>) {
        cx.subscribe(viewer, |this, _, event: &FileViewerEvent, cx| match event {
            FileViewerEvent::Close => cx.emit(ProjectInspectorEvent::Close),
            FileViewerEvent::Back => this.go_back(cx),
            FileViewerEvent::Detach => cx.emit(ProjectInspectorEvent::Detach),
            FileViewerEvent::OpenCommit(hash) => this.push_diff(
                None,
                Some(okena_core::types::DiffMode::Commit(hash.clone())),
                cx,
            ),
            FileViewerEvent::OpenFileDiff {
                hash,
                relative_path,
            } => this.push_diff(
                Some(relative_path.clone()),
                Some(okena_core::types::DiffMode::Commit(hash.clone())),
                cx,
            ),
            FileViewerEvent::BlamePreferenceChanged(visible) => {
                crate::settings::settings_entity(cx).update(cx, |state, cx| {
                    state.set_blame_visible(*visible, cx);
                });
            }
            FileViewerEvent::SendToTerminal(payload) => {
                cx.emit(ProjectInspectorEvent::SendToTerminal(payload.clone()));
            }
            FileViewerEvent::OpenExternally { path, line, column } => {
                cx.emit(ProjectInspectorEvent::OpenExternally {
                    path: path.clone(),
                    line: *line,
                    column: *column,
                });
            }
        })
        .detach();
    }

    fn subscribe_diff_viewer(&mut self, viewer: &Entity<DiffViewer>, cx: &mut Context<Self>) {
        cx.subscribe(viewer, |this, _, event: &DiffViewerEvent, cx| match event {
            DiffViewerEvent::Close => cx.emit(ProjectInspectorEvent::Close),
            DiffViewerEvent::Back => this.go_back(cx),
            DiffViewerEvent::Detach => cx.emit(ProjectInspectorEvent::Detach),
            DiffViewerEvent::OpenFile(target) => this.push_file(target.clone(), cx),
            DiffViewerEvent::SendToTerminal(payload) => {
                cx.emit(ProjectInspectorEvent::SendToTerminal(payload.clone()));
            }
        })
        .detach();
    }
}

impl Render for ProjectInspector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = theme(cx);
        fullscreen_panel("project-inspector", &colors)
            .track_focus(&self.focus_handle)
            .children(
                self.screens
                    .last()
                    .map(|screen| screen.view().cached(StyleRefinement::default().size_full())),
            )
    }
}

impl Focusable for ProjectInspector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Clone, Debug)]
pub enum ProjectInspectorEvent {
    Close,
    Detach,
    ScreenChanged,
    SendToTerminal(okena_core::send_payload::SendPayload),
    OpenExternally {
        path: String,
        line: Option<usize>,
        column: Option<usize>,
    },
}

impl EventEmitter<ProjectInspectorEvent> for ProjectInspector {}

impl okena_ui::overlay::CloseEvent for ProjectInspectorEvent {
    fn is_close(&self) -> bool {
        matches!(self, Self::Close)
    }
}
