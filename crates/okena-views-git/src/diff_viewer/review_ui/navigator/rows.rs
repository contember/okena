//! Files-mode row model — spec §7.
//!
//! Pure. The tree is rebuilt over the *visible* subset, so a directory row never
//! sums files the filter hides. Two flags decide how a directory opens:
//! `expanded_initialized` says whether the user's set is authoritative yet, and
//! until it is, [`default_expanded`] answers instead. The render pass seeds
//! `expanded_dirs` from [`default_expanded_dirs`] and flips the flag, so both
//! answers agree for the row the user first sees.

use super::super::labels::calls::{self, CALL_TEXT_CHARS};
use super::super::labels::nav as words;
use super::super::labels::role_short;
use super::super::model::{
    AttentionTarget, DirNode, FileEntry, KindGlyph, Reason, ReasonKind, ReviewModel, SymbolEntry,
};
use super::super::state::{NavRowId, ReviewUiState, RoleFilter};
use okena_core::review::{FileRole, ReviewFileStatus};
use okena_review::CallChangeKind;
use std::collections::{BTreeMap, HashSet};

/// Visible files at or below which the whole tree opens — spec §7.
pub(crate) const EXPAND_ALL_LIMIT: usize = 40;

/// A file row shows the two loudest reasons and no more — spec §7.
const MAX_MARKERS: usize = 2;

/// Call lines one symbol contributes to the outline; the rest are counted.
const MAX_OUTLINE_CALLS: usize = 6;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NavRow {
    /// `None` on a detail line: it is read, not walked by `↑` `↓`.
    pub id: Option<NavRowId>,
    pub depth: usize,
    pub kind: NavRowKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum NavRowKind {
    Dir(DirRow),
    File(FileRow),
    Symbol(SymbolRow),
    Detail(DetailRow),
}

/// One changed symbol, inlined under its file — spec §7.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SymbolRow {
    pub name: String,
    pub glyph: KindGlyph,
    pub added: u64,
    pub deleted: u64,
    /// At most [`MAX_MARKERS`]; the detail lines below say the rest.
    pub markers: Vec<Reason>,
    /// The qualified name — the row itself has one line for the short one.
    pub tooltip: String,
    pub target: AttentionTarget,
}

/// What one detail line under a symbol states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetailKind {
    Signature,
    Call(CallChangeKind),
    /// `… 4 more` — the calls the outline left out; the details bar has them.
    More,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DetailRow {
    pub kind: DetailKind,
    /// One line: the signature pair, or the call and the branch it sits in.
    pub text: String,
    /// The symbol the line belongs to; a click opens it.
    pub target: AttentionTarget,
    /// Position under the symbol, so the element id stays unique.
    pub position: usize,
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
    /// Every file under it shares one role worth naming (`Tests`, `Docs`…),
    /// so the directory carries the badge once and its files carry none.
    pub role_badge: Option<&'static str>,
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
    /// The outline is on and this file has symbols under it, so the row is the
    /// header of a block and not just another row.
    pub outlined: bool,
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

/// The navigator's one reading of "not analyzed": structure landed and did not
/// reach this file. The ranking already gates that on structure being present,
/// so its `NotAnalyzed` reason is the authority — while structure is still
/// loading no file is dim and nothing is counted.
pub(crate) fn not_analyzed(entry: &FileEntry) -> bool {
    entry
        .reasons
        .iter()
        .any(|reason| reason.kind == ReasonKind::NotAnalyzed)
}

pub(crate) fn not_analyzed_count(model: &ReviewModel) -> usize {
    model
        .files
        .iter()
        .filter(|entry| not_analyzed(entry))
        .count()
}

/// The ids `↑` `↓` walk; a detail line has none, so the cursor skips it.
pub(crate) fn row_ids(rows: &[NavRow]) -> Vec<NavRowId> {
    rows.iter().filter_map(|row| row.id.clone()).collect()
}

/// What the tree is built from; every field is client state, never the model.
#[derive(Clone, Copy)]
pub(crate) struct TreeArgs<'a> {
    pub role_filter: &'a RoleFilter,
    pub filter_text: &'a str,
    pub expanded_dirs: &'a HashSet<String>,
    pub flatten: bool,
    pub expanded_initialized: bool,
    /// Inline every file's changed symbols and what changed in them.
    pub outline: bool,
}

impl<'a> TreeArgs<'a> {
    pub(crate) fn of(state: &'a ReviewUiState) -> Self {
        Self {
            role_filter: &state.role_filter,
            filter_text: &state.filter_text,
            expanded_dirs: &state.expanded_dirs,
            flatten: state.flatten,
            expanded_initialized: state.expanded_initialized,
            outline: state.outline_inline,
        }
    }
}

/// Every visible row of the Files tree, in display order.
pub(crate) fn nav_rows(model: &ReviewModel, args: &TreeArgs<'_>) -> Vec<NavRow> {
    let visible = visible_files(model, args.role_filter, args.filter_text);
    if args.flatten {
        return flat_rows(model, &visible, args.outline);
    }
    let root = build_tree(model, &visible);
    let untested = untested_dirs(&model.root);
    let mut out = Vec::new();
    emit(Emit {
        dir: &root,
        depth: 0,
        model,
        untested: &untested,
        expanded_dirs: args.expanded_dirs,
        expanded_initialized: args.expanded_initialized,
        total: visible.len(),
        badged: false,
        outline: args.outline,
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
    /// The one role every file under this directory has, when there is one.
    uniform_role: Option<FileRole>,
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
    let uniform_role = uniform_role(
        files
            .iter()
            .filter_map(|index| model.files.get(*index))
            .map(|entry| Some(entry.role))
            .chain(dirs.iter().map(|child| child.uniform_role)),
    );
    Dir {
        name: builder.name,
        path: builder.path,
        dirs,
        files,
        file_count,
        added,
        deleted,
        uniform_role,
    }
}

/// `Some(role)` when every member agrees on one; a `None` member (a mixed
/// child directory) or two different roles make the whole mixed.
fn uniform_role(mut members: impl Iterator<Item = Option<FileRole>>) -> Option<FileRole> {
    let first = members.next()??;
    members.all(|member| member == Some(first)).then_some(first)
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

/// A joined chain stands for several directories at once, so the flag may sit
/// on any of them — the visible tree joins differently than the model's does.
fn covers_untested(name: &str, path: &str, untested: &HashSet<String>) -> bool {
    let joined = name.split('/').count();
    let segments: Vec<&str> = path.split('/').collect();
    let first = segments.len().saturating_sub(joined);
    (first..segments.len()).any(|last| untested.contains(&segments[..=last].join("/")))
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
    /// A directory above already carries the role badge.
    badged: bool,
    outline: bool,
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
        badged,
        outline,
        out,
    } = args;
    for child in &dir.dirs {
        let expanded = if expanded_initialized {
            expanded_dirs.contains(&child.path)
        } else {
            default_expanded(total, depth)
        };
        let role_badge = if badged {
            None
        } else {
            child.uniform_role.and_then(role_badge)
        };
        out.push(NavRow {
            id: Some(NavRowId::Dir(child.path.clone())),
            depth,
            kind: NavRowKind::Dir(DirRow {
                name: child.name.clone(),
                file_count: child.file_count,
                added: child.added,
                deleted: child.deleted,
                expanded,
                no_tests: covers_untested(&child.name, &child.path, untested),
                role_badge,
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
                badged: badged || role_badge.is_some(),
                outline,
                out,
            });
        }
    }
    for index in &dir.files {
        if let Some(entry) = model.files.get(*index) {
            let outlined = outline && !entry.symbols.is_empty();
            out.push(file_nav_row(entry, depth, false, badged, outlined));
            if outlined {
                push_outline(entry, depth.saturating_add(1), out);
            }
        }
    }
}

fn flat_rows(model: &ReviewModel, visible: &[usize], outline: bool) -> Vec<NavRow> {
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
    let mut out = Vec::new();
    for entry in indices.iter().filter_map(|index| model.files.get(*index)) {
        let outlined = outline && !entry.symbols.is_empty();
        out.push(file_nav_row(entry, 0, true, false, outlined));
        if outlined {
            push_outline(entry, 1, &mut out);
        }
    }
    out
}

// -- outline rows ------------------------------------------------------------

/// Every changed symbol of one file, each followed by what changed in it.
/// Source order, the order the file itself reads in.
fn push_outline(entry: &FileEntry, depth: usize, out: &mut Vec<NavRow>) {
    for symbol in &entry.symbols {
        let target = AttentionTarget::Symbol {
            file: entry.key.clone(),
            change_index: symbol.change_index,
        };
        out.push(NavRow {
            id: Some(NavRowId::Item(target.clone())),
            depth,
            kind: NavRowKind::Symbol(symbol_row(symbol, target.clone())),
        });
        for detail in detail_rows(symbol, &target) {
            out.push(NavRow {
                id: None,
                // One level under the symbol, so the guide of the block runs
                // down the symbol's own glyph column.
                depth: depth.saturating_add(1),
                kind: NavRowKind::Detail(detail),
            });
        }
    }
}

fn symbol_row(symbol: &SymbolEntry, target: AttentionTarget) -> SymbolRow {
    let candidates = symbol.reasons.iter().filter_map(|reason| {
        words::symbol_marker(reason.kind, &reason.label).map(|label| {
            (
                words::marker_rank(reason.kind),
                Reason {
                    kind: reason.kind,
                    label,
                },
            )
        })
    });
    SymbolRow {
        name: symbol.name.clone(),
        glyph: symbol.glyph,
        added: u64::from(symbol.lines_added),
        deleted: u64::from(symbol.lines_deleted),
        markers: top_markers(candidates),
        tooltip: symbol.qualified.clone(),
        target,
    }
}

/// The signature change first — it is the contract — then the calls.
fn detail_rows(symbol: &SymbolEntry, target: &AttentionTarget) -> Vec<DetailRow> {
    let mut lines: Vec<(DetailKind, String)> = Vec::new();
    if let Some((old, new)) = symbol.signature.as_ref() {
        lines.push((
            DetailKind::Signature,
            calls::signature_pair(old, new, CALL_TEXT_CHARS),
        ));
    }
    let calls = calls::call_lines(&symbol.calls, MAX_OUTLINE_CALLS);
    for line in &calls.shown {
        lines.push((DetailKind::Call(line.change), call_line_text(line)));
    }
    if let Some(note) = calls.hidden_note() {
        lines.push((DetailKind::More, note));
    }
    lines
        .into_iter()
        .enumerate()
        .map(|(position, (kind, text))| DetailRow {
            kind,
            text,
            target: target.clone(),
            position,
        })
        .collect()
}

/// `cx.notify()  in error branch` — one line, so the column truncates the
/// branch before the callee.
fn call_line_text(line: &calls::CallLine) -> String {
    let text = line.text_with_count();
    match line.context.as_deref() {
        Some(context) => format!("{text}  {context}"),
        None => text,
    }
}

// -- file rows ---------------------------------------------------------------

fn file_nav_row(
    entry: &FileEntry,
    depth: usize,
    flatten: bool,
    badged: bool,
    outlined: bool,
) -> NavRow {
    let is_rename = entry.status == ReviewFileStatus::Renamed
        || matches!((&entry.old_path, &entry.new_path), (Some(old), Some(new)) if old != new);
    let name_display = match (&entry.old_path, &entry.new_path) {
        (Some(old), Some(new)) if is_rename => words::rename_display(old, new),
        _ if flatten => tree_path(entry).to_string(),
        _ => basename(tree_path(entry)).to_string(),
    };
    let unreached = entry
        .reasons
        .iter()
        .find(|reason| reason.kind == ReasonKind::NotAnalyzed);
    let tooltip = match unreached {
        Some(reason) => format!("{} \u{00B7} {}", entry.display_path, reason.label),
        None => entry.display_path.clone(),
    };
    NavRow {
        id: Some(NavRowId::File(entry.key.clone())),
        depth,
        kind: NavRowKind::File(FileRow {
            name_display,
            icon_name: basename(tree_path(entry)).to_string(),
            added: entry.lines_added,
            deleted: entry.lines_deleted,
            markers: markers(entry),
            role_badge: role_badge(entry.role).filter(|_| !badged && !markers_name_the_role(entry)),
            dimmed: not_analyzed(entry),
            outlined,
            is_rename,
            tooltip,
        }),
    }
}

/// A `lockfile` / `CI config` / `submodule` chip already says what the role
/// badge would say; showing both reads as two facts.
fn markers_name_the_role(entry: &FileEntry) -> bool {
    markers(entry).iter().any(|reason| {
        matches!(
            reason.kind,
            ReasonKind::Lockfile | ReasonKind::CiConfig | ReasonKind::Submodule
        )
    })
}

/// Implementation is the default reading; Unclassified has nothing to say.
fn role_badge(role: FileRole) -> Option<&'static str> {
    match role {
        FileRole::Implementation | FileRole::Unclassified => None,
        other => Some(role_short(other)),
    }
}

/// The loudest two reasons of the file and of the symbols inside it — spec §7.
///
/// A new file's symbols are all new, so their reasons add nothing to `new`;
/// only the file's own reasons mark it.
fn markers(entry: &FileEntry) -> Vec<Reason> {
    let is_new = entry
        .reasons
        .iter()
        .any(|reason| reason.kind == ReasonKind::New);
    let symbol_reasons = entry
        .symbols
        .iter()
        .filter(|_| !is_new)
        .flat_map(|symbol| symbol.reasons.iter());
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
    let candidates = entry
        .reasons
        .iter()
        .chain(symbol_reasons)
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
        });
    top_markers(candidates)
}

/// The loudest [`MAX_MARKERS`] distinct markers of a row.
fn top_markers(candidates: impl Iterator<Item = (u8, Reason)>) -> Vec<Reason> {
    let mut candidates: Vec<(u8, Reason)> = candidates.collect();
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
    use super::super::super::ranking::{ModelInputs, StructureLoad, build_review_model};
    use super::super::super::state::{NavRowId, RoleFilter, RolePreset};
    use super::{
        AttentionTarget, CallChangeKind, DetailKind, DetailRow, DirRow, FileRow, NavRow,
        NavRowKind, SymbolRow, TreeArgs, covers_untested, default_expanded, nav_rows, not_analyzed,
        not_analyzed_count, row_ids, visible_files,
    };
    use std::collections::HashSet;

    fn args<'a>(
        filter: &'a RoleFilter,
        text: &'a str,
        expanded: &'a HashSet<String>,
        flatten: bool,
    ) -> TreeArgs<'a> {
        TreeArgs {
            role_filter: filter,
            filter_text: text,
            expanded_dirs: expanded,
            flatten,
            expanded_initialized: false,
            outline: false,
        }
    }

    fn tree(
        filter: &RoleFilter,
        text: &str,
        expanded: &HashSet<String>,
        flatten: bool,
    ) -> Vec<NavRow> {
        let model = fixtures::model();
        nav_rows(&model, &args(filter, text, expanded, flatten))
    }

    /// The same tree with every file's changed symbols inlined.
    fn outlined(flatten: bool) -> Vec<NavRow> {
        let model = fixtures::model();
        let filter = RoleFilter::everything();
        let expanded = HashSet::new();
        nav_rows(
            &model,
            &TreeArgs {
                outline: true,
                ..args(&filter, "", &expanded, flatten)
            },
        )
    }

    fn dir<'a>(rows: &'a [NavRow], path: &str) -> &'a DirRow {
        rows.iter()
            .find_map(|row| match (&row.id, &row.kind) {
                (Some(NavRowId::Dir(candidate)), NavRowKind::Dir(dir)) if candidate == path => {
                    Some(dir)
                }
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
                NavRowKind::Symbol(symbol) => symbol.name.clone(),
                NavRowKind::Detail(detail) => detail.text.clone(),
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
        // Its `CI config` chip already names the role; no second badge.
        assert_eq!(file(&rows, "Cargo.toml").role_badge, None);
        assert!(
            file(&rows, "Cargo.toml")
                .markers
                .iter()
                .any(|reason| reason.kind == ReasonKind::CiConfig)
        );
        assert_eq!(file(&rows, "handler_test.rs").role_badge, Some("Tests"));
    }

    #[test]
    fn a_directory_of_one_role_carries_the_badge_for_its_files() {
        let rows = tree(&RoleFilter::everything(), "", &HashSet::new(), false);
        assert_eq!(dir(&rows, "tests").role_badge, Some("Tests"));
        // The tree lists `tests/lib.rs` right after its directory row.
        let tests_row = rows
            .iter()
            .position(|row| row.id == Some(NavRowId::Dir("tests".into())))
            .unwrap();
        let NavRowKind::File(inner) = &rows[tests_row + 1].kind else {
            panic!("expected the file under tests/");
        };
        assert_eq!(inner.name_display, "lib.rs");
        assert_eq!(inner.role_badge, None);
        // Mixed directories badge nothing; their files keep their own.
        assert_eq!(dir(&rows, "src").role_badge, None);
        assert_eq!(dir(&rows, "worker").role_badge, None);
        // Flattened, there is no directory to carry it.
        let flat = tree(&RoleFilter::everything(), "", &HashSet::new(), true);
        assert_eq!(file(&flat, "tests/lib.rs").role_badge, Some("Tests"));
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
            &TreeArgs {
                expanded_initialized: true,
                ..args(&RoleFilter::everything(), "", &expanded, false)
            },
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

    #[test]
    fn every_tree_row_is_reachable_from_the_cursor() {
        let model = fixtures::model();
        let filter = RoleFilter::everything();
        let expanded = HashSet::new();
        for flatten in [false, true] {
            let rows = nav_rows(&model, &args(&filter, "", &expanded, flatten));
            assert!(!rows.is_empty());
            assert_eq!(
                row_ids(&rows),
                rows.iter()
                    .filter_map(|row| row.id.clone())
                    .collect::<Vec<_>>(),
                "the Files tree has no separator rows, so no row is skipped"
            );
        }
    }

    /// The symbol rows the outline emits directly under one file row.
    fn symbols_under<'a>(rows: &'a [NavRow], file: &str) -> Vec<&'a SymbolRow> {
        let start = rows
            .iter()
            .position(
                |row| matches!(&row.kind, NavRowKind::File(entry) if entry.name_display == file),
            )
            .unwrap_or_else(|| panic!("no file row named {file}"));
        let depth = rows[start].depth;
        rows[start.saturating_add(1)..]
            .iter()
            .take_while(|row| row.depth > depth)
            .filter_map(|row| match &row.kind {
                NavRowKind::Symbol(symbol) => Some(symbol),
                _ => None,
            })
            .collect()
    }

    /// The detail lines that follow one symbol row.
    fn details_of<'a>(rows: &'a [NavRow], symbol: &str) -> Vec<&'a DetailRow> {
        let start = rows
            .iter()
            .position(|row| matches!(&row.kind, NavRowKind::Symbol(entry) if entry.name == symbol))
            .unwrap_or_else(|| panic!("no symbol row named {symbol}"));
        rows[start.saturating_add(1)..]
            .iter()
            .map_while(|row| match &row.kind {
                NavRowKind::Detail(detail) => Some(detail),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_outline_inlines_every_changed_symbol_under_its_file() {
        let plain = tree(&RoleFilter::everything(), "", &HashSet::new(), false);
        assert!(
            !plain
                .iter()
                .any(|row| matches!(row.kind, NavRowKind::Symbol(_))),
            "the outline is off by default"
        );

        let model = fixtures::model();
        let entry = model
            .files
            .iter()
            .find(|entry| entry.display_path == "src/engine.rs")
            .expect("the fixture analyses src/engine.rs");
        let expected: Vec<&str> = entry
            .symbols
            .iter()
            .map(|symbol| symbol.name.as_str())
            .collect();

        for flatten in [false, true] {
            let rows = outlined(flatten);
            let name = if flatten {
                "src/engine.rs"
            } else {
                "engine.rs"
            };
            let names: Vec<&str> = symbols_under(&rows, name)
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect();
            assert_eq!(names, expected, "source order, the order the file reads in");
        }
    }

    #[test]
    fn a_symbol_row_carries_its_churn_and_only_the_markers_the_lines_lack() {
        let rows = outlined(false);
        let run = symbols_under(&rows, "engine.rs")
            .into_iter()
            .find(|symbol| symbol.name == "run")
            .expect("Engine::run changed");
        assert_eq!(run.tooltip, "Engine::run");
        assert!(run.added > 0 || run.deleted > 0);
        let labels: Vec<&str> = run
            .markers
            .iter()
            .map(|marker| marker.label.as_str())
            .collect();
        assert!(labels.contains(&"public"), "{labels:?}");
        assert!(
            !labels.iter().any(|label| label.contains("call")),
            "the call lines below say it: {labels:?}"
        );
    }

    #[test]
    fn detail_lines_state_the_signature_change_and_then_the_calls() {
        let rows = outlined(false);
        let details = details_of(&rows, "run");
        assert_eq!(
            details.iter().map(|detail| detail.kind).collect::<Vec<_>>(),
            vec![
                DetailKind::Signature,
                DetailKind::Call(CallChangeKind::Removed),
                DetailKind::Call(CallChangeKind::Modified),
            ],
            "the contract first, then what changed behind it"
        );
        assert!(
            details[0].text.contains('\u{2192}'),
            "the signature reads old → new: {}",
            details[0].text
        );
        assert!(
            details[1].text.starts_with("validate(input)"),
            "{}",
            details[1].text
        );
        assert!(
            details[1].text.ends_with("in error branch"),
            "the branch comes last, so a narrow column cuts it first: {}",
            details[1].text
        );
    }

    #[test]
    fn the_cursor_walks_the_symbols_and_steps_over_their_detail_lines() {
        let rows = outlined(false);
        assert!(
            rows.iter()
                .any(|row| matches!(row.kind, NavRowKind::Detail(_)))
        );
        for row in &rows {
            let walkable = !matches!(row.kind, NavRowKind::Detail(_));
            assert_eq!(row.id.is_some(), walkable);
        }
        let ids = row_ids(&rows);
        assert!(
            ids.iter()
                .any(|id| matches!(id, NavRowId::Item(AttentionTarget::Symbol { .. }))),
            "a symbol row opens the symbol"
        );
    }

    #[test]
    fn a_joined_chain_carries_the_flag_of_every_directory_it_swallowed() {
        let untested: HashSet<String> = HashSet::from(["packages/workers".to_string()]);
        assert!(
            covers_untested("workers/src", "packages/workers/src", &untested),
            "the joined row stands for packages/workers too"
        );
        assert!(covers_untested(
            "packages/workers",
            "packages/workers",
            &untested
        ));
        assert!(
            !covers_untested("src", "packages/workers/src", &untested),
            "an unjoined child never borrows its parent's flag"
        );
        assert!(!covers_untested(
            "packages/core",
            "packages/core",
            &untested
        ));
    }

    #[test]
    fn the_no_tests_flag_reads_the_comparison_not_the_filter() {
        // Hiding the test files must not turn every directory into an untested one.
        let review_code = tree(
            &RoleFilter::preset(RolePreset::ReviewCode),
            "",
            &HashSet::new(),
            false,
        );
        assert!(dir(&review_code, "src").no_tests);
        assert!(
            !dir(&review_code, "worker").no_tests,
            "worker/ changed a test next to its implementation, filtered or not"
        );
    }

    #[test]
    fn not_analyzed_means_structure_landed_and_missed_the_file() {
        let model = fixtures::model();
        for entry in &model.files {
            assert_eq!(
                not_analyzed(entry),
                !entry.analysis.is_analyzed(),
                "{} disagrees with the saved filter",
                entry.display_path
            );
        }
        let mut narrow = RoleFilter::everything();
        narrow.not_analyzed_only = true;
        assert_eq!(
            not_analyzed_count(&model),
            model
                .files
                .iter()
                .filter(|entry| narrow.allows(entry))
                .count()
        );
        assert!(not_analyzed_count(&model) > 0);
    }

    #[test]
    fn nothing_is_dim_while_structure_is_still_loading() {
        let inventory = fixtures::inventory();
        let loading = build_review_model(ModelInputs {
            inventory: Some(&inventory),
            inventory_error: None,
            structure: None,
            structure_state: StructureLoad::Loading,
            diff_mode: &okena_git::DiffMode::BranchCompare {
                base: "main".into(),
                head: "feature".into(),
            },
        });
        assert_eq!(not_analyzed_count(&loading), 0);
        let rows = nav_rows(
            &loading,
            &args(&RoleFilter::everything(), "", &HashSet::new(), true),
        );
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| match &row.kind {
            NavRowKind::File(file) => !file.dimmed,
            _ => true,
        }));
    }
}
