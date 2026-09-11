#[derive(Clone, PartialEq)]
pub(in crate::views::overlays) enum SettingsCategory {
    General,
    Font,
    Terminal,
    Worktree,
    Harness,
    Specs,
    Knowledge,
    Tasks,
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
            Self::Harness => "Harness",
            Self::Specs => "Specs",
            Self::Knowledge => "Knowledge",
            Self::Tasks => "Tasks",
            Self::Hooks => "Hooks",
            Self::Extensions => "Extensions",
            Self::PairedDevices => "Devices",
            Self::Extension(_) => "", // label provided dynamically from registry
        }
    }

    pub(super) fn all() -> &'static [SettingsCategory] {
        &[
            Self::General,
            Self::Font,
            Self::Terminal,
            Self::Worktree,
            Self::Harness,
            Self::Specs,
            Self::Knowledge,
            Self::Tasks,
            Self::Hooks,
            Self::Extensions,
            Self::PairedDevices,
        ]
    }

    /// Stable id used to open the panel on a given page from elsewhere.
    ///
    /// A string crosses crate boundaries that the enum cannot: the harness
    /// views live in this crate but request the page through the shared
    /// request broker, which does not know this type.
    pub(super) fn slug(&self) -> &str {
        match self {
            Self::General => "general",
            Self::Font => "font",
            Self::Terminal => "terminal",
            Self::Worktree => "worktree",
            Self::Harness => "harness",
            Self::Specs => "specs",
            Self::Knowledge => "knowledge",
            Self::Tasks => "tasks",
            Self::Hooks => "hooks",
            Self::Extensions => "extensions",
            Self::PairedDevices => "devices",
            Self::Extension(id) => id,
        }
    }

    /// Resolve a slug back to a page. Unknown slugs open the default page
    /// rather than failing — a stale link should still open settings.
    pub(super) fn from_slug(slug: &str) -> Option<SettingsCategory> {
        Self::all().iter().find(|c| c.slug() == slug).cloned()
    }

    /// Categories available in project mode (only hooks for now)
    pub(super) fn project_categories() -> &'static [SettingsCategory] {
        &[Self::Hooks]
    }
}
