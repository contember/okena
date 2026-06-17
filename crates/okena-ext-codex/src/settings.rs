use gpui::*;
use okena_extensions::ThemeColors;
use okena_ui::settings::section_header;
use okena_usage::WorkingDaysSetting;

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

pub struct CodexSettingsView {
    working_days: Entity<WorkingDaysSetting>,
}

impl CodexSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            working_days: cx.new(WorkingDaysSetting::new),
        }
    }
}

impl Render for CodexSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section_header("Codex", &t, cx))
            .child(self.working_days.clone())
    }
}
