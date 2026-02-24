use std::path::Path;

use super::config::KruhPlanOverrides;
use super::types::{IssueDetail, IssueRef, PlanInfo, StatusProgress};

pub fn parse_status(docs_dir: &str) -> std::io::Result<StatusProgress> {
    let path = Path::new(docs_dir).join("STATUS.md");
    let content = std::fs::read_to_string(&path)?;
    parse_status_content(&content)
}

pub fn parse_status_content(content: &str) -> std::io::Result<StatusProgress> {
    let mut progress = StatusProgress::default();

    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_prefix("- [ ] ") {
            let issue_ref = extract_issue_ref(name);
            progress.pending_issues.push(issue_ref.name.clone());
            progress.pending_refs.push(issue_ref);
            progress.pending += 1;
        } else if let Some(name) = trimmed.strip_prefix("- [x] ") {
            let issue_ref = extract_issue_ref(name);
            progress.done_issues.push(issue_ref.name.clone());
            progress.done_refs.push(issue_ref);
            progress.done += 1;
        }
    }

    progress.total = progress.pending + progress.done;
    Ok(progress)
}

pub fn extract_issue_ref(text: &str) -> IssueRef {
    // Match: digits, optional whitespace, dash/em-dash/en-dash, optional whitespace, rest
    if let Some(idx) = text.find(|c: char| c == '\u{2014}' || c == '\u{2013}' || c == '-') {
        let prefix = text[..idx].trim();
        if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
            let name = text[idx..]
                .trim_start_matches(|c: char| c == '\u{2014}' || c == '\u{2013}' || c == '-')
                .trim();
            return IssueRef { number: prefix.to_string(), name: name.to_string() };
        }
    }
    IssueRef { number: String::new(), name: text.to_string() }
}

pub fn build_prompt(docs_dir: &str) -> std::io::Result<String> {
    let has_issues_dir = Path::new(docs_dir).join("issues").is_dir();

    let issues_instruction = if has_issues_dir { "read its file from issues/, " } else { "" };

    Ok(format!(
        "Read {docs_dir}/INSTRUCTIONS.md and {docs_dir}/STATUS.md. \
         Find the first pending issue, {issues_instruction}implement it, \
         verify (type-check, tests, build), and update STATUS.md. \
         If no pending issues remain, respond: <done>promise</done>"
    ))
}

pub fn check_done_sentinel(output: &str) -> bool {
    output.contains("<done>promise</done>")
}

/// Scan a parent directory for plan subdirectories.
///
/// A valid plan directory must contain STATUS.md. If INSTRUCTIONS.md or an issues/
/// subdirectory also exist, they are considered plan markers but STATUS.md is required
/// for progress parsing.
pub fn scan_plans(parent_dir: &str) -> Vec<PlanInfo> {
    let parent = Path::new(parent_dir);
    if !parent.is_dir() {
        return Vec::new();
    }

    let mut plans = Vec::new();

    let mut entries: Vec<_> = match std::fs::read_dir(parent) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        Err(_) => return Vec::new(),
    };

    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let dir_path = entry.path();
        let status_path = dir_path.join("STATUS.md");

        if !status_path.exists() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let dir = dir_path.to_string_lossy().to_string();

        let (pending, done, total) = match std::fs::read_to_string(&status_path) {
            Ok(content) => match parse_status_content(&content) {
                Ok(progress) => (progress.pending, progress.done, progress.total),
                Err(_) => (0, 0, 0),
            },
            Err(_) => (0, 0, 0),
        };

        plans.push(PlanInfo { name, dir, pending, done, total });
    }

    // Sort: incomplete plans first, then by name
    plans.sort_by(|a, b| {
        let a_complete = a.pending == 0 && a.total > 0;
        let b_complete = b.pending == 0 && b.total > 0;
        a_complete.cmp(&b_complete).then_with(|| a.name.cmp(&b.name))
    });

    plans
}

/// Load issue details from a plan's STATUS.md, with optional file previews from issues/.
pub fn load_issue_details(docs_dir: &str) -> Vec<IssueDetail> {
    let progress = match parse_status(docs_dir) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut issues = Vec::new();

    // Pending issues first
    for ref_info in &progress.pending_refs {
        let preview = load_issue_preview(docs_dir, &ref_info.number);
        let overrides = parse_issue_overrides(docs_dir, &ref_info.number);
        issues.push(IssueDetail {
            ref_info: ref_info.clone(),
            raw_name: format!("{} — {}", ref_info.number, ref_info.name),
            done: false,
            preview,
            overrides,
        });
    }

    // Done issues
    for ref_info in &progress.done_refs {
        let preview = load_issue_preview(docs_dir, &ref_info.number);
        let overrides = parse_issue_overrides(docs_dir, &ref_info.number);
        issues.push(IssueDetail {
            ref_info: ref_info.clone(),
            raw_name: format!("{} — {}", ref_info.number, ref_info.name),
            done: true,
            preview,
            overrides,
        });
    }

    issues
}

/// Find the full path to an issue file by its number prefix.
pub fn find_issue_file_path(docs_dir: &str, issue_number: &str) -> Option<String> {
    if issue_number.is_empty() {
        return None;
    }
    let issues_dir = Path::new(docs_dir).join("issues");
    if !issues_dir.is_dir() {
        return None;
    }
    let entries = std::fs::read_dir(&issues_dir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(issue_number) && name.ends_with(".md") {
            return Some(entry.path().to_string_lossy().to_string());
        }
    }
    None
}

/// Load the first 3 lines from an issue file in the issues/ subdirectory.
pub fn load_issue_preview(docs_dir: &str, issue_number: &str) -> Option<String> {
    if issue_number.is_empty() {
        return None;
    }

    let issues_dir = Path::new(docs_dir).join("issues");
    if !issues_dir.is_dir() {
        return None;
    }

    // Look for files matching the issue number prefix (e.g., "01-some-name.md")
    let entries = std::fs::read_dir(&issues_dir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with(issue_number) && name.ends_with(".md") {
            let content = std::fs::read_to_string(entry.path()).ok()?;
            let preview: String = content
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join("\n");
            return if preview.is_empty() { None } else { Some(preview) };
        }
    }

    None
}

/// Strip YAML frontmatter from content, returning the body after the closing `---`.
pub fn strip_frontmatter(content: &str) -> &str {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return content;
    }
    let after_open = &trimmed[3..];
    let after_open = after_open.trim_start_matches(|c: char| c == '\r' || c == '\n');
    match after_open.find("\n---") {
        Some(pos) => {
            let after_close = &after_open[pos + 4..];
            after_close.trim_start_matches(|c: char| c == '\r' || c == '\n')
        }
        None => content,
    }
}

/// Parse overrides from a specific issue file's YAML frontmatter.
pub fn parse_issue_overrides(docs_dir: &str, issue_number: &str) -> KruhPlanOverrides {
    if let Some(path) = find_issue_file_path(docs_dir, issue_number) {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return KruhPlanOverrides::default(),
        };
        parse_plan_overrides_content(&content)
    } else {
        KruhPlanOverrides::default()
    }
}

/// Parse per-plan config overrides from YAML frontmatter in INSTRUCTIONS.md.
///
/// Looks for `---` delimited frontmatter at the top of the file and parses
/// supported fields: `agent`, `model`, `max_iterations`, `sleep_secs`, `dangerous`.
pub fn parse_plan_overrides(docs_dir: &str) -> KruhPlanOverrides {
    let path = Path::new(docs_dir).join("INSTRUCTIONS.md");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return KruhPlanOverrides::default(),
    };
    parse_plan_overrides_content(&content)
}

/// Parse overrides from frontmatter content (testable without filesystem).
pub fn parse_plan_overrides_content(content: &str) -> KruhPlanOverrides {
    let mut overrides = KruhPlanOverrides::default();

    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return overrides;
    }

    // Find the closing ---
    let after_open = &trimmed[3..];
    let after_open = after_open.trim_start_matches(|c: char| c == '\r' || c == '\n');
    let end = match after_open.find("\n---") {
        Some(pos) => pos,
        None => return overrides,
    };

    let frontmatter = &after_open[..end];

    for line in frontmatter.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            let value = value.trim();
            match key {
                "agent" => overrides.agent = Some(value.to_string()),
                "model" => overrides.model = Some(value.to_string()),
                "max_iterations" => {
                    if let Ok(n) = value.parse::<usize>() {
                        overrides.max_iterations = Some(n);
                    }
                }
                "sleep_secs" => {
                    if let Ok(n) = value.parse::<u64>() {
                        overrides.sleep_secs = Some(n);
                    }
                }
                "dangerous" => {
                    if let Ok(b) = value.parse::<bool>() {
                        overrides.dangerous = Some(b);
                    }
                }
                _ => {}
            }
        }
    }

    overrides
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_all_pending() {
        let content = "# Status\n\n- [ ] 01 \u{2014} Add types\n- [ ] 02 \u{2014} Add parser\n";
        let progress = parse_status_content(content).unwrap();
        assert_eq!(progress.pending, 2);
        assert_eq!(progress.done, 0);
        assert_eq!(progress.total, 2);
    }

    #[test]
    fn test_parse_all_done() {
        let content = "- [x] 01 \u{2014} Add types\n- [x] 02 \u{2014} Add parser\n";
        let progress = parse_status_content(content).unwrap();
        assert_eq!(progress.pending, 0);
        assert_eq!(progress.done, 2);
        assert_eq!(progress.total, 2);
    }

    #[test]
    fn test_parse_mixed() {
        let content =
            "- [x] 01 \u{2014} Done task\n- [ ] 02 \u{2014} Pending task\n- [x] 03 \u{2014} Another done\n";
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
        let r = extract_issue_ref("04 \u{2014} Status parser");
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
        let dir = std::env::temp_dir().join("kruh_test_with_issues");
        let _ = std::fs::create_dir_all(dir.join("issues"));
        let prompt = build_prompt(dir.to_str().unwrap()).unwrap();
        assert!(prompt.contains("read its file from issues/"));
        assert!(prompt.contains("<done>promise</done>"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_build_prompt_without_issues_dir() {
        let dir = std::env::temp_dir().join("kruh_test_without_issues");
        let _ = std::fs::create_dir_all(&dir);
        // Make sure issues/ doesn't exist
        let _ = std::fs::remove_dir_all(dir.join("issues"));
        let prompt = build_prompt(dir.to_str().unwrap()).unwrap();
        assert!(!prompt.contains("read its file from issues/"));
        assert!(prompt.contains("<done>promise</done>"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_check_done_sentinel() {
        assert!(check_done_sentinel("some output\n<done>promise</done>\n"));
        assert!(!check_done_sentinel("some output without sentinel"));
    }

    #[test]
    fn test_scan_plans_empty_dir() {
        let dir = std::env::temp_dir().join("kruh_scan_empty");
        let _ = std::fs::create_dir_all(&dir);
        let plans = scan_plans(dir.to_str().unwrap());
        assert!(plans.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_plans_valid() {
        let dir = std::env::temp_dir().join("kruh_scan_valid");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        // Plan with mixed progress
        let plan1 = dir.join("plan-alpha");
        std::fs::create_dir_all(&plan1).unwrap();
        std::fs::write(
            plan1.join("STATUS.md"),
            "- [x] 01 \u{2014} Done\n- [ ] 02 \u{2014} Pending\n",
        )
        .unwrap();
        std::fs::write(plan1.join("INSTRUCTIONS.md"), "# Instructions").unwrap();

        // Fully completed plan
        let plan2 = dir.join("plan-beta");
        std::fs::create_dir_all(&plan2).unwrap();
        std::fs::write(
            plan2.join("STATUS.md"),
            "- [x] 01 \u{2014} Done\n- [x] 02 \u{2014} Also done\n",
        )
        .unwrap();

        let plans = scan_plans(dir.to_str().unwrap());
        assert_eq!(plans.len(), 2);
        // Incomplete first
        assert_eq!(plans[0].name, "plan-alpha");
        assert_eq!(plans[0].pending, 1);
        assert_eq!(plans[0].done, 1);
        assert_eq!(plans[0].total, 2);
        // Completed second
        assert_eq!(plans[1].name, "plan-beta");
        assert_eq!(plans[1].pending, 0);
        assert_eq!(plans[1].done, 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_plans_skips_non_plan_dirs() {
        let dir = std::env::temp_dir().join("kruh_scan_skip");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);

        // Directory without STATUS.md — should be skipped
        let not_plan = dir.join("random-dir");
        std::fs::create_dir_all(&not_plan).unwrap();
        std::fs::write(not_plan.join("README.md"), "# Not a plan").unwrap();

        // Regular file — should be skipped
        std::fs::write(dir.join("some-file.txt"), "not a dir").unwrap();

        let plans = scan_plans(dir.to_str().unwrap());
        assert!(plans.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_scan_plans_missing_dir() {
        let plans = scan_plans("/tmp/nonexistent_kruh_dir_12345");
        assert!(plans.is_empty());
    }

    #[test]
    fn test_load_issue_details_mixed() {
        let dir = std::env::temp_dir().join("kruh_issue_details");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("issues")).unwrap();

        std::fs::write(
            dir.join("STATUS.md"),
            "- [ ] 01 \u{2014} Pending task\n- [x] 02 \u{2014} Done task\n",
        )
        .unwrap();

        std::fs::write(
            dir.join("issues/01-pending-task.md"),
            "# Pending Task\nLine 2\nLine 3\nLine 4 should not appear",
        )
        .unwrap();

        let issues = load_issue_details(dir.to_str().unwrap());
        assert_eq!(issues.len(), 2);

        // Pending first
        assert!(!issues[0].done);
        assert_eq!(issues[0].ref_info.number, "01");
        assert!(issues[0].preview.is_some());
        let preview = issues[0].preview.as_ref().unwrap();
        assert!(preview.contains("# Pending Task"));
        assert!(preview.contains("Line 3"));
        assert!(!preview.contains("Line 4"));

        // Done second
        assert!(issues[1].done);
        assert_eq!(issues[1].ref_info.number, "02");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_parse_plan_overrides_with_frontmatter() {
        let content = "---\nagent: codex\nmodel: gpt-4\nmax_iterations: 50\nsleep_secs: 5\ndangerous: false\n---\n\n# Instructions\n";
        let overrides = parse_plan_overrides_content(content);
        assert_eq!(overrides.agent.as_deref(), Some("codex"));
        assert_eq!(overrides.model.as_deref(), Some("gpt-4"));
        assert_eq!(overrides.max_iterations, Some(50));
        assert_eq!(overrides.sleep_secs, Some(5));
        assert_eq!(overrides.dangerous, Some(false));
    }

    #[test]
    fn test_parse_plan_overrides_no_frontmatter() {
        let content = "# Instructions\n\nJust do the thing.\n";
        let overrides = parse_plan_overrides_content(content);
        assert!(overrides.agent.is_none());
        assert!(overrides.model.is_none());
        assert!(overrides.max_iterations.is_none());
    }

    #[test]
    fn test_parse_plan_overrides_missing_file() {
        let overrides = parse_plan_overrides("/tmp/nonexistent_kruh_overrides_12345");
        assert!(overrides.agent.is_none());
    }

    #[test]
    fn test_parse_plan_overrides_partial() {
        let content = "---\nagent: aider\n---\n\n# Instructions\n";
        let overrides = parse_plan_overrides_content(content);
        assert_eq!(overrides.agent.as_deref(), Some("aider"));
        assert!(overrides.model.is_none());
        assert!(overrides.max_iterations.is_none());
        assert!(overrides.sleep_secs.is_none());
        assert!(overrides.dangerous.is_none());
    }

    #[test]
    fn test_strip_frontmatter_with_frontmatter() {
        let content = "---\nagent: codex\nmodel: gpt-4\n---\n\n# Instructions\n\nDo stuff.\n";
        let body = strip_frontmatter(content);
        assert_eq!(body, "# Instructions\n\nDo stuff.\n");
    }

    #[test]
    fn test_strip_frontmatter_without_frontmatter() {
        let content = "# Instructions\n\nDo stuff.\n";
        let body = strip_frontmatter(content);
        assert_eq!(body, content);
    }

    #[test]
    fn test_strip_frontmatter_unclosed() {
        let content = "---\nagent: codex\n# No closing delimiter\n";
        let body = strip_frontmatter(content);
        assert_eq!(body, content);
    }

    #[test]
    fn test_strip_frontmatter_empty_body() {
        let content = "---\nagent: codex\n---\n";
        let body = strip_frontmatter(content);
        assert_eq!(body, "");
    }
}
