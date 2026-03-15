# Issue 04: KruhPane status parser and prompt builder

**Priority:** high
**Files:** `src/views/layout/kruh_pane/status_parser.rs` (new)

## Description

Port kruh's `status.ts` and `prompt.ts` to Rust. This module parses STATUS.md files and builds prompts for the AI agents. It is a standalone module with no GPUI dependencies — pure Rust with file I/O.

## New file: `src/views/layout/kruh_pane/status_parser.rs`

### `parse_status(docs_dir: &str) -> std::io::Result<StatusProgress>`

Reads `{docs_dir}/STATUS.md` and parses task checkboxes:

```rust
use std::path::Path;
use regex::Regex;  // or just use str methods — regex may be overkill

pub fn parse_status(docs_dir: &str) -> std::io::Result<StatusProgress> {
    let path = Path::new(docs_dir).join("STATUS.md");
    let content = std::fs::read_to_string(&path)?;
    parse_status_content(&content)
}

pub fn parse_status_content(content: &str) -> std::io::Result<StatusProgress> {
    let mut progress = StatusProgress::default();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("- [ ] ") {
            let name = trimmed.strip_prefix("- [ ] ").unwrap().to_string();
            let issue_ref = extract_issue_ref(&name);
            progress.pending_issues.push(issue_ref.name.clone());
            progress.pending_refs.push(issue_ref);
            progress.pending += 1;
        } else if trimmed.starts_with("- [x] ") {
            let name = trimmed.strip_prefix("- [x] ").unwrap().to_string();
            let issue_ref = extract_issue_ref(&name);
            progress.done_issues.push(issue_ref.name.clone());
            progress.done_refs.push(issue_ref);
            progress.done += 1;
        }
    }

    progress.total = progress.pending + progress.done;
    Ok(progress)
}
```

### `extract_issue_ref(text: &str) -> IssueRef`

Parses patterns like `04 — Status parser` or `04 - Status parser`:

```rust
fn extract_issue_ref(text: &str) -> IssueRef {
    // Match: digits, optional whitespace, dash/em-dash/en-dash, optional whitespace, rest
    // Pattern: ^(\d+)\s*[—–-]\s*(.+)$
    if let Some(idx) = text.find(|c: char| c == '—' || c == '–' || c == '-') {
        let prefix = text[..idx].trim();
        if prefix.chars().all(|c| c.is_ascii_digit()) && !prefix.is_empty() {
            let name = text[idx..].trim_start_matches(|c: char| c == '—' || c == '–' || c == '-').trim();
            return IssueRef { number: prefix.to_string(), name: name.to_string() };
        }
    }
    IssueRef { number: String::new(), name: text.to_string() }
}
```

### `build_prompt(docs_dir: &str) -> std::io::Result<String>`

Generates the agent prompt. Checks if `{docs_dir}/issues/` directory exists to customize:

```rust
pub fn build_prompt(docs_dir: &str) -> std::io::Result<String> {
    let has_issues_dir = Path::new(docs_dir).join("issues").is_dir();

    let issues_instruction = if has_issues_dir {
        "read its file from issues/, "
    } else {
        ""
    };

    Ok(format!(
        "Read {docs_dir}/INSTRUCTIONS.md and {docs_dir}/STATUS.md. \
         Find the first pending issue, {issues_instruction}implement it, \
         verify (type-check, tests, build), and update STATUS.md. \
         If no pending issues remain, respond: <done>promise</done>"
    ))
}
```

### `check_done_sentinel(output: &str) -> bool`

Checks if agent output contains the completion sentinel:

```rust
pub fn check_done_sentinel(output: &str) -> bool {
    output.contains("<done>promise</done>")
}
```

## Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_all_pending() {
        let content = "# Status\n\n- [ ] 01 — Add types\n- [ ] 02 — Add parser\n";
        let progress = parse_status_content(content).unwrap();
        assert_eq!(progress.pending, 2);
        assert_eq!(progress.done, 0);
        assert_eq!(progress.total, 2);
    }

    #[test]
    fn test_parse_all_done() {
        let content = "- [x] 01 — Add types\n- [x] 02 — Add parser\n";
        let progress = parse_status_content(content).unwrap();
        assert_eq!(progress.pending, 0);
        assert_eq!(progress.done, 2);
        assert_eq!(progress.total, 2);
    }

    #[test]
    fn test_parse_mixed() {
        let content = "- [x] 01 — Done task\n- [ ] 02 — Pending task\n- [x] 03 — Another done\n";
        let progress = parse_status_content(content).unwrap();
        assert_eq!(progress.pending, 1);
        assert_eq!(progress.done, 2);
        assert_eq!(progress.total, 3);
        assert_eq!(progress.pending_issues, vec!["Pending task"]);
    }

    #[test]
    fn test_parse_empty() {
        let progress = parse_status_content("# Status\n\n## Pending\n").unwrap();
        assert_eq!(progress.total, 0);
    }

    #[test]
    fn test_extract_issue_ref_with_number() {
        let r = extract_issue_ref("04 — Status parser");
        assert_eq!(r.number, "04");
        assert_eq!(r.name, "Status parser");
    }

    #[test]
    fn test_extract_issue_ref_with_dash() {
        let r = extract_issue_ref("04 - Status parser");
        assert_eq!(r.number, "04");
        assert_eq!(r.name, "Status parser");
    }

    #[test]
    fn test_extract_issue_ref_without_number() {
        let r = extract_issue_ref("Just a task name");
        assert_eq!(r.number, "");
        assert_eq!(r.name, "Just a task name");
    }

    #[test]
    fn test_build_prompt_with_issues_dir() {
        // Use a temp dir with issues/ subdirectory
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("issues")).unwrap();
        let prompt = build_prompt(dir.path().to_str().unwrap()).unwrap();
        assert!(prompt.contains("read its file from issues/"));
        assert!(prompt.contains("<done>promise</done>"));
    }

    #[test]
    fn test_build_prompt_without_issues_dir() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = build_prompt(dir.path().to_str().unwrap()).unwrap();
        assert!(!prompt.contains("read its file from issues/"));
        assert!(prompt.contains("<done>promise</done>"));
    }

    #[test]
    fn test_check_done_sentinel() {
        assert!(check_done_sentinel("some output\n<done>promise</done>\n"));
        assert!(!check_done_sentinel("some output without sentinel"));
    }
}
```

Note: If `tempfile` is not a dev dependency, add it or use `std::env::temp_dir()` with manual cleanup.

## Acceptance Criteria

- `parse_status()` correctly parses all STATUS.md variants
- `extract_issue_ref()` handles numbered and unnumbered items
- `build_prompt()` adapts based on issues/ directory presence
- `check_done_sentinel()` detects the completion marker
- All tests pass
