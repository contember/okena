use gpui::*;
use okena_extensions::{ExtensionSettingsStore, ThemeColors};
use okena_ui::settings::{
    input_box, section_container, section_header, settings_input_row, settings_row_with_desc,
};
use okena_ui::simple_input::{InputChangedEvent, SimpleInput, SimpleInputState};
use okena_ui::toggle::toggle_switch;
use okena_ui::tokens::ui_text_md;
use okena_usage::WorkingDaysSetting;

const SETTINGS_KEY: &str = "claude-code";

fn theme(cx: &App) -> ThemeColors {
    okena_extensions::theme(cx)
}

/// Change one field of the extension's settings blob, keeping the others.
fn update_settings(
    cx: &mut App,
    change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) {
    let mut settings = match cx.global::<ExtensionSettingsStore>().get(SETTINGS_KEY, cx) {
        Some(serde_json::Value::Object(settings)) => settings,
        _ => serde_json::Map::new(),
    };
    change(&mut settings);
    ExtensionSettingsStore::update(SETTINGS_KEY, serde_json::Value::Object(settings), cx);
}

/// What the status bar shows for an AI proxy account pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PoolDisplay {
    pub accounts: bool,
    pub runway: bool,
    pub today: bool,
}

impl PoolDisplay {
    const KEYS: [&str; 3] = ["pool_accounts", "pool_runway", "pool_today"];

    pub fn read(cx: &App) -> Self {
        Self::from_settings(
            cx.try_global::<ExtensionSettingsStore>()
                .and_then(|store| store.get(SETTINGS_KEY, cx))
                .as_ref(),
        )
    }

    fn from_settings(settings: Option<&serde_json::Value>) -> Self {
        let flag = |key: &str, default: bool| {
            settings
                .and_then(|settings| settings[key].as_bool())
                .unwrap_or(default)
        };
        let display = Self {
            accounts: flag(Self::KEYS[0], true),
            runway: flag(Self::KEYS[1], false),
            today: flag(Self::KEYS[2], false),
        };
        if display.flags().contains(&true) {
            display
        } else {
            Self {
                accounts: true,
                ..display
            }
        }
    }

    pub fn summary(&self) -> bool {
        self.runway || self.today
    }

    fn flags(&self) -> [bool; 3] {
        [self.accounts, self.runway, self.today]
    }

    /// Flip one item; the last one shown stays on.
    fn toggle(index: usize, cx: &mut App) {
        let mut flags = Self::read(cx).flags();
        if flags[index] && flags.iter().filter(|shown| **shown).count() == 1 {
            return;
        }
        flags[index] = !flags[index];
        update_settings(cx, |settings| {
            settings.insert(Self::KEYS[index].into(), flags[index].into());
        });
    }
}

pub struct ClaudeSettingsView {
    config_dir_input: Entity<SimpleInputState>,
    working_days: Entity<WorkingDaysSetting>,
}

impl ClaudeSettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let current_value = cx
            .global::<ExtensionSettingsStore>()
            .get(SETTINGS_KEY, cx)
            .and_then(|settings| settings["config_dir"].as_str().map(ToOwned::to_owned))
            .unwrap_or_default();

        let config_dir_input = cx.new(|cx| {
            SimpleInputState::new(cx)
                .placeholder("e.g. ~/.claude-work")
                .default_value(current_value)
        });

        let working_days = cx.new(WorkingDaysSetting::new);

        cx.observe_global::<ExtensionSettingsStore>(move |this, cx| {
            let next_value = cx
                .global::<ExtensionSettingsStore>()
                .get(SETTINGS_KEY, cx)
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
                update_settings(cx, |settings| {
                    if value.is_empty() {
                        settings.remove("config_dir");
                    } else {
                        settings.insert("config_dir".into(), value.into());
                    }
                });
            },
        )
        .detach();

        Self {
            config_dir_input,
            working_days,
        }
    }
}

impl Render for ClaudeSettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        div()
            .flex()
            .flex_col()
            .child(section_header("Claude Code", &t, cx))
            .child(
                section_container(&t).child(
                    settings_input_row(
                        "claude-config-dir",
                        "Config directory",
                        "Optional override for Claude credentials/config. Supports ~/. Falls back to CLAUDE_CONFIG_DIR, then ~/.claude.",
                        &t,
                        cx,
                        false,
                    )
                    .child(
                        input_box(&t)
                            .child(SimpleInput::new(&self.config_dir_input).text_size(ui_text_md(cx))),
                    ),
                ),
            )
            .child(self.render_pool_display(&t, cx))
            .child(self.working_days.clone())
    }
}

impl ClaudeSettingsView {
    fn render_pool_display(&self, t: &ThemeColors, cx: &mut Context<Self>) -> impl IntoElement {
        let display = PoolDisplay::read(cx);
        // Rows in display order; the index names the flag in `PoolDisplay::KEYS`.
        let rows = [
            (
                0,
                "Per-account usage",
                "The 5h and 7d usage of every account.",
                display.accounts,
            ),
            (
                2,
                "Today",
                "Soft limit: how much more work today leaves enough weekly quota for every working day until the next resets.",
                display.today,
            ),
            (
                1,
                "Nonstop",
                "Hard limit: how long nonstop work at your usual pace lasts before every account is blocked.",
                display.runway,
            ),
        ];
        let last = rows.len() - 1;
        let mut container = section_container(t);
        for (position, (index, label, description, shown)) in rows.into_iter().enumerate() {
            container = container.child(
                settings_row_with_desc(
                    format!("claude-pool-display-{index}"),
                    label,
                    description,
                    t,
                    cx,
                    position < last,
                )
                .child(
                    toggle_switch(format!("claude-pool-display-toggle-{index}"), shown, t)
                        .on_click(cx.listener(move |_, _, _, cx| {
                            PoolDisplay::toggle(index, cx);
                            cx.notify();
                        })),
                ),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(section_header("Account pool in the status bar", t, cx))
            .child(container)
    }
}

#[cfg(test)]
mod tests {
    use super::PoolDisplay;

    #[test]
    fn pool_display_defaults_to_accounts_and_never_shows_nothing() {
        let accounts_only = PoolDisplay {
            accounts: true,
            runway: false,
            today: false,
        };
        assert_eq!(PoolDisplay::from_settings(None), accounts_only);
        let summary =
            serde_json::json!({"pool_accounts": false, "pool_runway": true, "pool_today": true});
        assert_eq!(
            PoolDisplay::from_settings(Some(&summary)),
            PoolDisplay {
                accounts: false,
                runway: true,
                today: true,
            }
        );
        let nothing = serde_json::json!({"pool_accounts": false});
        assert_eq!(PoolDisplay::from_settings(Some(&nothing)), accounts_only);
    }
}
