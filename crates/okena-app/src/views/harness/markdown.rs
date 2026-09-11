//! Markdown as the harness views show it: knowledge entries, spec documents
//! and task descriptions all read through the same renderer, so a heading or a
//! code block looks the same wherever it turns up.

use crate::theme::theme;
use crate::ui::tokens::ui_text;
use gpui::prelude::*;
use gpui::*;
use gpui_component::v_flex;
use okena_markdown::{MarkdownDocument, RenderedNode};
use std::cell::RefCell;
use std::rc::Rc;

use super::HarnessPane;

/// An opened file, parsed once when it arrives rather than on every frame.
pub(crate) enum OpenDocument {
    Markdown(MarkdownDocument),
    Text(String),
}

impl OpenDocument {
    /// Markdown when `path` says so, the file as written otherwise.
    pub(crate) fn from_file(path: &str, content: String, is_dark: bool) -> Self {
        let markdown = std::path::Path::new(path)
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("md"));
        if markdown {
            Self::Markdown(parse(&content, is_dark))
        } else {
            Self::Text(content)
        }
    }
}

fn parse(content: &str, is_dark: bool) -> MarkdownDocument {
    let mut doc = MarkdownDocument::parse(content);
    doc.highlight_code_blocks(is_dark);
    doc
}

/// The last Markdown text rendered from a `&self` view, parsed once and kept
/// until the text or the theme changes.
///
/// Task descriptions arrive inside the task list, not through a load of their
/// own, so there is no moment to parse them other than the frame that shows
/// them.
#[derive(Default)]
pub(crate) struct MarkdownCache {
    entry: RefCell<Option<CachedMarkdown>>,
}

struct CachedMarkdown {
    source: String,
    is_dark: bool,
    doc: Rc<MarkdownDocument>,
}

impl MarkdownCache {
    pub(crate) fn get(&self, source: &str, is_dark: bool) -> Rc<MarkdownDocument> {
        let mut entry = self.entry.borrow_mut();
        if let Some(cached) = entry.as_ref()
            && cached.is_dark == is_dark
            && cached.source == source
        {
            return cached.doc.clone();
        }
        let doc = Rc::new(parse(source, is_dark));
        *entry = Some(CachedMarkdown {
            source: source.to_string(),
            is_dark,
            doc: doc.clone(),
        });
        doc
    }
}

impl HarnessPane {
    /// Every block of a parsed Markdown document, spaced the way the file
    /// viewer spaces them.
    pub(super) fn render_markdown_blocks(
        &self,
        doc: &MarkdownDocument,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let t = theme(cx);
        (0..doc.node_count())
            .filter_map(|idx| {
                let (above, below) = doc.node_spacing(idx);
                let block: AnyElement = match doc.render_node(idx, &t, cx, None)? {
                    RenderedNode::Simple { div, .. } => div.into_any_element(),
                    RenderedNode::CodeBlock { lines, .. } => v_flex()
                        .w_full()
                        .px(px(12.0))
                        .py(px(8.0))
                        .rounded(px(4.0))
                        .bg(rgb(t.bg_secondary))
                        .children(lines.into_iter().map(|line| line.div))
                        .into_any_element(),
                    RenderedNode::Table { header, rows } => v_flex()
                        .w_full()
                        .children(header.into_iter().chain(rows).map(|row| row.div))
                        .into_any_element(),
                };
                Some(
                    div()
                        .w_full()
                        .pt(above)
                        .pb(below)
                        .child(block)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// An opened file's blocks: formatted Markdown, or its lines as written.
    ///
    /// Plain text goes line by line so long files wrap, and a single text
    /// node would collapse blank lines and lose the file's shape.
    pub(super) fn render_open_document(
        &self,
        document: &OpenDocument,
        id_prefix: &str,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        match document {
            OpenDocument::Markdown(doc) => self.render_markdown_blocks(doc, cx),
            OpenDocument::Text(content) => {
                let t = theme(cx);
                content
                    .lines()
                    .enumerate()
                    .map(|(i, line)| {
                        div()
                            .id(SharedString::from(format!("{id_prefix}-line-{i}")))
                            .w_full()
                            .min_h(px(16.0))
                            .text_size(ui_text(12.5, cx))
                            .text_color(rgb(t.text_primary))
                            .child(line.to_string())
                            .into_any_element()
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Not `use super::*`: the gpui glob would shadow `#[test]` with
    // `gpui::test`, which expands into itself forever.
    use super::{MarkdownCache, OpenDocument};
    use std::rc::Rc;

    #[test]
    fn only_markdown_files_are_parsed() {
        for path in ["openspec/changes/x/proposal.md", "docs/README.MD"] {
            assert!(matches!(
                OpenDocument::from_file(path, "# Hi".into(), true),
                OpenDocument::Markdown(_)
            ));
        }
        for path in ["skills/release/run.sh", "openspec/config.yaml", "Makefile"] {
            assert!(matches!(
                OpenDocument::from_file(path, "# Hi".into(), true),
                OpenDocument::Text(_)
            ));
        }
    }

    #[test]
    fn the_cache_reparses_only_when_the_text_or_theme_changes() {
        let cache = MarkdownCache::default();
        let first = cache.get("# One", true);
        assert!(Rc::ptr_eq(&first, &cache.get("# One", true)));
        let other = cache.get("# Two", true);
        assert!(!Rc::ptr_eq(&first, &other));
        assert!(!Rc::ptr_eq(&other, &cache.get("# Two", false)));
    }
}
