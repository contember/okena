#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

// GPUI-free file operations — usable from a headless daemon.
pub mod blame;
pub mod content_search;
pub mod file_scan;
pub mod list_directory;
pub mod project_fs;

// GPUI viewer — depends on gpui / okena-ui / okena-markdown and is only built
// when the `gpui` feature is on (the default for the GUI app).
#[cfg(feature = "gpui")]
pub mod code_view;
#[cfg(feature = "gpui")]
pub mod content_search_dialog;
#[cfg(feature = "gpui")]
pub mod file_search;
#[cfg(feature = "gpui")]
pub mod file_tree;
#[cfg(feature = "gpui")]
pub mod file_viewer;
#[cfg(feature = "gpui")]
pub mod in_page_search;
#[cfg(feature = "gpui")]
pub mod list_overlay;
#[cfg(feature = "gpui")]
pub mod markdown_highlight;
#[cfg(feature = "gpui")]
pub mod selection;
#[cfg(feature = "gpui")]
pub mod syntax;
// `theme` re-exports gpui theme helpers from `okena-ui` (a gpui crate) and is
// consumed only by gpui code, so it is gated with the rest of the viewer even
// though it holds no rendering itself.
#[cfg(feature = "gpui")]
pub mod theme;
