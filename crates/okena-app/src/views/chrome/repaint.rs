//! Invalidation for window chrome rendered as `.cached()` views.
//!
//! A cached view re-renders only when it is notified itself: a notify on a
//! model entity it reads during `render` does not reach it. These observers
//! turn a change of shared appearance state into a notify on the view.

use crate::settings::settings_entity;
use crate::theme::theme_entity;
use gpui::Context;

/// Notify the view whenever the displayed theme colors change, including a
/// live preview.
pub(crate) fn repaint_on_theme_change<V: 'static>(cx: &mut Context<V>) {
    let theme = theme_entity(cx);
    let mut last = theme.read(cx).display_colors();
    cx.observe(&theme, move |_, theme, cx| {
        let next = theme.read(cx).display_colors();
        if next != last {
            last = next;
            cx.notify();
        }
    })
    .detach();
}

/// Notify the view whenever `ui_font_size`, which scales every `ui_text_*`
/// size, changes.
pub(crate) fn repaint_on_ui_font_size_change<V: 'static>(cx: &mut Context<V>) {
    let settings = settings_entity(cx);
    let mut last = settings.read(cx).settings.ui_font_size;
    cx.observe(&settings, move |_, settings, cx| {
        let next = settings.read(cx).settings.ui_font_size;
        if next != last {
            last = next;
            cx.notify();
        }
    })
    .detach();
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::settings::{GlobalSettings, SettingsState};
    use crate::theme::{AppTheme, GlobalTheme, ThemeMode, theme_entity};
    use gpui::{AppContext as _, Entity, Subscription, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;

    /// The theme and settings globals the chrome views read.
    pub(crate) fn init_appearance_globals(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let theme = cx.new(|_| AppTheme::new(ThemeMode::Dark, true));
            cx.set_global(GlobalTheme(theme));
            let settings = cx.new(|_| SettingsState::new(Default::default()));
            cx.set_global(GlobalSettings(settings));
        });
    }

    /// Counts the notifies an entity receives. A `.cached()` view re-renders
    /// on exactly these (besides bounds changes and window refreshes).
    pub(crate) struct NotifyCount {
        count: Rc<Cell<usize>>,
        _subscription: Subscription,
    }

    impl NotifyCount {
        pub(crate) fn new<V: 'static>(cx: &mut TestAppContext, entity: &Entity<V>) -> Self {
            let count = Rc::new(Cell::new(0));
            let counter = count.clone();
            let subscription =
                cx.update(|cx| cx.observe(entity, move |_, _| counter.set(counter.get() + 1)));
            Self {
                count,
                _subscription: subscription,
            }
        }

        pub(crate) fn get(&self) -> usize {
            self.count.get()
        }
    }

    pub(crate) fn set_theme_mode(cx: &mut TestAppContext, mode: ThemeMode) {
        cx.update(|cx| {
            theme_entity(cx).update(cx, |theme, cx| {
                theme.set_mode(mode);
                cx.notify();
            })
        });
    }

    /// A theme notify that leaves the displayed colors as they are.
    pub(crate) fn notify_theme_unchanged(cx: &mut TestAppContext) {
        cx.update(|cx| theme_entity(cx).update(cx, |_, cx| cx.notify()));
    }

    pub(crate) fn update_settings(
        cx: &mut TestAppContext,
        update: impl FnOnce(&mut SettingsState, &mut gpui::Context<SettingsState>),
    ) {
        cx.update(|cx| crate::settings::settings_entity(cx).update(cx, update));
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{
        NotifyCount, init_appearance_globals, notify_theme_unchanged, set_theme_mode,
        update_settings,
    };
    use super::{repaint_on_theme_change, repaint_on_ui_font_size_change};
    use crate::theme::{ThemeMode, theme_entity};
    use gpui::{AppContext as _, Context, TestAppContext};

    struct Watched;

    fn watched(cx: &mut TestAppContext) -> (gpui::Entity<Watched>, NotifyCount) {
        init_appearance_globals(cx);
        let entity = cx.update(|cx| {
            cx.new(|cx: &mut Context<Watched>| {
                repaint_on_theme_change(cx);
                repaint_on_ui_font_size_change(cx);
                Watched
            })
        });
        let notifies = NotifyCount::new(cx, &entity);
        (entity, notifies)
    }

    #[gpui::test]
    fn theme_repaints_only_when_the_displayed_colors_change(cx: &mut TestAppContext) {
        let (_entity, notifies) = watched(cx);

        notify_theme_unchanged(cx);
        assert_eq!(notifies.get(), 0, "same colors, no repaint");

        set_theme_mode(cx, ThemeMode::Light);
        assert_eq!(notifies.get(), 1);

        cx.update(|cx| {
            theme_entity(cx).update(cx, |theme, cx| {
                theme.set_preview(ThemeMode::HighContrast);
                cx.notify();
            })
        });
        assert_eq!(notifies.get(), 2, "a live preview repaints");

        cx.update(|cx| {
            theme_entity(cx).update(cx, |theme, cx| {
                theme.clear_preview();
                cx.notify();
            })
        });
        assert_eq!(notifies.get(), 3, "leaving the preview repaints");
    }

    #[gpui::test]
    fn settings_repaint_only_when_the_ui_font_size_changes(cx: &mut TestAppContext) {
        let (_entity, notifies) = watched(cx);

        update_settings(cx, |s, cx| s.set_sidebar_show_agents(false, cx));
        assert_eq!(notifies.get(), 0, "an unrelated setting does not repaint");

        update_settings(cx, |s, cx| s.set_ui_font_size(16.0, cx));
        assert_eq!(notifies.get(), 1);
    }
}
