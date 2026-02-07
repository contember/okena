use std::collections::HashMap;

use super::types::ActionDescription;
use super::{
    AddTab, Cancel, CheckForUpdates, ClearFocus, CloseSearch, CloseTerminal, Copy,
    CreateWorktree, FocusDown, FocusLeft, FocusNextTerminal, FocusPrevTerminal, FocusRight,
    FocusUp, FullscreenNextTerminal, FullscreenPrevTerminal, InstallUpdate, MinimizeTerminal,
    NewProject, OpenSettingsFile, Paste, ResetZoom, ScrollDown, ScrollUp, Search, SearchNext,
    SearchPrev, SendEscape, ShowCommandPalette, ShowDiffViewer, ShowFileSearch, ShowKeybindings,
    ShowProjectSwitcher, ShowSessionManager, ShowSettings, ShowThemeSelector, SplitHorizontal,
    SplitVertical, ToggleFullscreen, ToggleSidebar, ToggleSidebarAutoHide, ZoomIn, ZoomOut,
};

/// Get human-readable descriptions for all actions
pub fn get_action_descriptions() -> HashMap<&'static str, ActionDescription> {
    let mut map = HashMap::new();

    // Global actions
    map.insert(
        "Cancel",
        ActionDescription {
            name: "Cancel",
            description: "Close overlay, cancel rename, or dismiss",
            category: "Global",
            factory: || Box::new(Cancel),
        },
    );
    map.insert(
        "SendEscape",
        ActionDescription {
            name: "Send Escape",
            description: "Send escape key to terminal",
            category: "Terminal",
            factory: || Box::new(SendEscape),
        },
    );
    map.insert(
        "ToggleSidebar",
        ActionDescription {
            name: "Toggle Sidebar",
            description: "Show or hide the sidebar",
            category: "Global",
            factory: || Box::new(ToggleSidebar),
        },
    );
    map.insert(
        "ToggleSidebarAutoHide",
        ActionDescription {
            name: "Toggle Auto-Hide",
            description: "Enable or disable sidebar auto-hide mode",
            category: "Global",
            factory: || Box::new(ToggleSidebarAutoHide),
        },
    );
    map.insert(
        "ClearFocus",
        ActionDescription {
            name: "Clear Focus",
            description: "Clear focus and show all projects",
            category: "Global",
            factory: || Box::new(ClearFocus),
        },
    );

    // Fullscreen actions
    map.insert(
        "ToggleFullscreen",
        ActionDescription {
            name: "Toggle Fullscreen",
            description: "Toggle fullscreen mode for focused terminal",
            category: "Fullscreen",
            factory: || Box::new(ToggleFullscreen),
        },
    );
    map.insert(
        "FullscreenNextTerminal",
        ActionDescription {
            name: "Next Terminal",
            description: "Switch to next terminal in fullscreen",
            category: "Fullscreen",
            factory: || Box::new(FullscreenNextTerminal),
        },
    );
    map.insert(
        "FullscreenPrevTerminal",
        ActionDescription {
            name: "Previous Terminal",
            description: "Switch to previous terminal in fullscreen",
            category: "Fullscreen",
            factory: || Box::new(FullscreenPrevTerminal),
        },
    );

    // Terminal pane actions
    map.insert(
        "SplitVertical",
        ActionDescription {
            name: "Split Vertical",
            description: "Split the terminal vertically",
            category: "Terminal",
            factory: || Box::new(SplitVertical),
        },
    );
    map.insert(
        "SplitHorizontal",
        ActionDescription {
            name: "Split Horizontal",
            description: "Split the terminal horizontally",
            category: "Terminal",
            factory: || Box::new(SplitHorizontal),
        },
    );
    map.insert(
        "AddTab",
        ActionDescription {
            name: "Add Tab",
            description: "Add a new tab (creates tab group if needed)",
            category: "Terminal",
            factory: || Box::new(AddTab),
        },
    );
    map.insert(
        "CloseTerminal",
        ActionDescription {
            name: "Close Terminal",
            description: "Close the current terminal",
            category: "Terminal",
            factory: || Box::new(CloseTerminal),
        },
    );
    map.insert(
        "MinimizeTerminal",
        ActionDescription {
            name: "Minimize Terminal",
            description: "Minimize/detach the terminal",
            category: "Terminal",
            factory: || Box::new(MinimizeTerminal),
        },
    );
    map.insert(
        "Copy",
        ActionDescription {
            name: "Copy",
            description: "Copy selected text",
            category: "Terminal",
            factory: || Box::new(Copy),
        },
    );
    map.insert(
        "Paste",
        ActionDescription {
            name: "Paste",
            description: "Paste from clipboard",
            category: "Terminal",
            factory: || Box::new(Paste),
        },
    );
    map.insert(
        "ScrollUp",
        ActionDescription {
            name: "Scroll Up",
            description: "Scroll terminal output up",
            category: "Terminal",
            factory: || Box::new(ScrollUp),
        },
    );
    map.insert(
        "ScrollDown",
        ActionDescription {
            name: "Scroll Down",
            description: "Scroll terminal output down",
            category: "Terminal",
            factory: || Box::new(ScrollDown),
        },
    );
    map.insert(
        "Search",
        ActionDescription {
            name: "Search",
            description: "Open search in terminal",
            category: "Terminal",
            factory: || Box::new(Search),
        },
    );
    map.insert(
        "SearchNext",
        ActionDescription {
            name: "Search Next",
            description: "Find next search match",
            category: "Search",
            factory: || Box::new(SearchNext),
        },
    );
    map.insert(
        "SearchPrev",
        ActionDescription {
            name: "Search Previous",
            description: "Find previous search match",
            category: "Search",
            factory: || Box::new(SearchPrev),
        },
    );
    map.insert(
        "CloseSearch",
        ActionDescription {
            name: "Close Search",
            description: "Close search panel",
            category: "Search",
            factory: || Box::new(CloseSearch),
        },
    );

    // Zoom actions
    map.insert(
        "ZoomIn",
        ActionDescription {
            name: "Zoom In",
            description: "Increase terminal font size",
            category: "Terminal",
            factory: || Box::new(ZoomIn),
        },
    );
    map.insert(
        "ZoomOut",
        ActionDescription {
            name: "Zoom Out",
            description: "Decrease terminal font size",
            category: "Terminal",
            factory: || Box::new(ZoomOut),
        },
    );
    map.insert(
        "ResetZoom",
        ActionDescription {
            name: "Reset Zoom",
            description: "Reset terminal font size to default",
            category: "Terminal",
            factory: || Box::new(ResetZoom),
        },
    );

    // Navigation actions
    map.insert(
        "FocusLeft",
        ActionDescription {
            name: "Focus Left",
            description: "Move focus to the left terminal",
            category: "Navigation",
            factory: || Box::new(FocusLeft),
        },
    );
    map.insert(
        "FocusRight",
        ActionDescription {
            name: "Focus Right",
            description: "Move focus to the right terminal",
            category: "Navigation",
            factory: || Box::new(FocusRight),
        },
    );
    map.insert(
        "FocusUp",
        ActionDescription {
            name: "Focus Up",
            description: "Move focus to the terminal above",
            category: "Navigation",
            factory: || Box::new(FocusUp),
        },
    );
    map.insert(
        "FocusDown",
        ActionDescription {
            name: "Focus Down",
            description: "Move focus to the terminal below",
            category: "Navigation",
            factory: || Box::new(FocusDown),
        },
    );
    map.insert(
        "FocusNextTerminal",
        ActionDescription {
            name: "Focus Next",
            description: "Move focus to the next terminal",
            category: "Navigation",
            factory: || Box::new(FocusNextTerminal),
        },
    );
    map.insert(
        "FocusPrevTerminal",
        ActionDescription {
            name: "Focus Previous",
            description: "Move focus to the previous terminal",
            category: "Navigation",
            factory: || Box::new(FocusPrevTerminal),
        },
    );

    // Project actions
    map.insert(
        "NewProject",
        ActionDescription {
            name: "New Project",
            description: "Create a new project",
            category: "Project",
            factory: || Box::new(NewProject),
        },
    );
    map.insert(
        "CreateWorktree",
        ActionDescription {
            name: "Create Worktree",
            description: "Create a git worktree from the focused project",
            category: "Project",
            factory: || Box::new(CreateWorktree),
        },
    );
    map.insert(
        "ShowKeybindings",
        ActionDescription {
            name: "Show Keybindings",
            description: "Display keybinding help",
            category: "Global",
            factory: || Box::new(ShowKeybindings),
        },
    );
    map.insert(
        "ShowSessionManager",
        ActionDescription {
            name: "Session Manager",
            description: "Open session manager to save/load workspaces",
            category: "Global",
            factory: || Box::new(ShowSessionManager),
        },
    );
    map.insert(
        "ShowThemeSelector",
        ActionDescription {
            name: "Theme Selector",
            description: "Open theme selector to change appearance",
            category: "Global",
            factory: || Box::new(ShowThemeSelector),
        },
    );
    map.insert(
        "ShowCommandPalette",
        ActionDescription {
            name: "Command Palette",
            description: "Open command palette for quick access to all commands",
            category: "Global",
            factory: || Box::new(ShowCommandPalette),
        },
    );
    map.insert(
        "ShowSettings",
        ActionDescription {
            name: "Settings",
            description: "Open settings panel",
            category: "Global",
            factory: || Box::new(ShowSettings),
        },
    );
    map.insert(
        "OpenSettingsFile",
        ActionDescription {
            name: "Open Settings File",
            description: "Open settings JSON file in default editor",
            category: "Global",
            factory: || Box::new(OpenSettingsFile),
        },
    );
    map.insert(
        "ShowFileSearch",
        ActionDescription {
            name: "Go to File",
            description: "Quick file search in the active project",
            category: "Global",
            factory: || Box::new(ShowFileSearch),
        },
    );
    map.insert(
        "ShowProjectSwitcher",
        ActionDescription {
            name: "Switch Project",
            description: "Quick project navigation (Enter=focus, Space=toggle visibility)",
            category: "Global",
            factory: || Box::new(ShowProjectSwitcher),
        },
    );
    map.insert(
        "ShowDiffViewer",
        ActionDescription {
            name: "Show Git Diff",
            description: "View git diff for the current project",
            category: "Git",
            factory: || Box::new(ShowDiffViewer),
        },
    );
    map.insert(
        "CheckForUpdates",
        ActionDescription {
            name: "Check for Updates",
            description: "Check for a new version of Okena",
            category: "Global",
            factory: || Box::new(CheckForUpdates),
        },
    );
    map.insert(
        "InstallUpdate",
        ActionDescription {
            name: "Install Update",
            description: "Install a downloaded update",
            category: "Global",
            factory: || Box::new(InstallUpdate),
        },
    );

    map
}
