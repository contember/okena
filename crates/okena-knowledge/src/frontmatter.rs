//! YAML frontmatter at the top of a Markdown file, read leniently.
//!
//! Fences follow `okena-markdown`'s `split_frontmatter`, which draws the
//! metadata card in the viewer. That crate is GPUI-bound, so the daemon cannot
//! share it — keep the two rules in step, or a file the viewer shows metadata
//! for would list here without it.

use serde_yaml_ng::{Mapping, Value};

/// Split a leading `---` block from the rest: `(yaml, body)`.
///
/// The opening `---` must be the first line; the block closes at a line that
/// is exactly `---` or `...`. `None` when there is no well-formed block.
pub fn split(content: &str) -> Option<(&str, &str)> {
    let first = content.split_inclusive('\n').next()?;
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return None;
    }
    // A bare `---` with nothing after it is a horizontal rule.
    let body = &content[first.len()..];
    if body.is_empty() {
        return None;
    }
    let mut offset = first.len();
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed == "---" || trimmed == "..." {
            return Some((
                &content[first.len()..offset],
                &content[offset + line.len()..],
            ));
        }
        offset += line.len();
    }
    None
}

/// A file's frontmatter fields.
#[derive(Debug, Default)]
pub struct Frontmatter {
    fields: Mapping,
    /// Why a block that was present could not be read as key/value pairs.
    pub error: Option<String>,
}

/// Parse `content`'s frontmatter, returning it with the body that follows.
///
/// Never fails: a broken block yields no fields and an [`Frontmatter::error`],
/// and the whole file is still an entry.
pub fn parse(content: &str) -> (Frontmatter, &str) {
    let Some((yaml, body)) = split(content) else {
        return (Frontmatter::default(), content);
    };
    let fm = match serde_yaml_ng::from_str::<Value>(yaml) {
        Ok(Value::Mapping(fields)) => Frontmatter {
            fields,
            error: None,
        },
        Ok(Value::Null) => Frontmatter::default(),
        Ok(_) => Frontmatter {
            fields: Mapping::new(),
            error: Some("frontmatter is not a set of `key: value` pairs".into()),
        },
        Err(e) => Frontmatter {
            fields: Mapping::new(),
            error: Some(e.to_string()),
        },
    };
    (fm, body)
}

impl Frontmatter {
    /// A non-empty scalar field as text. Numbers and booleans count —
    /// `title: 2024` is still a title.
    pub fn string(&self, key: &str) -> Option<String> {
        self.fields.get(key).and_then(scalar)
    }

    /// A list field. A single string is accepted too and split on commas, the
    /// way Claude subagents write `tools: Read, Grep`.
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.fields.get(key) {
            Some(Value::Sequence(items)) => items.iter().filter_map(scalar).collect(),
            Some(Value::String(s)) => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect(),
            Some(other) => scalar(other).into_iter().collect(),
            None => Vec::new(),
        }
    }
}

fn scalar(value: &Value) -> Option<String> {
    let text = match value {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => return None,
    };
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_needs_an_opening_fence_on_the_first_line_and_a_close() {
        assert_eq!(split("---\na: 1\n---\nbody"), Some(("a: 1\n", "body")));
        assert_eq!(
            split("---\r\na: 1\r\n...\r\nbody"),
            Some(("a: 1\r\n", "body"))
        );
        assert_eq!(split("---\n---\n"), Some(("", "")));
        assert_eq!(split("\n---\na: 1\n---\n"), None, "not on the first line");
        assert_eq!(split("---\na: 1\n"), None, "never closed");
        assert_eq!(split("---"), None, "a horizontal rule");
        assert_eq!(split("# Title"), None);
    }

    #[test]
    fn fields_read_as_text_and_lists_accept_one_string() {
        let (fm, body) = parse(
            "---\ntitle: 2024 plan\nyear: 2024\ndraft: true\ntags: [ci, release]\ntools: Read, Grep\nfor: task-start\nempty: ''\n---\n# Body\n",
        );
        assert!(fm.error.is_none());
        assert_eq!(body, "# Body\n");
        assert_eq!(fm.string("title").as_deref(), Some("2024 plan"));
        assert_eq!(fm.string("year").as_deref(), Some("2024"));
        assert_eq!(fm.string("draft").as_deref(), Some("true"));
        assert_eq!(fm.string("empty"), None);
        assert_eq!(fm.string("missing"), None);
        assert_eq!(fm.list("tags"), ["ci", "release"]);
        assert_eq!(fm.list("tools"), ["Read", "Grep"]);
        assert_eq!(fm.list("for"), ["task-start"]);
        assert!(fm.list("missing").is_empty());
    }

    #[test]
    fn a_broken_block_reports_why_and_keeps_the_body() {
        let (fm, body) = parse("---\ntitle: [unclosed\n---\ntext");
        assert!(fm.error.is_some());
        assert_eq!(fm.string("title"), None);
        assert_eq!(body, "text");

        let (fm, _) = parse("---\n- a\n- b\n---\n");
        assert!(
            fm.error
                .as_deref()
                .is_some_and(|e| e.contains("key: value"))
        );

        let (fm, _) = parse("---\n\n---\n");
        assert!(fm.error.is_none(), "an empty block is not an error");
    }

    #[test]
    fn no_block_means_the_whole_file_is_body() {
        let (fm, body) = parse("# Title\n---\n");
        assert!(fm.error.is_none());
        assert_eq!(body, "# Title\n---\n");
    }
}
