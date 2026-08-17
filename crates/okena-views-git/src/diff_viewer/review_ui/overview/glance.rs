//! "Change at a glance" — the headline and the volume legend. Pure; the render
//! pass only paints what these return.

use super::super::labels::facts as words;
use super::super::labels::role_label;
use super::super::model::{ReviewModel, VolumeRow};
use okena_core::review::FileRole;

/// Below this the Overview stacks its two columns — spec §12.
const NARROW_WIDTH: f32 = 1_000.0;

/// The one number the Overview leads with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Headline {
    pub main: String,
    pub sub: String,
}

pub(crate) fn is_narrow(width: f32) -> bool {
    width < NARROW_WIDTH
}

/// Implementation volume, or — when nothing implements anything — the largest role.
pub(crate) fn headline(model: &ReviewModel) -> Headline {
    let Some(row) = headline_row(model) else {
        return Headline {
            main: words::NOTHING_CHANGED.to_string(),
            sub: String::new(),
        };
    };
    let role = role_label(row.role);
    // Binary-only comparisons have no lines to share out — spec §12.
    if model.total_changed_lines == 0 {
        return Headline {
            main: words::headline_files(role, row.files),
            sub: words::headline_share_of_files(row.percent, model.files.len()),
        };
    }
    Headline {
        main: words::headline_lines(role, row.lines, deletions_dominate(model, row.role)),
        sub: words::headline_share_of_lines(row.percent, model.total_changed_lines, row.files),
    }
}

/// Implementation when it changed at all, else the role that changed the most.
fn headline_row(model: &ReviewModel) -> Option<&VolumeRow> {
    let implementation = model
        .volume
        .iter()
        .find(|row| row.role == FileRole::Implementation && row.files > 0);
    if implementation.is_some() {
        return implementation;
    }
    model
        .volume
        .iter()
        .filter(|row| row.files > 0)
        .fold(None, |best: Option<&VolumeRow>, row| match best {
            // Ties keep the earlier role, so the order stays the one the menu uses.
            Some(best) if (best.lines, best.files) >= (row.lines, row.files) => Some(best),
            _ => Some(row),
        })
}

fn deletions_dominate(model: &ReviewModel, role: FileRole) -> bool {
    let (added, deleted) = model
        .files
        .iter()
        .filter(|entry| entry.role == role)
        .fold((0u64, 0u64), |(added, deleted), entry| {
            (
                added.saturating_add(entry.lines_added),
                deleted.saturating_add(entry.lines_deleted),
            )
        });
    deleted > added
}

/// The legend rows worth a line: the model keeps all 11 roles, display drops the
/// ones nothing touched — spec §5.
pub(crate) fn legend_rows(model: &ReviewModel) -> Vec<&VolumeRow> {
    model.volume.iter().filter(|row| row.files > 0).collect()
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::super::super::model::ReviewModel;
    use super::{headline, is_narrow, legend_rows};
    use okena_core::review::{FileRole, ReviewInventory};
    use okena_git::DiffMode;

    fn model_of(inventory: &ReviewInventory) -> ReviewModel {
        build_review_model(ModelInputs {
            inventory: Some(inventory),
            inventory_error: None,
            structure: None,
            structure_state: StructureLoad::NotStarted,
            diff_mode: &DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
        })
    }

    #[test]
    fn the_headline_leads_with_implementation_lines() {
        let model = fixtures::model();
        let headline = headline(&model);
        assert!(
            headline.main.starts_with("Implementation "),
            "{}",
            headline.main
        );
        assert!(headline.main.ends_with(" lines"), "{}", headline.main);
        assert!(
            headline.sub.contains(" % of "),
            "the share names the comparison total: {}",
            headline.sub
        );
    }

    #[test]
    fn a_deletion_heavy_role_carries_the_sign() {
        let mut model = fixtures::model();
        for entry in &mut model.files {
            if entry.role == FileRole::Implementation {
                std::mem::swap(&mut entry.lines_added, &mut entry.lines_deleted);
            }
        }
        assert!(
            headline(&model).main.contains('\u{2212}'),
            "{}",
            headline(&model).main
        );
    }

    #[test]
    fn a_binary_only_comparison_counts_files_instead_of_lines() {
        let inventory = fixtures::inventory_binary_only();
        let model = model_of(&inventory);
        let headline = headline(&model);
        assert_eq!(headline.main, "Unclassified 2 files");
        assert_eq!(headline.sub, "100 % of 2 files");
    }

    #[test]
    fn an_empty_comparison_says_nothing_changed() {
        let inventory = fixtures::empty_inventory();
        let model = model_of(&inventory);
        assert_eq!(headline(&model).main, "No files changed");
        assert!(headline(&model).sub.is_empty());
    }

    #[test]
    fn the_legend_drops_the_roles_nothing_touched() {
        let model = fixtures::model();
        let rows = legend_rows(&model);
        assert!(!rows.is_empty());
        assert!(
            rows.iter().all(|row| row.files > 0),
            "a zero-file role has nothing to show"
        );
        assert!(
            rows.len() < model.volume.len(),
            "the model keeps all 11 roles"
        );
    }

    #[test]
    fn the_overview_stacks_below_a_thousand_pixels() {
        assert!(is_narrow(999.0));
        assert!(!is_narrow(1_000.0));
        assert!(!is_narrow(1_400.0));
    }
}
