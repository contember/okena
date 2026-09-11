//! The entries inside one knowledge root.
//!
//! - `docs/`, `agents/` and `templates/`: every `.md` file at any depth;
//! - `skills/`: every directory below `skills/` holding a `SKILL.md`, with the
//!   rest of that directory as the skill's supporting files;
//! - dot-entries are skipped, symlinked directories are never followed, and a
//!   symlinked file counts only when it resolves inside the root.
//!
//! Only the head of each file is read: listing a store must stay cheap however
//! large its documents are.

use crate::frontmatter;
use crate::{canonical, display};
use okena_core::knowledge::{
    Diagnostic, KnowledgeCounts, KnowledgeEntry, KnowledgeKind, KnowledgeTree,
};
use std::collections::BTreeSet;
use std::fs::FileType;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Most entries one tree lists. A root past this is almost certainly the wrong
/// directory, and the listing would be unusable anyway.
pub const MAX_ENTRIES: usize = 5_000;
/// Most supporting files listed for one skill.
const MAX_SKILL_FILES: usize = 200;
/// How much of each file is read for frontmatter, heading and placeholders.
const HEAD_BYTES: u64 = 256 * 1024;
const SKILL_FILE: &str = "SKILL.md";

/// Read every entry in `root`. The caller fills in the root key and store id.
pub fn read_tree(root: &Path) -> KnowledgeTree {
    walk(root, true)
}

/// How many entries of each kind `root` holds, without opening any file — what
/// discovery shows for every root on every refresh.
pub fn count_entries(root: &Path) -> KnowledgeCounts {
    count(&walk(root, false))
}

/// Whether `root` has any kind folder at all.
pub fn has_kind_folder(root: &Path) -> bool {
    KnowledgeKind::all()
        .into_iter()
        .any(|kind| kind_dir(root, kind).is_some())
}

pub fn count(tree: &KnowledgeTree) -> KnowledgeCounts {
    let mut counts = KnowledgeCounts::default();
    for entry in &tree.entries {
        let slot = match entry.kind {
            KnowledgeKind::Doc => &mut counts.docs,
            KnowledgeKind::Skill => &mut counts.skills,
            KnowledgeKind::Agent => &mut counts.agents,
            KnowledgeKind::Template => &mut counts.templates,
        };
        *slot += 1;
    }
    counts
}

/// Path relative to `root`, in the forward-slash form the wire uses.
pub fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolve a client-supplied relative path inside `root`.
///
/// Checked on the canonical path, so neither `..` nor a symlink can escape:
/// `docs/../../.ssh/id_rsa` is an ordinary-looking string.
pub fn resolve_document(root: &Path, path: &str) -> Result<PathBuf, String> {
    let real = root
        .join(path)
        .canonicalize()
        .map_err(|_| format!("no such document: {path}"))?;
    let real_root = root
        .canonicalize()
        .map_err(|e| format!("knowledge root is unreadable: {e}"))?;
    if !real.starts_with(&real_root) {
        return Err("path is outside the knowledge root".into());
    }
    if !real.is_file() {
        return Err(format!("not a file: {path}"));
    }
    Ok(real)
}

fn walk(root: &Path, read: bool) -> KnowledgeTree {
    let mut walk = Walk {
        root,
        real_root: canonical(root),
        read,
        entries: Vec::new(),
        truncated: false,
    };
    for kind in KnowledgeKind::all() {
        let Some(dir) = kind_dir(root, kind) else {
            continue;
        };
        match kind {
            KnowledgeKind::Skill => walk.skills(&dir, &dir),
            _ => walk.markdown(kind, &dir, &dir),
        }
    }
    let mut status = Vec::new();
    if walk.truncated {
        status.push(
            Diagnostic::warning(
                "entry_limit",
                format!("Only the first {MAX_ENTRIES} entries are listed."),
            )
            .with_fix(
                "Check that this is the right folder; a knowledge root this large is unusual.",
            ),
        );
    }
    let mut entries = walk.entries;
    entries.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.path.cmp(&b.path)));
    KnowledgeTree {
        root: display(root),
        entries,
        status,
        ..Default::default()
    }
}

/// A root's folder for `kind`, when it is a real directory — a symlinked kind
/// folder would let a root list files from anywhere.
fn kind_dir(root: &Path, kind: KnowledgeKind) -> Option<PathBuf> {
    let dir = root.join(kind.folder());
    std::fs::symlink_metadata(&dir)
        .is_ok_and(|m| m.is_dir())
        .then_some(dir)
}

struct Walk<'a> {
    root: &'a Path,
    real_root: PathBuf,
    /// Open files for frontmatter, titles and skill files; off when counting.
    read: bool,
    entries: Vec<KnowledgeEntry>,
    truncated: bool,
}

impl Walk<'_> {
    fn push(&mut self, entry: KnowledgeEntry) {
        if self.entries.len() >= MAX_ENTRIES {
            self.truncated = true;
        } else {
            self.entries.push(entry);
        }
    }

    fn entry(&self, kind: KnowledgeKind, path: &Path, name: String) -> KnowledgeEntry {
        if self.read {
            read_entry(self.root, kind, path, name)
        } else {
            bare_entry(self.root, kind, path, name)
        }
    }

    /// Markdown files at any depth under `dir`.
    fn markdown(&mut self, kind: KnowledgeKind, kind_dir: &Path, dir: &Path) {
        for (path, file_type) in children(dir) {
            if self.truncated {
                return;
            }
            if file_type.is_dir() {
                self.markdown(kind, kind_dir, &path);
            } else if is_markdown(&path) && self.counts_as_file(&path, file_type) {
                let entry = self.entry(kind, &path, rel(kind_dir, &path.with_extension("")));
                self.push(entry);
            }
        }
    }

    /// Skill directories under `dir`. A skill's own subdirectories are its
    /// files, not further skills.
    fn skills(&mut self, skills_dir: &Path, dir: &Path) {
        for (path, file_type) in children(dir) {
            if self.truncated {
                return;
            }
            if !file_type.is_dir() {
                continue;
            }
            let skill_md = path.join(SKILL_FILE);
            let is_skill = std::fs::symlink_metadata(&skill_md)
                .is_ok_and(|m| self.counts_as_file(&skill_md, m.file_type()));
            if is_skill {
                let mut entry = self.entry(KnowledgeKind::Skill, &skill_md, rel(skills_dir, &path));
                if self.read {
                    let mut files = Vec::new();
                    self.skill_files(&path, &path, &mut files);
                    files.sort();
                    entry.files = files;
                }
                self.push(entry);
            } else {
                self.skills(skills_dir, &path);
            }
        }
    }

    fn skill_files(&self, skill_dir: &Path, dir: &Path, out: &mut Vec<String>) {
        for (path, file_type) in children(dir) {
            if out.len() >= MAX_SKILL_FILES {
                return;
            }
            if file_type.is_dir() {
                self.skill_files(skill_dir, &path, out);
            } else if !(dir == skill_dir && path.file_name().is_some_and(|n| n == SKILL_FILE))
                && self.counts_as_file(&path, file_type)
            {
                out.push(rel(self.root, &path));
            }
        }
    }

    fn counts_as_file(&self, path: &Path, file_type: FileType) -> bool {
        file_type.is_file()
            || (file_type.is_symlink()
                && path
                    .canonicalize()
                    .is_ok_and(|real| real.is_file() && real.starts_with(&self.real_root)))
    }
}

/// Non-dot children of `dir`, by name. `FileType` comes from the entry itself,
/// which does not follow symlinks — that is what keeps linked directories from
/// being walked.
fn children(dir: &Path) -> Vec<(PathBuf, FileType)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, FileType)> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| Some((e.path(), e.file_type().ok()?)))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .is_some_and(|x| x.eq_ignore_ascii_case("md"))
}

fn bare_entry(root: &Path, kind: KnowledgeKind, path: &Path, name: String) -> KnowledgeEntry {
    KnowledgeEntry {
        kind,
        path: rel(root, path),
        title: name.clone(),
        name,
        description: None,
        tags: Vec::new(),
        files: Vec::new(),
        flows: Vec::new(),
        variables: Vec::new(),
        status: Vec::new(),
    }
}

fn read_entry(
    root: &Path,
    kind: KnowledgeKind,
    path: &Path,
    derived_name: String,
) -> KnowledgeEntry {
    let rel_path = rel(root, path);
    let mut status = Vec::new();
    let content = match read_head(path) {
        Ok(c) => c,
        Err(e) => {
            status.push(Diagnostic::warning(
                "entry_unreadable",
                format!("Could not read {rel_path}: {e}"),
            ));
            String::new()
        }
    };
    let (fm, body) = frontmatter::parse(&content);
    if let Some(error) = &fm.error {
        status.push(
            Diagnostic::warning(
                "frontmatter_invalid",
                format!("{rel_path} has frontmatter okena cannot read: {error}"),
            )
            .with_fix("Fix the YAML between the opening and closing `---`."),
        );
    }
    // Skills and agents are named by their formats' own `name` field.
    let name = match kind {
        KnowledgeKind::Skill | KnowledgeKind::Agent => fm.string("name"),
        KnowledgeKind::Doc | KnowledgeKind::Template => None,
    }
    .unwrap_or(derived_name);
    let title = fm
        .string("title")
        .or_else(|| first_heading(body))
        .unwrap_or_else(|| name.clone());
    let (flows, variables) = match kind {
        KnowledgeKind::Template => (fm.list("for"), placeholders(body)),
        _ => (Vec::new(), Vec::new()),
    };
    KnowledgeEntry {
        kind,
        path: rel_path,
        name,
        title,
        description: fm.string("description"),
        tags: fm.list("tags"),
        files: Vec::new(),
        flows,
        variables,
        status,
    }
}

fn read_head(path: &Path) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(HEAD_BYTES)
        .read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The first level-one ATX heading outside a fenced code block.
fn first_heading(body: &str) -> Option<String> {
    let mut fence: Option<&str> = None;
    for line in body.lines() {
        let t = line.trim_start();
        if let Some(open) = fence {
            if t.starts_with(open) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = ["```", "~~~"].into_iter().find(|f| t.starts_with(f)) {
            fence = Some(open);
            continue;
        }
        if let Some(rest) = t.strip_prefix("# ") {
            let heading = rest.trim().trim_end_matches('#').trim();
            if !heading.is_empty() {
                return Some(heading.to_string());
            }
        }
    }
    None
}

/// `{name}` placeholders in a template body, sorted and unique. A name is an
/// identifier, so JSON (`{"a": 1}`) and prose braces don't count.
fn placeholders(body: &str) -> Vec<String> {
    let mut found = BTreeSet::new();
    let mut rest = body;
    while let Some(open) = rest.find('{') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find(['}', '{']) else {
            break;
        };
        if rest.as_bytes()[close] == b'}' {
            let name = &rest[..close];
            let mut chars = name.chars();
            if chars
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                found.insert(name.to_string());
            }
        }
        rest = &rest[close..];
    }
    found.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::write;

    fn names(tree: &KnowledgeTree, kind: KnowledgeKind) -> Vec<&str> {
        tree.entries
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.name.as_str())
            .collect()
    }

    #[test]
    fn every_kind_is_found_in_its_folder_and_sorted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(&root.join("docs/ci/pipeline.md"), "# CI pipeline\n");
        write(&root.join("docs/principles.md"), "x");
        write(&root.join("docs/diagram.png"), "not markdown");
        write(
            &root.join("skills/release/SKILL.md"),
            "---\nname: release\ndescription: Cut a release\n---\n",
        );
        write(&root.join("skills/release/checklist.md"), "x");
        write(&root.join("skills/release/scripts/tag.sh"), "x");
        write(
            &root.join("skills/SKILL.md"),
            "a SKILL.md at the top is not a skill",
        );
        write(
            &root.join("agents/reviewer.md"),
            "---\nname: code-reviewer\n---\n",
        );
        write(
            &root.join("templates/task-start.md"),
            "Work on {key}: {title}",
        );
        write(&root.join("README.md"), "outside every kind folder");

        let tree = read_tree(root);
        assert_eq!(
            names(&tree, KnowledgeKind::Doc),
            ["ci/pipeline", "principles"]
        );
        assert_eq!(names(&tree, KnowledgeKind::Skill), ["release"]);
        assert_eq!(names(&tree, KnowledgeKind::Agent), ["code-reviewer"]);
        assert_eq!(names(&tree, KnowledgeKind::Template), ["task-start"]);
        assert_eq!(tree.entries.len(), 5);

        let skill = tree.entry("skills/release/SKILL.md").expect("skill");
        assert_eq!(skill.description.as_deref(), Some("Cut a release"));
        assert_eq!(
            skill.files,
            [
                "skills/release/checklist.md",
                "skills/release/scripts/tag.sh"
            ]
        );
        let counts = KnowledgeCounts {
            docs: 2,
            skills: 1,
            agents: 1,
            templates: 1,
        };
        assert_eq!(count(&tree), counts);
        // Counting without reading finds exactly what reading lists.
        assert_eq!(count_entries(root), counts);
        assert!(tree.status.is_empty());
    }

    #[test]
    fn titles_fall_back_from_frontmatter_to_heading_to_name() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(
            &root.join("docs/a.md"),
            "---\ntitle: From frontmatter\n---\n# From heading\n",
        );
        write(
            &root.join("docs/b.md"),
            "```\n# not a heading\n```\n## Level two\n# From heading\n",
        );
        write(&root.join("docs/c.md"), "no heading at all\n");
        let tree = read_tree(root);
        let title = |p| tree.entry(p).expect(p).title.as_str();
        assert_eq!(title("docs/a.md"), "From frontmatter");
        assert_eq!(title("docs/b.md"), "From heading");
        assert_eq!(title("docs/c.md"), "c");
    }

    #[test]
    fn docs_read_description_and_tags_and_templates_read_flows_and_variables() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write(
            &root.join("docs/ci.md"),
            "---\ndescription: How CI runs\ntags: ci, release\n---\n",
        );
        write(
            &root.join("templates/spec.md"),
            "---\nfor: [spec-draft, agent-session]\n---\nDraft {idea} in {change_dir}. JSON {\"a\": 1} and {idea} again, {{nested}}, { spaced }, {9lives}.",
        );
        let tree = read_tree(root);
        let doc = tree.entry("docs/ci.md").expect("doc");
        assert_eq!(doc.description.as_deref(), Some("How CI runs"));
        assert_eq!(doc.tags, ["ci", "release"]);
        assert!(doc.flows.is_empty() && doc.variables.is_empty());
        let template = tree.entry("templates/spec.md").expect("template");
        assert_eq!(template.flows, ["spec-draft", "agent-session"]);
        assert_eq!(template.variables, ["change_dir", "idea", "nested"]);
    }

    #[test]
    fn invalid_frontmatter_still_lists_the_entry_with_a_warning() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(
            &dir.path().join("agents/broken.md"),
            "---\nname: [oops\n---\n# Broken agent\n",
        );
        let tree = read_tree(dir.path());
        let entry = tree.entry("agents/broken.md").expect("listed");
        assert_eq!(entry.name, "broken");
        assert_eq!(entry.title, "Broken agent");
        assert_eq!(entry.status[0].code, "frontmatter_invalid");
    }

    #[cfg(unix)]
    #[test]
    fn dot_entries_and_symlinked_directories_are_skipped_and_escaping_links_do_not_count() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        let outside = dir.path().join("outside");
        write(&outside.join("linked/secret.md"), "x");
        write(&outside.join("secret.md"), "x");
        write(&root.join("docs/.drafts/wip.md"), "x");
        write(&root.join("docs/.hidden.md"), "x");
        write(&root.join("docs/real.md"), "x");
        symlink(outside.join("linked"), root.join("docs/linked-dir")).expect("symlink");
        symlink(outside.join("secret.md"), root.join("docs/escape.md")).expect("symlink");
        symlink(root.join("docs/real.md"), root.join("docs/alias.md")).expect("symlink");
        // A whole kind folder that is a link is not walked either.
        symlink(outside.join("linked"), root.join("agents")).expect("symlink");

        let tree = read_tree(&root);
        let paths: Vec<_> = tree.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["docs/alias.md", "docs/real.md"]);
    }

    #[test]
    fn the_entry_cap_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        for i in 0..=MAX_ENTRIES {
            write(&dir.path().join(format!("docs/{i:05}.md")), "");
        }
        let tree = read_tree(dir.path());
        assert_eq!(tree.entries.len(), MAX_ENTRIES);
        assert_eq!(tree.status[0].code, "entry_limit");
    }

    #[test]
    fn a_root_without_kind_folders_is_empty_and_says_so() {
        let dir = tempfile::tempdir().expect("tempdir");
        write(&dir.path().join("README.md"), "x");
        assert!(!has_kind_folder(dir.path()));
        assert!(read_tree(dir.path()).entries.is_empty());
        std::fs::create_dir_all(dir.path().join("templates")).expect("mkdir");
        assert!(has_kind_folder(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn documents_resolve_only_inside_the_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("root");
        write(&root.join("docs/a.md"), "x");
        write(&dir.path().join("secret.md"), "x");
        std::os::unix::fs::symlink(dir.path().join("secret.md"), root.join("docs/link.md"))
            .expect("symlink");

        assert!(resolve_document(&root, "docs/a.md").is_ok());
        assert!(resolve_document(&root, "docs/../../secret.md").is_err());
        assert!(
            resolve_document(&root, dir.path().join("secret.md").to_str().expect("utf8")).is_err()
        );
        assert!(resolve_document(&root, "docs/link.md").is_err());
        assert!(resolve_document(&root, "docs").is_err(), "a directory");
        assert!(resolve_document(&root, "docs/missing.md").is_err());
    }
}
