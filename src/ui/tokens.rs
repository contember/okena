//! Design tokens — re-exported from okena-ui, with scaled text size helpers.

pub use okena_ui::tokens::*;

use okena_ui::tokens as base;

pub fn ui_text_xs(cx: &gpui::App) -> gpui::Pixels {
    base::ui_text_xs(crate::settings::settings(cx).ui_font_size)
}

pub fn ui_text_sm(cx: &gpui::App) -> gpui::Pixels {
    base::ui_text_sm(crate::settings::settings(cx).ui_font_size)
}

pub fn ui_text_ms(cx: &gpui::App) -> gpui::Pixels {
    base::ui_text_ms(crate::settings::settings(cx).ui_font_size)
}

pub fn ui_text_md(cx: &gpui::App) -> gpui::Pixels {
    base::ui_text_md(crate::settings::settings(cx).ui_font_size)
}

pub fn ui_text_xl(cx: &gpui::App) -> gpui::Pixels {
    base::ui_text_xl(crate::settings::settings(cx).ui_font_size)
}

pub fn ui_text(default_px: f32, cx: &gpui::App) -> gpui::Pixels {
    base::ui_text(default_px, crate::settings::settings(cx).ui_font_size)
}
