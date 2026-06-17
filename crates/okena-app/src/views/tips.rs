//! Curated discoverability tips shown on the empty (no terminal attached)
//! project screen.
//!
//! Each tip is bound to an action name where it has a keybinding, so the
//! shortcut is rendered live from the user's `KeybindingConfig` and stays
//! correct even if they rebind it. Tips without a keybinding carry a short
//! `hint` describing how to trigger them instead (e.g. "right-click", "CLI").
//!
//! The tip set was compiled by auditing the codebase; every entry was
//! verified against the actual implementation. Desktop only.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::keybindings::{format_keystroke, get_config};

/// A single discoverability tip.
pub struct Tip {
    /// The tip copy shown to the user.
    pub text: &'static str,
    /// Action name (a key in `KeybindingConfig.bindings`) whose live shortcut
    /// is shown as a chip. `None` when the tip is not keyboard-triggered.
    pub action: Option<&'static str>,
    /// Static trigger hint shown as a chip when there is no `action`
    /// (e.g. "right-click", "CLI", "settings").
    pub hint: Option<&'static str>,
}

/// Tip triggered by a keybinding — its chip shows the live shortcut for `action`.
const fn key(text: &'static str, action: &'static str) -> Tip {
    Tip { text, action: Some(action), hint: None }
}

/// Tip triggered some other way — its chip shows the static `hint`.
const fn hint(text: &'static str, hint: &'static str) -> Tip {
    Tip { text, action: None, hint: Some(hint) }
}

/// Tip with no chip (an ambient behavior, or keys are already in the text).
const fn plain(text: &'static str) -> Tip {
    Tip { text, action: None, hint: None }
}

/// The curated tip pool.
pub static TIPS: &[Tip] = &[
    // Windows, projects & layout
    key("Open a second window onto the same workspace — each window keeps its own set of visible projects.", "NewWindow"),
    key("Overlay numbers on every pane, then press the digit to jump straight to it.", "TogglePaneSwitcher"),
    key("Zoom the focused terminal full-screen, then cycle the rest with ⌘] / ⌘[.", "ToggleFullscreen"),
    key("Resize every column and pane to match in a single keystroke.", "EqualizeLayout"),
    key("Switch your project grid between columns and rows — handy on a portrait or vertical monitor.", "ToggleProjectLayout"),
    key("Minimize a busy terminal instead of closing it — it keeps running and comes back later.", "MinimizeTerminal"),
    key("Zoom to just the project holding the active terminal; press ⌘0 to show them all again.", "FocusActiveProject"),
    hint("Give each project one of 12 colors so its column is easy to spot.", "right-click a project"),
    hint("Group projects into folders, drag to reorder, and \"Show Only This Folder\" to focus one group.", "drag / right-click"),
    hint("Hide a project from a window without deleting it — it stays in your other windows.", "right-click"),

    // Splits, tabs & navigation
    key("Split a terminal into side-by-side or stacked panes.", "SplitVertical"),
    key("Add tabs inside a pane — double-click a tab to rename, middle-click to close.", "AddTab"),
    plain("Move focus between panes by direction with ⌘⌥ (Ctrl+Alt) + the arrow keys."),

    // Find anything
    key("Open the command palette to find any action — and see its shortcut.", "ShowCommandPalette"),
    key("Jump between projects fast — Enter to focus, Space to show/hide in this window.", "ShowProjectSwitcher"),
    key("Fuzzy-find any file in the project.", "ShowFileSearch"),
    key("Search file contents across the project with literal, regex, or fuzzy matching.", "ShowContentSearch"),

    // Terminal
    key("Jump to the previous or next shell prompt instantly (needs OSC 133 shell integration).", "JumpToPreviousPrompt"),
    hint("⌘/Ctrl-click a file path (file:line:col) or a URL in the terminal to open it in your editor or browser.", "⌘/Ctrl + click"),
    key("Search the terminal scrollback with regex and a live match count.", "Search"),
    hint("Press Shift+Enter to insert a newline for multi-line input without submitting.", "⇧Enter"),
    key("Zoom the font size of just one terminal; press ⌘0 to reset it.", "ZoomIn"),

    // Git & worktrees
    key("Filter branches or create a new one from HEAD, right inside the branch switcher.", "ShowBranchSwitcher"),
    hint("Spin up a git worktree as its own column — even straight from a GitHub PR (needs gh).", "right-click a repo"),
    plain("New worktrees appear automatically and stale ones are cleaned up in the background."),
    plain("In the diff viewer: S toggles side-by-side, W ignores whitespace, Tab switches staged/unstaged."),

    // Services & ports
    hint("Running services show port badges — click one to open it in your browser.", "click a port"),
    hint("Define project services in okena.yaml; Docker Compose is auto-detected too.", "okena.yaml"),
    hint("Services can auto-start on project open and auto-restart on crash, with automatic port detection.", "okena.yaml"),

    // Sessions & safety
    plain("With dtach, tmux, or screen installed, your terminals keep running across app restarts."),
    plain("Closed a busy terminal by accident? You get a few seconds to undo before the process is killed."),
    key("Save, load, and export entire workspace layouts from the session manager.", "ShowSessionManager"),

    // Remote, CLI & web
    hint("Drive any terminal from the shell or an agent: okena run, okena key, okena read, okena ls.", "CLI"),
    hint("Target a specific window from the CLI with --window <id> (or --window main).", "CLI"),
    hint("Enable the remote server and open your terminal in a browser from any device via a pairing code.", "remote"),
    hint("Run okena headless on a server: okena --headless --listen <addr> serves the API with no GUI.", "CLI"),

    // Customize
    key("Switch themes live — Auto, Dark, Light, Pastel Dark, High Contrast, or your own JSON.", "ShowThemeSelector"),
    hint("Run commands automatically on project, terminal, and worktree events; prefix with \"terminal:\" to open them in a pane.", "settings"),
    hint("Set a {shell} wrapper to auto-enter a devcontainer or nix shell for every new terminal.", "settings"),
    hint("Keep separate settings, keybindings, themes, and sessions per profile (work vs. personal).", "command palette"),
    key("Every shortcut is editable — open the keybindings editor to record your own.", "ShowKeybindings"),
    hint("Get notified on bell or OSC alerts from background terminals.", "settings"),
];

/// Returns the tip at `index`, wrapping around the pool.
pub fn tip_at(index: usize) -> &'static Tip {
    &TIPS[index % TIPS.len()]
}

/// Picks a starting index for an empty-state column. The base is randomized
/// once per app launch (seeded from the clock) so the first tip differs each
/// run, then advances sequentially so columns shown together don't repeat.
pub fn next_start_index() -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    static BASE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let base = *BASE.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as usize)
            .unwrap_or(0)
    });
    base.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed))
}

/// The shortcut currently bound to `action`, formatted for display, or `None`
/// if it is unbound. On non-macOS we prefer a Ctrl-based binding so the chip
/// shows the key the user will actually press.
pub fn shortcut_for_action(action: &str) -> Option<String> {
    let config = get_config();
    let entries = config.bindings.get(action)?;
    let chosen = if cfg!(target_os = "macos") {
        entries.iter().find(|e| e.enabled)
    } else {
        entries
            .iter()
            .find(|e| e.enabled && e.keystroke.contains("ctrl"))
            .or_else(|| entries.iter().find(|e| e.enabled))
    }?;
    Some(format_keystroke(&chosen.keystroke))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tip_at_wraps_around() {
        let len = TIPS.len();
        assert!(len > 0);
        // Same tip is returned for index and index + len.
        assert!(std::ptr::eq(tip_at(0), tip_at(len)));
        assert!(std::ptr::eq(tip_at(3), tip_at(len + 3)));
    }

    #[test]
    fn every_tip_has_exactly_one_chip_source_or_none() {
        // A tip never carries both an action and a hint (the chip would be
        // ambiguous); it has at most one chip source.
        for tip in TIPS {
            assert!(
                !(tip.action.is_some() && tip.hint.is_some()),
                "tip has both action and hint: {}",
                tip.text
            );
        }
    }

    #[test]
    fn next_start_index_advances() {
        let a = next_start_index();
        let b = next_start_index();
        assert_ne!(a, b);
    }
}
