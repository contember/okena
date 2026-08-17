//! Files-mode row model — spec §7.
//!
//! Pure. The tree is rebuilt over the *visible* subset, so a directory row never
//! sums files the filter hides. Two flags decide how a directory opens:
//! `expanded_initialized` says whether the user's set is authoritative yet, and
//! until it is, [`default_expanded`] answers instead. The render pass seeds
//! `expanded_dirs` from [`default_expanded_dirs`] and flips the flag, so both
//! answers agree for the row the user first sees.

use super::super::labels::nav as words;
use super::super::labels::role_short;
use super::super::model::{DirNode, FileEntry, Reason, ReasonKind, ReviewModel};
use super::super::state::{NavRowId, RoleFilter};
use okena_core::review::{FileRole, ReviewFileStatus};
use std::collections::{BTreeMap, HashSet};

/// Visible files at or below which the whole tree opens — spec §7.
pub(crate) const EXPAND_ALL_LIMIT: usize = 40;

/// A file row shows the two loudest reasons and no more — spec §7.
const MAX_MARKERS: usize = 2;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NavRow {
    pub id: NavRowId,
    pub depth: usize,
    pub kind: NavRowKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NavRowKind {
    Dir(DirRow),
    File(FileRow),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirRow {
    /// Joined single-child chains keep the whole `src/build`.
    pub name: String,
    pub file_count: usize,
    pub added: u64,
    pub deleted: u64,
    pub expanded: bool,
    pub no_tests: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileRow {
    /// Basename, the elided rename pair, or the whole path while flattened.
    pub name_display: String,
    /// The name the file-type icon is chosen from.
    pub icon_name: String,
    pub added: u64,
    pub deleted: u64,
    /// At most [`MAX_MARKERS`], loudest first.
    pub markers: Vec<Reason>,
    pub role_badge: Option<&'static str>,
    /// Structure never reached the file — spec §7 dims instead of badging.
    pub dimmed: bool,
    pub is_rename: bool,
    /// Full path(s), plus why the row is dimmed.
    pub tooltip: String,
}

/// Indices into `ReviewModel::files` the navigator currently shows.
///
/// Mirrors `DiffViewer::review_visible_files`; the two must stay identical or
/// `↑` `↓` and the tree would disagree about what is on screen.
pub(crate) fn visible_files(
    model: &ReviewModel,
    role_filter: &RoleFilter,
    filter_text: &str,
) -> Vec<usize> {
    let needle = filter_text.to_lowercase();
    model
        .files
        .iter()
        .enumerate()
        .filter(|(_, entry)| role_filter.allows(entry))
        .filter(|(_, entry)| matches_filter(&entry.display_path, &needle))
        .map(|(index, _)| index)
        .collect()
}

pub(crate) fn matches_filter(haystack: &str, lowercase_needle: &str) -> bool {
    lowercase_needle.is_empty() || haystack.to_lowercase().contains(lowercase_needle)
}

/// Every visible row of the Files tree, in display order.
pub(crate) fn nav_rows(
    model: &ReviewModel,
    role_filter: &RoleFilter,
    filter_text: &str,
    expanded_dirs: &HashSet<String>,
    flatten: bool,
    expanded_initialized: bool,
) -> Vec<NavRow> {
    let visible = visible_files(model, role_filter, filter_text);
    if flatten {
        return flat_rows(model, &visible);
    }
    let root = build_tree(model, &visible);
    let untested = untested_dirs(&model.root);
    let mut out = Vec::new();
    emit(Emit {
        dir: &root,
        depth: 0,
        model,
        untested: &untested,
        expanded_dirs,
        expanded_initialized,
        total: visible.len(),
        out: &mut out,
    });
    out
}

/// Directory paths the tree opens with, before the user touches it.
pub(crate) fn default_expanded_dirs(
    model: &ReviewModel,
    role_filter: &RoleFilter,
    filter_text: &str,
) -> Vec<String> {
    let visible = visible_files(model, role_filter, filter_text);
    let root = build_tree(model, &visible);
    let mut out = Vec::new();
    collect_default_expanded(&root, 0, visible.len(), &mut out);
    out
}

/// Spec §7: under the limit everything opens, above it only the top level closes.
pub(crate) fn default_expanded(visible_files: usize, depth: usize) -> bool {
    visible_files <= EXPAND_ALL_LIMIT || depth > 0
}

fn collect_default_expanded(dir: &Dir, depth: usize, total: usize, out: &mut Vec<String>) {
    for child in &dir.dirs {
        if default_expanded(total, depth) {
            out.push(child.path.clone());
        }
        collect_default_expanded(child, depth.saturating_add(1), total, out);
    }
}

// -- tree --------------------------------------------------------------------

/// A directory over the visible subset only.
#[derive(Debug, Default)]
struct Dir {
    name: String,
    path: String,
    dirs: Vec<Dir>,
    /// Indices into `ReviewModel::files`, this directory only.
    files: Vec<usize>,
    file_count: usize,
    added: u64,
    deleted: u64,
}

#[derive(Debug, Default)]
struct DirBuilder {
    name: String,
    path: String,
    children: BTreeMap<String, DirBuilder>,
    files: Vec<usize>,
}

/// The path a file occupies in the tree — head side when it has one.
fn tree_path(entry: &FileEntry) -> &str {
    entry
        .new_path
        .as_deref()
        .or(entry.old_path.as_deref())
        .unwrap_or_default()
}

fn build_tree(model: &ReviewModel, visible: &[usize]) -> Dir {
    let mut root = DirBuilder::default();
    for index in visible {
        let Some(entry) = model.files.get(*index) else {
            continue;
        };
        let path = tree_path(entry);
        let segments: Vec<&str> = path.split('/').collect();
        let mut node = &mut root;
        for segment in segments.iter().take(segments.len().saturating_sub(1)) {
            let child_path = if node.path.is_empty() {
                (*segment).to_string()
            } else {
                format!("{}/{segment}", node.path)
            };
            node = node
                .children
                .entry((*segment).to_string())
                .or_insert_with(|| DirBuilder {
                    name: (*segment).to_string(),
                    path: child_path,
                    ..DirBuilder::default()
                });
        }
        node.files.push(*index);
    }
    finish(root, model)
}

fn finish(builder: DirBuilder, model: &ReviewModel) -> Dir {
    let dirs: Vec<Dir> = builder
        .children
        .into_values()
        .map(|child| join_chain(finish(child, model)))
        .collect();
    let mut files = builder.files;
    files.sort_by(|left, right| {
        let name = |index: &usize| {
            model
                .files
                .get(*index)
                .map(|entry| basename(tree_path(entry)).to_lowercase())
                .unwrap_or_default()
        };
        name(left).cmp(&name(right))
    });
    let mut file_count = files.len();
    let mut added = 0u64;
    let mut deleted = 0u64;
    for index in &files {
        if let Some(entry) = model.files.get(*index) {
            added = added.saturating_add(entry.lines_added);
            deleted = deleted.saturating_add(entry.lines_deleted);
        }
    }
    for child in &dirs {
        file_count = file_count.saturating_add(child.file_count);
        added = added.saturating_add(child.added);
        deleted = deleted.saturating_add(child.deleted);
    }
    Dir {
        name: builder.name,
        path: builder.path,
        dirs,
        files,
        file_count,
        added,
        deleted,
    }
}

/// Collapse `a` → `b` → `c` into one `a/b/c` row.
fn join_chain(mut node: Dir) -> Dir {
    while node.files.is_empty() && node.dirs.len() == 1 {
        let child = node.dirs.remove(0);
        node.name = format!("{}/{}", node.name, child.name);
        node.path = child.path;
        node.dirs = child.dirs;
        node.files = child.files;
    }
    node
}

/// Implementation directories with no test change, top-most only — spec §6.
fn untested_dirs(root: &DirNode) -> HashSet<String> {
    let mut out = HashSet::new();
    collect_untested(root, &mut out);
    out
}

fn collect_untested(node: &DirNode, out: &mut HashSet<String>) {
    if node.no_test_changes {
        out.insert(node.path.clone());
        return;
    }
    for child in &node.children {
        collect_untested(child, out);
    }
}

struct Emit<'a> {
    dir: &'a Dir,
    depth: usize,
    model: &'a ReviewModel,
    untested: &'a HashSet<String>,
    expanded_dirs: &'a HashSet<String>,
    expanded_initialized: bool,
    total: usize,
    out: &'a mut Vec<NavRow>,
}

/// Directories first, then the files that sit in them; both alphabetical.
fn emit(args: Emit<'_>) {
    let Emit {
        dir,
        depth,
        model,
        untested,
        expanded_dirs,
        expanded_initialized,
        total,
        out,
    } = args;
    for child in &dir.dirs {
        let expanded = if expanded_initialized {
            expanded_dirs.contains(&child.path)
        } else {
            default_expanded(total, depth)
        };
        out.push(NavRow {
            id: NavRowId::Dir(child.path.clone()),
            depth,
            kind: NavRowKind::Dir(DirRow {
                name: child.name.clone(),
                file_count: child.file_count,
                added: child.added,
                deleted: child.deleted,
                expanded,
                no_tests: untested.contains(&child.path),
            }),
        });
        if expanded {
            emit(Emit {
                dir: child,
                depth: depth.saturating_add(1),
                model,
                untested,
                expanded_dirs,
                expanded_initialized,
                total,
                out,
            });
        }
    }
    for index in &dir.files {
        if let Some(entry) = model.files.get(*index) {
            out.push(file_nav_row(entry, depth, false));
        }
    }
}

fn flat_rows(model: &ReviewModel, visible: &[usize]) -> Vec<NavRow> {
    let mut indices = visible.to_vec();
    indices.sort_by(|left, right| {
        let path = |index: &usize| {
            model
                .files
                .get(*index)
                .map(|entry| tree_path(entry).to_string())
                .unwrap_or_default()
        };
        path(left).cmp(&path(right))
    });
    indices
        .iter()
        .filter_map(|index| model.files.get(*index))
        .map(|entry| file_nav_row(entry, 0, true))
        .collect()
}

// -- file rows ---------------------------------------------------------------

fn file_nav_row(entry: &FileEntry, depth: usize, flatten: bool) -> NavRow {
    let is_rename = entry.status == ReviewFileStatus::Renamed
        || matches!((&entry.old_path, &entry.new_path), (Some(old), Some(new)) if old != new);
    let name_display = match (&entry.old_path, &entry.new_path) {
        (Some(old), Some(new)) if is_rename => words::rename_display(old, new),
        _ if flatten => tree_path(entry).to_string(),
        _ => basename(tree_path(entry)).to_string(),
    };
    let not_analyzed = entry
        .reasons
        .iter()
        .find(|reason| reason.kind == ReasonKind::NotAnalyzed);
    let tooltip = match not_analyzed {
        Some(reason) => format!("{} \u{00B7} {}", entry.display_path, reason.label),
        None => entry.display_path.clone(),
    };
    NavRow {
        id: NavRowId::File(entry.key.clone()),
        depth,
        kind: NavRowKind::File(FileRow {
            name_display,
            icon_name: basename(tree_path(entry)).to_string(),
            added: entry.lines_added,
            deleted: entry.lines_deleted,
            markers: markers(entry),
            role_badge: role_badge(entry.role),
            dimmed: not_analyzed.is_some(),
            is_rename,
            tooltip,
        }),
    }
}

/// Implementation is the default reading; Unclassified has nothing to say.
fn role_badge(role: FileRole) -> Option<&'static str> {
    match role {
        FileRole::Implementation | FileRole::Unclassified => None,
        other => Some(role_short(other)),
    }
}

/// The loudest two reasons of the file and of the symbols inside it — spec §7.
fn markers(entry: &FileEntry) -> Vec<Reason> {
    let signatures = entry
        .symbols
        .iter()
        .flat_map(|symbol| symbol.reasons.iter())
        .filter(|reason| {
            matches!(
                reason.kind,
                ReasonKind::PublicSignature | ReasonKind::ExportedSignature
            )
        })
        .count();
    let mut candidates: Vec<(u8, Reason)> = entry
        .reasons
        .iter()
        .chain(
            entry
                .symbols
                .iter()
                .flat_map(|symbol| symbol.reasons.iter()),
        )
        .filter_map(|reason| {
            words::file_marker(reason.kind, &reason.label, signatures).map(|label| {
                (
                    words::marker_rank(reason.kind),
                    Reason {
                        kind: reason.kind,
                        label,
                    },
                )
            })
        })
        .collect();
    candidates.sort_by_key(|(rank, _)| *rank);
    let mut out: Vec<Reason> = Vec::with_capacity(MAX_MARKERS);
    for (_, reason) in candidates {
        if out.iter().any(|kept| kept.label == reason.label) {
            continue;
        }
        out.push(reason);
        if out.len() == MAX_MARKERS {
            break;
        }
    }
    out
}

pub(crate) fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures;
    use super::super::super::model::{FileEntry, ReasonKind};
    use super::super::super::state::{NavRowId, RoleFilter, RolePreset};
    use super::{DirRow, FileRow, NavRow, NavRowKind, default_expanded, nav_rows, visible_files};
    use std::collections::HashSet;

    fn tree(
        filter: &RoleFilter,
        text: &str,
        expanded: &HashSet<String>,
        flatten: bool,
    ) -> Vec<NavRow> {
        let model = fixtures::model();
        nav_rows(&model, filter, text, expanded, flatten, false)
    }

    fn dir<'a>(rows: &'a [NavRow], path: &str) -> &'a DirRow {
        rows.iter()
            .find_map(|row| match (&row.id, &row.kind) {
                (NavRowId::Dir(candidate), NavRowKind::Dir(dir)) if candidate == path => Some(dir),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no directory row for {path}"))
    }

    fn file<'a>(rows: &'a [NavRow], name: &str) -> &'a FileRow {
        rows.iter()
            .find_map(|row| match &row.kind {
                NavRowKind::File(file) if file.name_display == name => Some(file),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no file row named {name}"))
    }

    fn names(rows: &[NavRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match &row.kind {
                NavRowKind::Dir(dir) => dir.name.clone(),
                NavRowKind::File(file) => file.name_display.clone(),
            })
            .collect()
    }

    #[test]
    fn the_tree_opens_fully_under_the_limit_and_closes_the_top_level_above_it() {
        assert!(default_expanded(40, 0));
        assert!(!default_expanded(41, 0));
        assert!(
            default_expanded(41, 1),
            "only the top level closes above the limit"
        );
    }

    #[test]
    fn directory_rows_sum_only_the_visible_files() {
        let everything = tree(&RoleFilter::everything(), "", &HashSet::new(), false);
        let src = dir(&everything, "src");
        assert_eq!(src.file_count, 6, "six fixture files live under src/");
        let sum = |pick: fn(&FileEntry) -> u64| -> u64 {
            let model = fixtures::model();
            model
                .files
                .iter()
                .filter(|entry| {
                    entry
                        .new_path
                        .as_deref()
                        .or(entry.old_path.as_deref())
                        .is_some_and(|path| path.starts_with("src/"))
                })
                .map(pick)
                .sum()
        };
        assert_eq!(src.added, sum(|entry| entry.lines_added));
        assert_eq!(src.deleted, sum(|entry| entry.lines_deleted));
        assert!(src.added > 0 && src.deleted > 0);

        // The worker directory holds one implementation file and one test file.
        let worker = dir(&everything, "worker");
        assert_eq!(worker.file_count, 2);
        assert_eq!(worker.added, 208);

        let review_code = tree(
            &RoleFilter::preset(RolePreset::ReviewCode),
            "",
            &HashSet::new(),
            false,
        );
        let worker = dir(&review_code, "worker");
        assert_eq!(worker.file_count, 1, "the test file is filtered out");
        assert_eq!(worker.added, 200);
        assert_eq!(worker.deleted, 60);
    }

    #[test]
    fn single_child_chains_are_joined_into_one_row() {
        let rows = tree(&RoleFilter::everything(), "handler", &HashSet::new(), false);
        assert_eq!(
            names(&rows),
            ["worker", "handler.rs", "handler_test.rs"],
            "worker holds two files, so nothing joins"
        );

        let rows = tree(&RoleFilter::everything(), "logo", &HashSet::new(), false);
        assert_eq!(names(&rows), ["assets", "logo.png"]);

        // One visible file under a two-deep chain joins the whole chain.
        let rows = tree(
            &RoleFilter::everything(),
            "handler_test",
            &HashSet::new(),
            false,
        );
        assert_eq!(names(&rows), ["worker", "handler_test.rs"]);
    }

    #[test]
    fn markers_keep_the_two_loudest_reasons_in_priority_order() {
        let rows = tree(&RoleFilter::everything(), "engine", &HashSet::new(), false);
        let engine = file(&rows, "engine.rs");
        let labels: Vec<&str> = engine
            .markers
            .iter()
            .map(|marker| marker.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["removed", "sig 2"],
            "a removed public symbol outranks the two changed signatures"
        );
        assert!(engine.markers.len() <= 2);
    }

    #[test]
    fn a_rename_shows_its_similarity_and_a_new_file_shows_new() {
        let rows = tree(&RoleFilter::everything(), "motion", &HashSet::new(), false);
        let moved = file(
            &rows,
            "\u{2026}/motion_old.rs \u{2192} \u{2026}/motion_new.rs",
        );
        assert!(moved.is_rename);
        assert_eq!(
            moved
                .markers
                .iter()
                .map(|marker| marker.label.as_str())
                .collect::<Vec<_>>(),
            ["moved 91 %", "86 residual lines"]
        );

        let rows = tree(&RoleFilter::everything(), "src/lib", &HashSet::new(), false);
        let new = file(&rows, "lib.rs");
        assert_eq!(
            new.markers
                .iter()
                .map(|marker| marker.label.as_str())
                .collect::<Vec<_>>(),
            ["new"]
        );
    }

    #[test]
    fn files_structure_never_reached_are_dimmed_and_never_badged() {
        let rows = tree(&RoleFilter::everything(), "app.js", &HashSet::new(), false);
        let unsupported = file(&rows, "app.js");
        assert!(unsupported.dimmed);
        assert!(
            !unsupported
                .markers
                .iter()
                .any(|marker| marker.kind == ReasonKind::NotAnalyzed),
            "the dim is the whole signal"
        );
        assert!(
            unsupported.tooltip.contains("not analyzed"),
            "the reason is on hover instead: {}",
            unsupported.tooltip
        );

        let rows = tree(&RoleFilter::everything(), "engine", &HashSet::new(), false);
        assert!(!file(&rows, "engine.rs").dimmed);
    }

    #[test]
    fn role_badges_appear_only_when_the_role_is_worth_naming() {
        let rows = tree(&RoleFilter::everything(), "", &HashSet::new(), false);
        assert_eq!(file(&rows, "lib.rs").role_badge, None);
        assert_eq!(file(&rows, "logo.png").role_badge, None);
        assert_eq!(file(&rows, "README.md").role_badge, Some("Docs"));
        assert_eq!(file(&rows, "Cargo.toml").role_badge, Some("Config"));
        assert_eq!(file(&rows, "handler_test.rs").role_badge, Some("Tests"));
    }

    #[test]
    fn an_untested_implementation_directory_is_marked_once() {
        let rows = tree(&RoleFilter::everything(), "", &HashSet::new(), false);
        assert!(dir(&rows, "src").no_tests, "src/ changed no test file");
        assert!(
            !dir(&rows, "worker").no_tests,
            "worker/ changed a test next to its implementation"
        );
    }

    #[test]
    fn the_fixture_tree_opens_fully_and_lists_directories_before_files() {
        let rows = tree(&RoleFilter::everything(), "", &HashSet::new(), false);
        assert_eq!(
            names(&rows),
            [
                "assets",
                "logo.png",
                "src",
                "app.js",
                "engine.rs",
                "legacy.rs",
                "lib.rs",
                "\u{2026}/motion_old.rs \u{2192} \u{2026}/motion_new.rs",
                "\u{2026}/old.rs \u{2192} \u{2026}/new.rs",
                "tests",
                "lib.rs",
                "worker",
                "handler.rs",
                "handler_test.rs",
                "Cargo.toml",
                "pnpm-lock.yaml",
                "README.md"
            ]
        );
    }

    #[test]
    fn a_collapsed_directory_hides_its_files() {
        let model = fixtures::model();
        let mut expanded: HashSet<String> = HashSet::new();
        expanded.insert("assets".to_string());
        expanded.insert("tests".to_string());
        expanded.insert("worker".to_string());
        let rows = nav_rows(
            &model,
            &RoleFilter::everything(),
            "",
            &expanded,
            false,
            true,
        );
        assert!(
            !names(&rows).contains(&"engine.rs".to_string()),
            "src/ is not in the expanded set"
        );
        assert!(names(&rows).contains(&"handler.rs".to_string()));
    }

    #[test]
    fn flatten_drops_the_directories_and_shows_whole_paths() {
        let rows = tree(&RoleFilter::everything(), "", &HashSet::new(), true);
        assert!(
            rows.iter()
                .all(|row| matches!(row.kind, NavRowKind::File(_))),
            "a flat list has no directory rows"
        );
        assert!(names(&rows).contains(&"src/engine.rs".to_string()));
        assert!(names(&rows).contains(&"tests/lib.rs".to_string()));
    }

    #[test]
    fn the_visible_set_intersects_the_role_filter_with_the_text_filter() {
        let model = fixtures::model();
        let paths = |filter: &RoleFilter, text: &str| -> Vec<String> {
            visible_files(&model, filter, text)
                .iter()
                .filter_map(|index| model.files.get(*index))
                .map(|entry| entry.display_path.clone())
                .collect()
        };
        assert_eq!(
            paths(&RoleFilter::everything(), "lib"),
            ["src/lib.rs", "tests/lib.rs"]
        );
        assert_eq!(
            paths(&RoleFilter::preset(RolePreset::ReviewCode), "lib"),
            ["src/lib.rs"]
        );
        assert_eq!(paths(&RoleFilter::everything(), "").len(), 13);
    }
}
