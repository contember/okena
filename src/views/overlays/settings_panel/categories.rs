#[derive(Clone, Copy, PartialEq)]
pub(in crate::views::overlays) enum SettingsCategory {
    General,
    Font,
    Terminal,
    Worktree,
    Hooks,
    PairedDevices,
}

impl SettingsCategory {
    pub(super) fn label(&self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Font => "Font",
            Self::Terminal => "Terminal",
            Self::Worktree => "Worktree",
            Self::Hooks => "Hooks",
            Self::PairedDevices => "Devices",
        }
    }

    pub(super) fn all() -> &'static [SettingsCategory] {
        &[Self::General, Self::Font, Self::Terminal, Self::Worktree, Self::Hooks, Self::PairedDevices]
    }

    /// Categories available in project mode (only hooks for now)
    pub(super) fn project_categories() -> &'static [SettingsCategory] {
        &[Self::Hooks]
    }
}
