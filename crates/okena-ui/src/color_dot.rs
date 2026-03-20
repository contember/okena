//! Color dot indicator component.
//!
//! A small circle used for project/folder color indicators.

use gpui::*;

/// 8x8 color dot — solid fill or hollow (border only).
///
/// Used for project/folder color indicators in sidebars and lists.
///
/// # Example
///
/// ```rust,ignore
/// // Solid dot for regular projects
/// color_dot(0x4EC9B0, false)
///
/// // Hollow dot for worktree projects
/// color_dot(0x4EC9B0, true)
/// ```
pub fn color_dot(color: u32, hollow: bool) -> Div {
    let base = div()
        .flex_shrink_0()
        .w(px(8.0))
        .h(px(8.0))
        .rounded(px(4.0));

    if hollow {
        base.border_1().border_color(rgb(color))
    } else {
        base.bg(rgb(color))
    }
}
