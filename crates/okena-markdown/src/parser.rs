//! Markdown parsing logic.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::MarkdownDocument;
use super::types::{FmValue, Frontmatter, Inline, Node};

/// Append `text` to the innermost inline run, merging it into the preceding text
/// rather than starting a new one. The renderer lays each run out as its own
/// inline flex item, so an unmerged run per source line would make a paragraph
/// break wherever the author happened to wrap the file, not where the column
/// ends.
fn push_text(stack: &mut [Vec<Inline>], text: &str) {
    let Some(current) = stack.last_mut() else {
        return;
    };
    match current.last_mut() {
        Some(Inline::Text(prev)) => prev.push_str(text),
        _ => current.push(Inline::Text(text.to_string())),
    }
}

impl MarkdownDocument {
    /// Parse markdown content into a document.
    pub fn parse(content: &str) -> Self {
        let mut nodes = Vec::new();

        // Peel off a leading YAML frontmatter block before handing the rest to
        // pulldown-cmark, which would otherwise turn `key: value` lines plus the
        // closing `---` into a setext heading (everything mashed onto one line).
        let markdown = match split_frontmatter(content) {
            Some((inner, rest)) => {
                if let Some(node) = parse_frontmatter(inner) {
                    nodes.push(node);
                }
                rest
            }
            None => content,
        };

        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        let parser = Parser::new_ext(markdown, options);

        let mut inline_stack: Vec<Vec<Inline>> = vec![Vec::new()];
        // URLs of the links currently open, parallel to their `inline_stack`
        // frames. Kept beside the tree rather than as a sentinel inside it, so
        // the label text is free to merge with its neighbours.
        let mut link_urls: Vec<String> = Vec::new();

        // State
        let mut in_heading: Option<u8> = None;
        let mut in_paragraph = false;
        let mut in_code_block = false;
        let mut code_block_lang: Option<String> = None;
        let mut code_block_content = String::new();
        let mut in_list = false;
        let mut list_ordered = false;
        let mut list_items: Vec<Vec<Inline>> = Vec::new();
        let mut in_blockquote = false;
        let mut in_table = false;
        let mut in_table_head = false;
        let mut table_headers: Vec<Vec<Inline>> = Vec::new();
        let mut table_rows: Vec<Vec<Vec<Inline>>> = Vec::new();
        let mut current_row: Vec<Vec<Inline>> = Vec::new();

        for event in parser {
            match event {
                // Block elements
                Event::Start(Tag::Heading { level, .. }) => {
                    in_heading = Some(match level {
                        HeadingLevel::H1 => 1,
                        HeadingLevel::H2 => 2,
                        HeadingLevel::H3 => 3,
                        HeadingLevel::H4 => 4,
                        HeadingLevel::H5 => 5,
                        HeadingLevel::H6 => 6,
                    });
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Heading(_)) => {
                    if let Some(level) = in_heading.take() {
                        let children = inline_stack.pop().unwrap_or_default();
                        nodes.push(Node::Heading { level, children });
                    }
                }
                Event::Start(Tag::Paragraph) => {
                    in_paragraph = true;
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Paragraph) if in_paragraph => {
                    let children = inline_stack.pop().unwrap_or_default();
                    if in_blockquote {
                        // Add to blockquote
                        if let Some(last) = inline_stack.last_mut() {
                            last.extend(children);
                        }
                    } else if in_list {
                        // Will be collected by Item end
                        if let Some(last) = inline_stack.last_mut() {
                            last.extend(children);
                        }
                    } else if in_table {
                        // Table cell content
                        if let Some(last) = inline_stack.last_mut() {
                            last.extend(children);
                        }
                    } else {
                        nodes.push(Node::Paragraph { children });
                    }
                    in_paragraph = false;
                }
                Event::Start(Tag::CodeBlock(kind)) => {
                    in_code_block = true;
                    code_block_lang = match kind {
                        CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.to_string()),
                        _ => None,
                    };
                    code_block_content.clear();
                }
                Event::End(TagEnd::CodeBlock) => {
                    nodes.push(Node::CodeBlock {
                        language: code_block_lang.take(),
                        code: std::mem::take(&mut code_block_content),
                    });
                    in_code_block = false;
                }
                Event::Start(Tag::List(first_item)) => {
                    in_list = true;
                    list_ordered = first_item.is_some();
                    list_items.clear();
                }
                Event::End(TagEnd::List(_)) => {
                    nodes.push(Node::List {
                        ordered: list_ordered,
                        items: std::mem::take(&mut list_items),
                    });
                    in_list = false;
                }
                Event::Start(Tag::Item) => {
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Item) => {
                    let children = inline_stack.pop().unwrap_or_default();
                    list_items.push(children);
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    in_blockquote = true;
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    let children = inline_stack.pop().unwrap_or_default();
                    nodes.push(Node::Blockquote { children });
                    in_blockquote = false;
                }
                Event::Rule => {
                    nodes.push(Node::HorizontalRule);
                }

                // Table elements
                Event::Start(Tag::Table(_)) => {
                    in_table = true;
                    table_headers.clear();
                    table_rows.clear();
                }
                Event::End(TagEnd::Table) => {
                    let headers = std::mem::take(&mut table_headers);
                    let rows = std::mem::take(&mut table_rows);
                    let col_widths = Self::table_col_widths(&headers, &rows);
                    nodes.push(Node::Table {
                        headers,
                        rows,
                        col_widths,
                    });
                    in_table = false;
                }
                Event::Start(Tag::TableHead) => {
                    in_table_head = true;
                    current_row.clear();
                }
                Event::End(TagEnd::TableHead) => {
                    table_headers = std::mem::take(&mut current_row);
                    in_table_head = false;
                }
                Event::Start(Tag::TableRow) => {
                    current_row.clear();
                }
                Event::End(TagEnd::TableRow) if !in_table_head => {
                    table_rows.push(std::mem::take(&mut current_row));
                }
                Event::Start(Tag::TableCell) => {
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::TableCell) => {
                    let children = inline_stack.pop().unwrap_or_default();
                    current_row.push(children);
                }

                // Inline elements
                Event::Start(Tag::Strong) => {
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Strong) => {
                    let children = inline_stack.pop().unwrap_or_default();
                    if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Bold(children));
                    }
                }
                Event::Start(Tag::Emphasis) => {
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Emphasis) => {
                    let children = inline_stack.pop().unwrap_or_default();
                    if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Italic(children));
                    }
                }
                Event::Start(Tag::Link { dest_url, .. }) => {
                    inline_stack.push(Vec::new());
                    link_urls.push(dest_url.to_string());
                }
                Event::End(TagEnd::Link) => {
                    let children = inline_stack.pop().unwrap_or_default();
                    let url = link_urls.pop().unwrap_or_default();
                    if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Link {
                            _url: url,
                            children,
                        });
                    }
                }
                Event::Code(text) => {
                    if in_code_block {
                        code_block_content.push_str(&text);
                    } else if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Code(text.to_string()));
                    }
                }
                Event::Text(text) => {
                    if in_code_block {
                        code_block_content.push_str(&text);
                    } else {
                        push_text(&mut inline_stack, &text);
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if in_code_block {
                        code_block_content.push('\n');
                    } else {
                        push_text(&mut inline_stack, " ");
                    }
                }
                _ => {}
            }
        }

        // Build flat text representation
        let mut plain_text = String::new();

        for node in &nodes {
            Self::node_to_flat_text(node, &mut plain_text);
        }

        // Precompute each node's cumulative start offset (in characters) once,
        // so rendering does not re-walk node text lengths on every frame.
        let mut node_offsets = Vec::with_capacity(nodes.len());
        let mut offset = 0usize;
        for node in &nodes {
            node_offsets.push(offset);
            offset += Self::node_text_length(node);
        }

        Self {
            nodes,
            node_offsets,
            plain_text,
        }
    }

    /// Convert a node to flat text (in characters, not bytes).
    pub(crate) fn node_to_flat_text(node: &Node, text: &mut String) {
        match node {
            Node::Heading { children, .. }
            | Node::Paragraph { children }
            | Node::Blockquote { children } => {
                Self::inlines_to_flat_text(children, text);
                text.push('\n');
            }
            Node::CodeBlock { code, .. } => {
                for line in code.lines() {
                    text.push_str(line);
                    text.push('\n');
                }
            }
            Node::List { items, .. } => {
                for item in items {
                    Self::inlines_to_flat_text(item, text);
                    text.push('\n');
                }
            }
            Node::Table { headers, rows, .. } => {
                for (i, header) in headers.iter().enumerate() {
                    if i > 0 {
                        text.push('\t');
                    }
                    Self::inlines_to_flat_text(header, text);
                }
                text.push('\n');
                for row in rows {
                    for (i, cell) in row.iter().enumerate() {
                        if i > 0 {
                            text.push('\t');
                        }
                        Self::inlines_to_flat_text(cell, text);
                    }
                    text.push('\n');
                }
            }
            Node::HorizontalRule => {
                text.push('\n');
            }
            Node::Frontmatter { block, .. } => {
                text.push_str(&super::types::frontmatter_flat_text(block));
            }
        }
    }

    /// Compute per-column display widths (in characters) for a table: the max
    /// content length across the header and every row cell in that column.
    pub(crate) fn table_col_widths(
        headers: &[Vec<Inline>],
        rows: &[Vec<Vec<Inline>>],
    ) -> Vec<usize> {
        let mut col_widths: Vec<usize> = headers
            .iter()
            .map(|h| Self::inlines_text_length(h))
            .collect();
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                let len = Self::inlines_text_length(cell);
                if i < col_widths.len() {
                    col_widths[i] = col_widths[i].max(len);
                }
            }
        }
        col_widths
    }

    /// Convert inline elements to flat text.
    pub(crate) fn inlines_to_flat_text(inlines: &[Inline], text: &mut String) {
        for inline in inlines {
            match inline {
                Inline::Text(t) => text.push_str(t),
                Inline::Code(c) => text.push_str(c),
                Inline::Bold(children) | Inline::Italic(children) => {
                    Self::inlines_to_flat_text(children, text);
                }
                Inline::Link { children, .. } => {
                    Self::inlines_to_flat_text(children, text);
                }
            }
        }
    }
}

/// Detect a leading YAML frontmatter block delimited by `---` fences.
///
/// The opening `---` must be the very first line of the document. The block is
/// closed by a line that is exactly `---` or `...`. Returns `(inner, rest)`
/// where `inner` is the YAML between the fences and `rest` is the markdown that
/// follows the closing fence. Returns `None` when no well-formed block is found.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let first = content.split_inclusive('\n').next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    // A bare `---` with no following line is a horizontal rule, not frontmatter.
    let body = &content[first.len()..];
    if body.is_empty() {
        return None;
    }

    let mut offset = first.len();
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" || trimmed == "..." {
            let inner = &content[first.len()..offset];
            let rest = &content[offset + line.len()..];
            return Some((inner, rest));
        }
        offset += line.len();
    }
    None
}

/// Parse the inner YAML of a frontmatter block into a [`Node::Frontmatter`].
///
/// A well-formed mapping becomes an ordered key/value card; anything else
/// (invalid YAML, or a bare scalar/sequence) is preserved verbatim. An empty
/// block (`---\n---`, `{}`, or only-null) carries no metadata to show, so it
/// yields `None` and no node is emitted — no empty card.
fn parse_frontmatter(inner: &str) -> Option<Node> {
    let block = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(inner) {
        Ok(serde_yaml_ng::Value::Mapping(map)) => {
            let entries: Vec<(String, FmValue)> = map
                .into_iter()
                .map(|(k, v)| (super::types::yaml_key_to_string(&k), FmValue::from_yaml(v)))
                .collect();
            if entries.is_empty() {
                return None;
            }
            Frontmatter::Parsed(entries)
        }
        Ok(serde_yaml_ng::Value::Null) => return None,
        _ => Frontmatter::Raw(inner.trim_matches('\n').to_string()),
    };
    let text_len = super::types::char_len(&super::types::frontmatter_flat_text(&block));
    Some(Node::Frontmatter { block, text_len })
}

#[cfg(test)]
mod tests {
    use super::super::MarkdownDocument;

    /// Adjacent text is merged into one run so a paragraph wraps at the column
    /// rather than at the author's line breaks. A link label is a run of its
    /// own, and must survive that merging.
    #[test]
    fn link_label_and_soft_wrapped_text_survive_merging() {
        let doc = MarkdownDocument::parse("A [label](https://example.com) here.");
        assert_eq!(doc.plain_text, "A label here.\n");

        // A soft break inside a paragraph becomes a space, not a line break.
        let doc = MarkdownDocument::parse("first line\nsecond line");
        assert_eq!(doc.plain_text, "first line second line\n");
    }

    /// The precomputed `node_offsets` must match a running offset computed by
    /// walking `node_text_length` over the nodes (the previous behavior).
    #[test]
    fn precomputed_offsets_match_walk() {
        let content = "\
# Heading One

A paragraph with **bold** and `code`.

```rust
fn main() {}
let x = 1;
```

| A | B |
|---|---|
| 1 | 2 |
| 3 | 4 |

## Heading Two
";
        let doc = MarkdownDocument::parse(content);

        // Reconstruct offsets the old way.
        let mut expected = Vec::with_capacity(doc.nodes.len());
        let mut offset = 0usize;
        for node in &doc.nodes {
            expected.push(offset);
            offset += MarkdownDocument::node_text_length(node);
        }

        assert_eq!(doc.node_offsets, expected);
        assert_eq!(doc.node_offsets.len(), doc.nodes.len());
        // First node always starts at 0.
        assert_eq!(doc.node_offsets.first().copied(), Some(0));
    }

    use super::super::types::{FmValue, Frontmatter, Node};
    use super::split_frontmatter;

    #[test]
    fn detects_frontmatter_and_keeps_markdown() {
        let content = "\
---
title: Hello World
draft: true
---

# Body
";
        let doc = MarkdownDocument::parse(content);

        // First node is the frontmatter card, parsed in order.
        let Node::Frontmatter {
            block: Frontmatter::Parsed(entries),
            ..
        } = &doc.nodes[0]
        else {
            panic!("expected parsed frontmatter, got something else");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, "title");
        assert!(matches!(&entries[0].1, FmValue::Scalar(s) if s == "Hello World"));
        assert_eq!(entries[1].0, "draft");
        assert!(matches!(&entries[1].1, FmValue::Scalar(s) if s == "true"));

        // The heading after the closing fence still parses as a heading.
        assert!(
            doc.nodes[1..]
                .iter()
                .any(|n| matches!(n, Node::Heading { .. }))
        );
        // Offsets stay consistent with node lengths once frontmatter is included.
        let mut offset = 0usize;
        for (i, node) in doc.nodes.iter().enumerate() {
            assert_eq!(doc.node_offsets[i], offset);
            offset += MarkdownDocument::node_text_length(node);
        }
    }

    #[test]
    fn parses_lists_and_nested_maps() {
        let content = "\
---
tags:
  - rust
  - gpui
author:
  name: David
  role: dev
---
body
";
        let doc = MarkdownDocument::parse(content);
        let Node::Frontmatter {
            block: Frontmatter::Parsed(entries),
            ..
        } = &doc.nodes[0]
        else {
            panic!("expected parsed frontmatter");
        };
        assert!(matches!(&entries[0].1, FmValue::List(items) if items.len() == 2));
        assert!(matches!(&entries[1].1, FmValue::Map(sub) if sub.len() == 2));
    }

    #[test]
    fn invalid_yaml_falls_back_to_raw() {
        // A bare scalar between fences is valid YAML but not a mapping.
        let content = "---\njust some text\n---\n# Body\n";
        let doc = MarkdownDocument::parse(content);
        assert!(matches!(
            &doc.nodes[0],
            Node::Frontmatter {
                block: Frontmatter::Raw(_),
                ..
            }
        ));
    }

    #[test]
    fn bare_horizontal_rule_is_not_frontmatter() {
        // No closing fence -> not frontmatter; `---` stays a horizontal rule.
        assert_eq!(split_frontmatter("---\njust a hr\n\nmore text\n"), None);
        assert_eq!(split_frontmatter("---"), None);
        assert_eq!(split_frontmatter("# Heading\n---\n"), None);
    }

    #[test]
    fn closing_fence_with_dots() {
        let content = "---\nkey: value\n...\nbody\n";
        let doc = MarkdownDocument::parse(content);
        assert!(matches!(
            &doc.nodes[0],
            Node::Frontmatter {
                block: Frontmatter::Parsed(_),
                ..
            }
        ));
    }

    #[test]
    fn empty_frontmatter_emits_no_node() {
        // An empty block carries no metadata: no frontmatter node is emitted and
        // the following markdown still parses as usual (no empty card).
        for content in [
            "---\n---\n# Body\n",
            "---\n\n---\n# Body\n",
            "---\n{}\n---\n# Body\n",
        ] {
            let doc = MarkdownDocument::parse(content);
            assert!(
                !doc.nodes
                    .iter()
                    .any(|n| matches!(n, Node::Frontmatter { .. })),
                "expected no frontmatter node for {content:?}"
            );
            assert!(
                matches!(doc.nodes.first(), Some(Node::Heading { .. })),
                "expected heading first for {content:?}"
            );
        }
    }

    #[test]
    fn cached_frontmatter_len_matches_flat_text() {
        // The precomputed `text_len` must equal the actual flat-text length, or
        // node offsets drift out of sync with `plain_text` and copy breaks.
        let content = "\
---
title: Hello
tags:
  - a
  - b
author:
  name: David
---
# Body
";
        let doc = MarkdownDocument::parse(content);
        let Node::Frontmatter { block, text_len } = &doc.nodes[0] else {
            panic!("expected frontmatter");
        };
        assert_eq!(
            *text_len,
            super::super::types::char_len(&super::super::types::frontmatter_flat_text(block))
        );

        // Sum of node lengths must equal the flat-text length copy slices from.
        let total: usize = doc
            .nodes
            .iter()
            .map(MarkdownDocument::node_text_length)
            .sum();
        assert_eq!(total, doc.plain_text.chars().count());
    }
}
