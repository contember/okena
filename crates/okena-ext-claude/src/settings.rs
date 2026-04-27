use gpui::*;
use okena_extensions::{ExtensionSettingsStore, ThemeColors};
use okena_ui::settings::{section_container, section_header, settings_row_with_desc};
use okena_ui::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use okena_ui::tokens::ui_text_md;

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

pub struct ClaudeSettingsView {
    config_dir_input: Entity<SimpleInputState>,
}

impl ClaudeSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let current_value = cx
            .global::<ExtensionSettingsStore>()
            .get("claude-code", cx)
            .and_then(|settings| settings["config_dir"].as_str().map(ToOwned::to_owned))
            .unwrap_or_default();

        let config_dir_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("e.g. ~/.claude-work")
                .default_value(current_value)
        });

        cx.observe_global::<ExtensionSettingsStore>(move |this, cx| {
            let next_value = cx
                .global::<ExtensionSettingsStore>()
                .get("claude-code", cx)
                .and_then(|settings| settings["config_dir"].as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            this.config_dir_input.update(cx, |input, cx| {
                if input.value() != next_value {
                    input.set_value(next_value, cx);
                }
            });
            cx.notify();
        })
        .detach();

        cx.subscribe(
            &config_dir_input,
            |_this, entity, _: &InputChangedEvent, cx| {
                let value = entity.read(cx).value().trim().to_string();
                let settings = if value.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::json!({ "config_dir": value })
                };
                okena_extensions::ExtensionSettingsStore::update("claude-code", settings, cx);
            },
        )
        .detach();

        Self { config_dir_input }
    }
}

impl Render for ClaudeSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(section_header("Claude Code", &t, cx))
            .child(
                section_container(&t).child(
                    settings_row_with_desc(
                        "claude-config-dir",
                        "Config directory",
                        "Optional override for Claude credentials/config. Supports ~/. Falls back to CLAUDE_CONFIG_DIR, then ~/.claude.",
                        &t,
                        cx,
                        false,
                    )
                    .child(
                        div()
                            .w(px(260.0))
                            .bg(rgb(t.bg_secondary))
                            .border_1()
                            .border_color(rgb(t.border))
                            .rounded(px(4.0))
                            .child(SimpleInput::new(&self.config_dir_input).text_size(ui_text_md(cx))),
                    ),
                ),
            )
    }
}
