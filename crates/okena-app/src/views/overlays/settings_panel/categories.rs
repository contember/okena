#[derive(Clone, PartialEq)]
pub(in crate::views::overlays) enum SettingsCategory {
    General,
    Font,
    Terminal,
    Worktree,
    Hooks,
    Extensions,
    PairedDevices,
    /// Dynamic category for an extension's own settings (keyed by extension ID).
    Extension(String),
}

impl SettingsCategory {
    pub(super) fn label(&self) -> &str {
        match self {
            Self::General => "General",
            Self::Font => "Font",
            Self::Terminal => "Terminal",
            Self::Worktree => "Worktree",
            Self::Hooks => "Hooks",
            Self::Extensions => "Extensions",
            Self::PairedDevices => "Devices",
            Self::Extension(_) => "", // label provided dynamically from registry
        }
    }

    pub(super) fn all() -> &'static [SettingsCategory] {
        &[Self::General, Self::Font, Self::Terminal, Self::Worktree, Self::Hooks, Self::Extensions, Self::PairedDevices]
    }

    /// Categories available in project mode (only hooks for now)
    pub(super) fn project_categories() -> &'static [SettingsCategory] {
        &[Self::Hooks]
    }
}
