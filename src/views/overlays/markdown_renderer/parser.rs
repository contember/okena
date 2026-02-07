//! Markdown parsing logic.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::types::{Inline, Node};
use super::MarkdownDocument;

impl MarkdownDocument {
    /// Parse markdown content into a document.
    pub fn parse(content: &str) -> Self {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_TABLES);
        let parser = Parser::new_ext(content, options);

        let mut nodes = Vec::new();
        let mut inline_stack: Vec<Vec<Inline>> = vec![Vec::new()];

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
                Event::End(TagEnd::Paragraph) => {
                    if in_paragraph {
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
                    nodes.push(Node::Table {
                        headers: std::mem::take(&mut table_headers),
                        rows: std::mem::take(&mut table_rows),
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
                Event::End(TagEnd::TableRow) => {
                    if !in_table_head {
                        table_rows.push(std::mem::take(&mut current_row));
                    }
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
                    // Store URL temporarily - we'll use it on End
                    if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Text(format!("\x00LINK:{}\x00", dest_url)));
                    }
                }
                Event::End(TagEnd::Link) => {
                    let mut children = inline_stack.pop().unwrap_or_default();
                    // Extract URL from marker
                    let url = children.iter().find_map(|c| {
                        if let Inline::Text(t) = c {
                            if t.starts_with("\x00LINK:") && t.ends_with("\x00") {
                                return Some(t[6..t.len()-1].to_string());
                            }
                        }
                        None
                    }).unwrap_or_default();
                    children.retain(|c| {
                        if let Inline::Text(t) = c {
                            !t.starts_with("\x00LINK:")
                        } else {
                            true
                        }
                    });
                    if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Link { _url: url, children });
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
                    } else if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Text(text.to_string()));
                    }
                }
                Event::SoftBreak | Event::HardBreak => {
                    if in_code_block {
                        code_block_content.push('\n');
                    } else if let Some(last) = inline_stack.last_mut() {
                        last.push(Inline::Text(" ".to_string()));
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

        Self { nodes, plain_text }
    }

    /// Convert a node to flat text (in characters, not bytes).
    pub(super) fn node_to_flat_text(node: &Node, text: &mut String) {
        match node {
            Node::Heading { children, .. } |
            Node::Paragraph { children } |
            Node::Blockquote { children } => {
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
            Node::Table { headers, rows } => {
                for (i, header) in headers.iter().enumerate() {
                    if i > 0 { text.push('\t'); }
                    Self::inlines_to_flat_text(header, text);
                }
                text.push('\n');
                for row in rows {
                    for (i, cell) in row.iter().enumerate() {
                        if i > 0 { text.push('\t'); }
                        Self::inlines_to_flat_text(cell, text);
                    }
                    text.push('\n');
                }
            }
            Node::HorizontalRule => {
                text.push('\n');
            }
        }
    }

    /// Convert inline elements to flat text.
    pub(super) fn inlines_to_flat_text(inlines: &[Inline], text: &mut String) {
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
