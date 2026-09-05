use crate::keybindings::{
    About as AboutAction, NewWindow as NewWindowAction, Quit as QuitAction, ShowCommandPalette,
    ShowKeybindings, ShowProfileManager, ShowSettings, ShowThemeSelector,
};
use gpui::{Action, MenuItem, SystemMenuType};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMenuAction {
    About,
    Settings,
    Profiles,
    CommandPalette,
    Theme,
    Keybindings,
    NewWindow,
    Quit,
}

impl AppMenuAction {
    pub const fn label(self) -> &'static str {
        match self {
            Self::About => "About Okena",
            Self::Settings => "Settings...",
            Self::Profiles => "Profiles...",
            Self::CommandPalette => "Command Palette",
            Self::Theme => "Select Theme",
            Self::Keybindings => "Keyboard Shortcuts",
            Self::NewWindow => "New Window",
            Self::Quit => "Quit Okena",
        }
    }

    pub const fn action_name(self) -> &'static str {
        match self {
            Self::About => "About",
            Self::Settings => "ShowSettings",
            Self::Profiles => "ShowProfileManager",
            Self::CommandPalette => "ShowCommandPalette",
            Self::Theme => "ShowThemeSelector",
            Self::Keybindings => "ShowKeybindings",
            Self::NewWindow => "NewWindow",
            Self::Quit => "Quit",
        }
    }

    pub fn boxed_action(self) -> Box<dyn Action> {
        match self {
            Self::About => Box::new(AboutAction),
            Self::Settings => Box::new(ShowSettings),
            Self::Profiles => Box::new(ShowProfileManager),
            Self::CommandPalette => Box::new(ShowCommandPalette),
            Self::Theme => Box::new(ShowThemeSelector),
            Self::Keybindings => Box::new(ShowKeybindings),
            Self::NewWindow => Box::new(NewWindowAction),
            Self::Quit => Box::new(QuitAction),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMenuEntry {
    Action(AppMenuAction),
    Separator,
    SystemServices,
}

use AppMenuAction::*;
use AppMenuEntry::{Action as MenuAction, Separator, SystemServices};

pub const MACOS_APP_MENU: &[AppMenuEntry] = &[
    MenuAction(About),
    Separator,
    MenuAction(Settings),
    MenuAction(Profiles),
    Separator,
    SystemServices,
    Separator,
    MenuAction(Quit),
];

pub const MACOS_VIEW_MENU: &[AppMenuEntry] = &[
    MenuAction(CommandPalette),
    MenuAction(Theme),
    Separator,
    MenuAction(Keybindings),
];

pub const MACOS_WINDOW_MENU: &[AppMenuEntry] = &[MenuAction(NewWindow)];

pub const TITLE_BAR_MENU: &[AppMenuEntry] = &[
    MenuAction(NewWindow),
    Separator,
    MenuAction(CommandPalette),
    MenuAction(Theme),
    MenuAction(Keybindings),
    Separator,
    MenuAction(Settings),
    MenuAction(Profiles),
    Separator,
    MenuAction(About),
    Separator,
    MenuAction(Quit),
];

pub fn native_menu_items(entries: &[AppMenuEntry]) -> Vec<MenuItem> {
    entries
        .iter()
        .map(|entry| match entry {
            AppMenuEntry::Action(action) => MenuItem::Action {
                name: action.label().into(),
                action: action.boxed_action(),
                os_action: None,
                checked: false,
                disabled: false,
            },
            AppMenuEntry::Separator => MenuItem::separator(),
            AppMenuEntry::SystemServices => {
                MenuItem::os_submenu("Services", SystemMenuType::Services)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_bar_contains_every_cross_platform_native_action() {
        let native_actions: Vec<_> = MACOS_APP_MENU
            .iter()
            .chain(MACOS_VIEW_MENU)
            .chain(MACOS_WINDOW_MENU)
            .filter_map(|entry| match entry {
                AppMenuEntry::Action(action) => Some(*action),
                AppMenuEntry::Separator | AppMenuEntry::SystemServices => None,
            })
            .collect();
        let title_bar_actions: Vec<_> = TITLE_BAR_MENU
            .iter()
            .filter_map(|entry| match entry {
                AppMenuEntry::Action(action) => Some(*action),
                AppMenuEntry::Separator | AppMenuEntry::SystemServices => None,
            })
            .collect();

        assert_eq!(native_actions.len(), title_bar_actions.len());
        assert!(
            native_actions
                .iter()
                .all(|action| title_bar_actions.contains(action))
        );
    }

    #[test]
    fn menus_have_no_adjacent_or_edge_separators() {
        for entries in [
            MACOS_APP_MENU,
            MACOS_VIEW_MENU,
            MACOS_WINDOW_MENU,
            TITLE_BAR_MENU,
        ] {
            assert!(!matches!(entries.first(), Some(AppMenuEntry::Separator)));
            assert!(!matches!(entries.last(), Some(AppMenuEntry::Separator)));
            assert!(
                entries
                    .windows(2)
                    .all(|pair| pair != [AppMenuEntry::Separator, AppMenuEntry::Separator])
            );
        }
    }
}
