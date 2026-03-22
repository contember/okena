//! GPUI keybinding actions for terminal views.
//!
//! These actions are used by terminal pane views for keyboard shortcuts.
//! The main app's keybindings module must register these in addition to its own.

gpui::actions!(
    okena_views_terminal,
    [
        Cancel,
        SendEscape,
        SplitVertical,
        SplitHorizontal,
        AddTab,
        CloseTerminal,
        MinimizeTerminal,
        FocusNextTerminal,
        FocusPrevTerminal,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        Copy,
        Paste,
        Search,
        SearchNext,
        SearchPrev,
        CloseSearch,
        SendTab,
        SendBacktab,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        ToggleFullscreen,
        FullscreenNextTerminal,
        FullscreenPrevTerminal,
    ]
);
