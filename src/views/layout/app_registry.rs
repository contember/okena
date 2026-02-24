//! Central registry of available app types.
//!
//! Adding a new app type requires:
//! 1. Creating the app module (entity struct, Render impl)
//! 2. Adding an `AppDefinition` to `ALL_APPS`
//! 3. Adding a match arm in `create_app_pane()`
//!
//! Everything else (sidebar, tabs, actions, persistence) is data-driven.

use gpui::*;

use crate::views::layout::app_pane::AppPaneEntity;
use crate::views::layout::kruh_pane::KruhPane;
use crate::views::layout::kruh_pane::config::KruhConfig;
use crate::views::layout::layout_container::LayoutContainer;
use crate::workspace::state::Workspace;

/// Metadata for a registered app type.
pub struct AppDefinition {
    /// Unique identifier used in serialization and action dispatch (e.g. "kruh")
    pub kind: &'static str,
    /// Human-readable name shown in tabs and sidebar (e.g. "Kruh")
    pub display_name: &'static str,
    /// Icon path relative to assets (e.g. "icons/kruh.svg")
    pub icon_path: &'static str,
    /// Short description for command palette / tooltips
    pub description: &'static str,
}

/// All registered app types.
static ALL_APPS: &[AppDefinition] = &[
    AppDefinition {
        kind: "kruh",
        display_name: "Kruh",
        icon_path: "icons/kruh.svg",
        description: "Automated AI agent loop",
    },
];

/// Iterate over all registered app definitions.
pub fn all_apps() -> &'static [AppDefinition] {
    ALL_APPS
}

/// Look up an app definition by its kind string.
pub fn find_app(kind: &str) -> Option<&'static AppDefinition> {
    ALL_APPS.iter().find(|a| a.kind == kind)
}

/// Factory: create an `AppPaneEntity` for the given app kind.
///
/// This is the ONE place where kind strings are matched to concrete entity types.
pub fn create_app_pane(
    kind: &str,
    app_id: &Option<String>,
    app_config: &serde_json::Value,
    workspace: Entity<Workspace>,
    project_id: String,
    project_path: String,
    layout_path: Vec<usize>,
    window: &mut Window,
    cx: &mut Context<LayoutContainer>,
) -> Option<AppPaneEntity> {
    match kind {
        "kruh" => {
            let config: KruhConfig = app_config
                .as_object()
                .map(|_| serde_json::from_value(app_config.clone()).unwrap_or_default())
                .unwrap_or_default();
            let app_id_clone = app_id.clone();
            let pane = cx.new(|cx| {
                KruhPane::new(
                    workspace,
                    project_id,
                    project_path,
                    layout_path,
                    app_id_clone,
                    config,
                    window,
                    cx,
                )
            });
            let focus = pane.read(cx).focus_handle.clone();
            Some(AppPaneEntity::new(
                pane,
                app_id.clone(),
                "Kruh",
                "icons/kruh.svg",
                focus,
            ))
        }
        _ => {
            log::warn!("Unknown app kind: {}", kind);
            None
        }
    }
}
