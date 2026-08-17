//! Roles-menu model — spec §7: presets first, then all eleven roles with
//! counts, then the two saved filters. Pure; the menu only renders this.

use super::super::labels::{role_label, role_short};
use super::super::model::ReviewModel;
use super::super::state::{ALL_ROLES, RoleFilter, RolePreset, is_likely_mechanical};
use okena_core::review::FileRole;

/// Presets in the order the menu offers them.
const PRESETS: [RolePreset; 3] = [
    RolePreset::ReviewCode,
    RolePreset::Supporting,
    RolePreset::Everything,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RolesMenu {
    pub presets: Vec<PresetRow>,
    pub roles: Vec<RoleRow>,
    pub saved: Vec<SavedRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PresetRow {
    pub preset: RolePreset,
    pub label: &'static str,
    /// The roles the preset stands for, so the name is never the only clue.
    pub hint: String,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RoleRow {
    pub role: FileRole,
    pub label: &'static str,
    pub count: usize,
    pub checked: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SavedFilter {
    LikelyMechanical,
    NotAnalyzed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedRow {
    pub filter: SavedFilter,
    pub label: &'static str,
    /// `17 moves` / `185` — what turning it on would leave.
    pub note: String,
    pub checked: bool,
}

pub(crate) fn roles_menu(model: &ReviewModel, filter: &RoleFilter) -> RolesMenu {
    RolesMenu {
        presets: PRESETS
            .into_iter()
            .map(|preset| PresetRow {
                preset,
                label: preset.label(),
                hint: preset_hint(preset),
                active: filter.preset == preset,
            })
            .collect(),
        roles: ALL_ROLES
            .into_iter()
            .map(|role| RoleRow {
                role,
                label: role_label(role),
                count: model
                    .files
                    .iter()
                    .filter(|entry| entry.role == role)
                    .count(),
                checked: filter.roles.contains(role),
            })
            .collect(),
        saved: vec![
            SavedRow {
                filter: SavedFilter::LikelyMechanical,
                label: super::super::labels::nav::LIKELY_MECHANICAL,
                note: moves_note(model),
                checked: filter.likely_mechanical_only,
            },
            SavedRow {
                filter: SavedFilter::NotAnalyzed,
                label: super::super::labels::nav::NOT_ANALYZED_ONLY,
                note: not_analyzed_count(model).to_string(),
                checked: filter.not_analyzed_only,
            },
        ],
    }
}

/// `Impl + Config + Unclassified`; *Everything* names its own size instead.
fn preset_hint(preset: RolePreset) -> String {
    match preset.roles() {
        Some(roles) if preset == RolePreset::Everything => format!("all {}", roles.len()),
        Some(roles) => roles.iter().map(role_short).collect::<Vec<_>>().join(" + "),
        None => String::new(),
    }
}

fn moves_note(model: &ReviewModel) -> String {
    let moves = model
        .files
        .iter()
        .filter(|entry| is_likely_mechanical(entry))
        .count();
    if moves == 1 {
        "1 move".to_string()
    } else {
        format!("{moves} moves")
    }
}

fn not_analyzed_count(model: &ReviewModel) -> usize {
    model
        .files
        .iter()
        .filter(|entry| !entry.analysis.is_analyzed())
        .count()
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::state::{RoleFilter, RolePreset};
    use super::{SavedFilter, roles_menu};
    use okena_core::review::FileRole;

    #[test]
    fn presets_come_first_and_spell_out_the_roles_they_stand_for() {
        let model = fixtures::model();
        let menu = roles_menu(&model, &RoleFilter::everything());
        let labels: Vec<&str> = menu.presets.iter().map(|row| row.label).collect();
        assert_eq!(labels, ["Review code", "Supporting", "Everything"]);
        assert_eq!(menu.presets[0].hint, "Impl + Config + Unclassified");
        assert_eq!(
            menu.presets[1].hint,
            "Tests + Fixtures + Snapshots + Examples + Docs"
        );
        assert_eq!(menu.presets[2].hint, "all 11");
        assert!(menu.presets[2].active, "Everything is the default");
        assert!(!menu.presets[0].active);
    }

    #[test]
    fn every_role_is_listed_with_the_files_it_holds() {
        let model = fixtures::model();
        let menu = roles_menu(&model, &RoleFilter::preset(RolePreset::ReviewCode));
        assert_eq!(menu.roles.len(), 11, "no role may vanish from the menu");
        assert!(menu.roles.iter().all(|row| !row.label.is_empty()));

        let count = |role: FileRole| {
            menu.roles
                .iter()
                .find(|row| row.role == role)
                .map(|row| row.count)
                .expect("every role is listed")
        };
        assert_eq!(count(FileRole::Implementation), 7);
        assert_eq!(count(FileRole::Test), 2);
        assert_eq!(count(FileRole::Documentation), 1);
        assert_eq!(count(FileRole::Configuration), 1);
        assert_eq!(count(FileRole::Lockfile), 1);
        assert_eq!(count(FileRole::Unclassified), 1);
        assert_eq!(count(FileRole::Generated), 0, "zero is still listed");

        let checked: Vec<FileRole> = menu
            .roles
            .iter()
            .filter(|row| row.checked)
            .map(|row| row.role)
            .collect();
        assert_eq!(
            checked,
            [
                FileRole::Implementation,
                FileRole::Configuration,
                FileRole::Unclassified
            ]
        );
    }

    #[test]
    fn the_saved_filters_carry_their_own_counts() {
        let model = fixtures::model();
        let mut filter = RoleFilter::everything();
        filter.not_analyzed_only = true;
        let menu = roles_menu(&model, &filter);

        // Each note promises exactly what turning that filter on would leave.
        let leaves = |narrow: &RoleFilter| {
            model
                .files
                .iter()
                .filter(|entry| narrow.allows(entry))
                .count()
        };
        let mut mechanical_only = RoleFilter::everything();
        mechanical_only.likely_mechanical_only = true;
        let mut analyzed_only = RoleFilter::everything();
        analyzed_only.not_analyzed_only = true;

        let mechanical = &menu.saved[0];
        assert_eq!(mechanical.filter, SavedFilter::LikelyMechanical);
        assert_eq!(leaves(&mechanical_only), 1, "src/old.rs → src/new.rs");
        assert_eq!(mechanical.note, "1 move");
        assert!(!mechanical.checked);

        let analyzed = &menu.saved[1];
        assert_eq!(analyzed.filter, SavedFilter::NotAnalyzed);
        assert_eq!(analyzed.note, leaves(&analyzed_only).to_string());
        assert!(
            leaves(&analyzed_only) > leaves(&mechanical_only),
            "structure reaches only a few fixture files"
        );
        assert!(analyzed.checked);
    }
}
