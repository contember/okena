//! Markdown parsing logic.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::MarkdownDocument;
use super::types::{FmValue, Frontmatter, Inline, ListItem, Node};

/// A block container that is currently open.
///
/// Lists nest (a list inside an item inside a list), so the parser keeps them
/// on a stack. The flat `in_list` / `list_items` state this replaced could only
/// describe one list at a time: a nested list cleared the outer list's items,
/// took its ordered-ness, and closed it early, which dropped every item after
/// the nesting point out of the list entirely.
enum Frame {
    List {
        ordered: bool,
        start: u64,
        items: Vec<ListItem>,
    },
    Item {
        blocks: Vec<Node>,
    },
    Blockquote {
        blocks: Vec<Node>,
    },
}

/// Route a finished block to the innermost open block container (a list item or
/// a quote), or to the document root when none is open.
fn push_block(nodes: &mut Vec<Node>, frames: &mut [Frame], node: Node) {
    match frames.last_mut() {
        Some(Frame::Item { blocks } | Frame::Blockquote { blocks }) => blocks.push(node),
        _ => nodes.push(node),
    }
}

/// Turn the inline text collected directly under the innermost item into a
/// paragraph block.
///
/// A *tight* list item (no blank line between items) carries its text as bare
/// inline events with no `Paragraph` around it, so the text has to be closed off
/// by hand: before any block opens inside the item, and when the item ends.
fn flush_item_inlines(inline_stack: &mut [Vec<Inline>], frames: &mut [Frame]) {
    let Some(Frame::Item { blocks }) = frames.last_mut() else {
        return;
    };
    let Some(pending) = inline_stack.last_mut() else {
        return;
    };
    if pending.is_empty() {
        return;
    }
    blocks.push(Node::Paragraph {
        children: std::mem::take(pending),
    });
}

/// Whether an event opens or closes a block inside a list item, and so has to
/// be preceded by [`flush_item_inlines`].
fn is_item_block_boundary(event: &Event) -> bool {
    matches!(
        event,
        Event::Start(
            Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::List(_)
                | Tag::BlockQuote(_)
                | Tag::Table(_)
        ) | Event::End(TagEnd::Item)
            | Event::Rule
    )
}

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
        // Open block containers (lists, items, quotes), innermost last.
        let mut frames: Vec<Frame> = Vec::new();
        let mut in_table = false;
        let mut in_table_head = false;
        let mut table_headers: Vec<Vec<Inline>> = Vec::new();
        let mut table_rows: Vec<Vec<Vec<Inline>>> = Vec::new();
        let mut current_row: Vec<Vec<Inline>> = Vec::new();

        for event in parser {
            if is_item_block_boundary(&event) {
                flush_item_inlines(&mut inline_stack, &mut frames);
            }
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
                        push_block(&mut nodes, &mut frames, Node::Heading { level, children });
                    }
                }
                Event::Start(Tag::Paragraph) => {
                    in_paragraph = true;
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Paragraph) if in_paragraph => {
                    let children = inline_stack.pop().unwrap_or_default();
                    if in_table {
                        // Collected by the table-cell end instead.
                        if let Some(last) = inline_stack.last_mut() {
                            last.extend(children);
                        }
                    } else {
                        push_block(&mut nodes, &mut frames, Node::Paragraph { children });
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
                    push_block(
                        &mut nodes,
                        &mut frames,
                        Node::CodeBlock {
                            language: code_block_lang.take(),
                            code: std::mem::take(&mut code_block_content),
                            highlighted: Vec::new(),
                        },
                    );
                    in_code_block = false;
                }
                Event::Start(Tag::List(first_item)) => {
                    frames.push(Frame::List {
                        ordered: first_item.is_some(),
                        start: first_item.unwrap_or(1),
                        items: Vec::new(),
                    });
                }
                Event::End(TagEnd::List(_)) => {
                    if let Some(Frame::List {
                        ordered,
                        start,
                        items,
                    }) = frames.pop()
                    {
                        push_block(
                            &mut nodes,
                            &mut frames,
                            Node::List {
                                ordered,
                                start,
                                items,
                            },
                        );
                    }
                }
                Event::Start(Tag::Item) => {
                    frames.push(Frame::Item { blocks: Vec::new() });
                    // Holds text written straight into the item (a tight list);
                    // `flush_item_inlines` turns it into a paragraph block.
                    inline_stack.push(Vec::new());
                }
                Event::End(TagEnd::Item) => {
                    // Emptied by the flush that ran for this event.
                    inline_stack.pop();
                    if let Some(Frame::Item { blocks }) = frames.pop()
                        && let Some(Frame::List { items, .. }) = frames.last_mut()
                    {
                        items.push(ListItem { blocks });
                    }
                }
                Event::Start(Tag::BlockQuote(_)) => {
                    frames.push(Frame::Blockquote { blocks: Vec::new() });
                }
                Event::End(TagEnd::BlockQuote(_)) => {
                    if let Some(Frame::Blockquote { blocks }) = frames.pop() {
                        push_block(&mut nodes, &mut frames, Node::Blockquote { blocks });
                    }
                }
                Event::Rule => {
                    push_block(&mut nodes, &mut frames, Node::HorizontalRule);
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
                    push_block(
                        &mut nodes,
                        &mut frames,
                        Node::Table {
                            headers,
                            rows,
                            col_widths,
                        },
                    );
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
            Node::Heading { children, .. } | Node::Paragraph { children } => {
                Self::inlines_to_flat_text(children, text);
                text.push('\n');
            }
            Node::Blockquote { blocks } => {
                for block in blocks {
                    Self::node_to_flat_text(block, text);
                }
            }
            Node::CodeBlock { code, .. } => {
                for line in code.lines() {
                    text.push_str(line);
                    text.push('\n');
                }
            }
            Node::List { items, .. } => {
                for item in items {
                    for block in &item.blocks {
                        Self::node_to_flat_text(block, text);
                    }
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

    use super::super::types::{FmValue, Frontmatter, ListItem, Node};
    use super::split_frontmatter;

    fn expect_list(node: &Node) -> (bool, u64, &[ListItem]) {
        match node {
            Node::List {
                ordered,
                start,
                items,
            } => (*ordered, *start, items),
            _ => panic!("expected a list"),
        }
    }

    fn item_text(item: &ListItem) -> String {
        let mut out = String::new();
        for block in &item.blocks {
            MarkdownDocument::node_to_flat_text(block, &mut out);
        }
        out
    }

    /// A nested list used to clobber the list around it: the inner `Start(List)`
    /// reset the single flat list state, so the outer list lost its items, took
    /// the inner list's bullet marker, and closed early. Every item after the
    /// nesting point fell out of the list and rendered as a bare paragraph.
    #[test]
    fn nested_list_keeps_the_list_around_it_intact() {
        let content = "\
1. First question.

2. Second, with sub-points:
   - changed since
   - paging

3. Third question.

4. Fourth question.
";
        let doc = MarkdownDocument::parse(content);

        // The whole thing is one top-level list: nothing leaked out of it.
        assert_eq!(doc.nodes.len(), 1, "expected a single top-level list");
        let (ordered, start, items) = expect_list(&doc.nodes[0]);
        assert!(ordered, "the outer list is numbered");
        assert_eq!(start, 1);
        assert_eq!(items.len(), 4);

        // Item 2 holds its own text plus the nested list, in that order.
        assert_eq!(items[1].blocks.len(), 2);
        assert!(matches!(items[1].blocks[0], Node::Paragraph { .. }));
        let (inner_ordered, _, inner_items) = expect_list(&items[1].blocks[1]);
        assert!(!inner_ordered, "the nested list is a bullet list");
        assert_eq!(inner_items.len(), 2);

        // The items after the nesting point are still items, with their text.
        assert_eq!(item_text(&items[2]), "Third question.\n");
        assert_eq!(item_text(&items[3]), "Fourth question.\n");
    }

    /// Tight items (no blank line between them) carry their text as bare inline
    /// events; each still ends up as one paragraph block inside its item.
    #[test]
    fn tight_and_multi_paragraph_items_become_blocks() {
        let doc = MarkdownDocument::parse("- one\n- two\n  - nested\n");
        let (_, _, items) = expect_list(&doc.nodes[0]);
        assert_eq!(items.len(), 2);
        assert_eq!(item_text(&items[0]), "one\n");
        assert_eq!(items[1].blocks.len(), 2, "text plus the nested list");

        let doc = MarkdownDocument::parse("- first para\n\n  second para\n");
        let (_, _, items) = expect_list(&doc.nodes[0]);
        assert_eq!(items[0].blocks.len(), 2);
        assert_eq!(item_text(&items[0]), "first para\nsecond para\n");
    }

    /// A fenced block indented under an item belongs to that item. It used to be
    /// hoisted to the document root and drawn after the list it sat inside.
    #[test]
    fn code_block_stays_inside_its_item() {
        let doc = MarkdownDocument::parse("1. Run it:\n\n   ```sh\n   cargo test\n   ```\n");
        assert_eq!(doc.nodes.len(), 1);
        let (_, _, items) = expect_list(&doc.nodes[0]);
        assert!(matches!(
            items[0].blocks.as_slice(),
            [Node::Paragraph { .. }, Node::CodeBlock { .. }]
        ));
    }

    /// A quote holds blocks, so its paragraphs stay separate instead of being
    /// merged into one inline run, and a quoted list stays inside the quote
    /// rather than being emitted after it.
    #[test]
    fn blockquote_keeps_its_blocks() {
        let doc = MarkdownDocument::parse("> first para\n>\n> second para\n");
        assert_eq!(doc.nodes.len(), 1);
        let Node::Blockquote { blocks } = &doc.nodes[0] else {
            panic!("expected a blockquote");
        };
        assert_eq!(blocks.len(), 2);
        assert_eq!(doc.plain_text, "first para\nsecond para\n");

        let doc = MarkdownDocument::parse("> Note:\n>\n> - one\n> - two\n");
        assert_eq!(doc.nodes.len(), 1, "the list must not escape the quote");
        let Node::Blockquote { blocks } = &doc.nodes[0] else {
            panic!("expected a blockquote");
        };
        assert!(matches!(
            blocks.as_slice(),
            [Node::Paragraph { .. }, Node::List { .. }]
        ));
    }

    /// The containers nest both ways round.
    #[test]
    fn quotes_and_lists_nest_in_each_other() {
        let doc = MarkdownDocument::parse("1. Step:\n\n   > watch out\n\n2. Next\n");
        assert_eq!(doc.nodes.len(), 1);
        let (_, _, items) = expect_list(&doc.nodes[0]);
        assert_eq!(items.len(), 2);
        assert!(matches!(
            items[0].blocks.as_slice(),
            [Node::Paragraph { .. }, Node::Blockquote { .. }]
        ));
        assert_eq!(item_text(&items[0]), "Step:\nwatch out\n");
    }

    /// Markers follow the source numbering rather than always restarting at 1.
    #[test]
    fn ordered_list_keeps_its_first_number() {
        let doc = MarkdownDocument::parse("3. three\n4. four\n");
        let (ordered, start, items) = expect_list(&doc.nodes[0]);
        assert!(ordered);
        assert_eq!(start, 3);
        assert_eq!(items.len(), 2);
    }

    /// Selection maps a character offset onto `plain_text`, so every node's
    /// reported length must add up to it, nested blocks included.
    #[test]
    fn nested_block_lengths_match_the_flat_text() {
        let content = "\
# Title

1. First

2. Second:
   - a
   - b

   ```sh
   run me
   ```

3. Third

> A quote,
>
> in two paragraphs.
";
        let doc = MarkdownDocument::parse(content);
        let total: usize = doc
            .nodes
            .iter()
            .map(MarkdownDocument::node_text_length)
            .sum();
        assert_eq!(total, doc.plain_text.chars().count());
        assert!(doc.plain_text.contains("run me"));
    }

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
