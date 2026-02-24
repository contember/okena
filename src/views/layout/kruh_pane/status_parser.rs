use std::path::Path;

use time::OffsetDateTime;

use super::agent_instructions::DEFAULT_AGENT_INSTRUCTIONS;
use super::config::KruhPlanOverrides;
use super::types::{IssueDetail, IssueRef, PlanInfo, StatusProgress};

const KNOWN_CONFIG_KEYS: &[&str] = &["agent", "model", "max_iterations", "sleep_secs", "dangerous"];

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

pub fn ensure_agent_md(plans_dir: &str) -> std::io::Result<()> {
    if plans_dir.is_empty() {
        return Ok(());
    }
    let path = Path::new(plans_dir).join("AGENT.md");
    if path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(plans_dir)?;
    std::fs::write(&path, DEFAULT_AGENT_INSTRUCTIONS)?;
    Ok(())
}

/// Resolve the INSTRUCTIONS.md path: per-plan first, then shared .plans/ level.
pub fn resolve_instructions_path(docs_dir: &str, plans_dir: &str) -> String {
    let per_plan = Path::new(docs_dir).join("INSTRUCTIONS.md");
    if per_plan.exists() {
        return per_plan.to_string_lossy().to_string();
    }
    let shared = Path::new(plans_dir).join("INSTRUCTIONS.md");
    shared.to_string_lossy().to_string()
}

pub fn build_prompt(docs_dir: &str, plans_dir: &str) -> std::io::Result<String> {
    let has_issues_dir = Path::new(docs_dir).join("issues").is_dir();

    let issues_instruction = if has_issues_dir { "read its file from issues/, " } else { "" };

    let instructions_path = resolve_instructions_path(docs_dir, plans_dir);

    let agent_md_path = Path::new(plans_dir).join("AGENT.md");
    let prefix = if !plans_dir.is_empty() && agent_md_path.exists() {
        format!(
            "First read {}/AGENT.md for general workflow guidelines. Then r",
            plans_dir
        )
    } else {
        "R".to_string()
    };

    Ok(format!(
        "{prefix}ead {instructions_path} and {docs_dir}/STATUS.md. \
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
/// Uses the fallback chain: per-plan INSTRUCTIONS.md first, then shared .plans/ level.
pub fn parse_plan_overrides(docs_dir: &str, plans_dir: &str) -> KruhPlanOverrides {
    let path = resolve_instructions_path(docs_dir, plans_dir);
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

/// Update or insert frontmatter key-value pairs in content.
///
/// If frontmatter exists, existing keys are updated in place and new keys are appended.
/// If no frontmatter exists, a new `---` section is created at the top.
/// Body content after the closing `---` is preserved exactly (including blank lines).
pub fn update_frontmatter_content(content: &str, updates: &[(&str, &str)]) -> String {
    let trimmed = content.trim_start();
    if trimmed.starts_with("---") {
        let after_open = &trimmed[3..];
        let after_open = after_open.trim_start_matches(|c: char| c == '\r' || c == '\n');
        if let Some(pos) = after_open.find("\n---") {
            let frontmatter_block = &after_open[..pos];
            let suffix = &after_open[pos + 4..];

            let mut pairs: Vec<(String, String)> = Vec::new();
            for line in frontmatter_block.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once(':') {
                    pairs.push((key.trim().to_string(), value.trim().to_string()));
                }
            }

            for (key, value) in updates {
                if let Some(existing) = pairs.iter_mut().find(|(k, _)| k.as_str() == *key) {
                    existing.1 = value.to_string();
                } else {
                    pairs.push((key.to_string(), value.to_string()));
                }
            }

            let mut result = String::from("---\n");
            for (k, v) in &pairs {
                result.push_str(&format!("{}: {}\n", k, v));
            }
            result.push_str("---");
            result.push_str(suffix);
            return result;
        }
    }

    // No frontmatter (or unclosed): create new section
    let mut result = String::from("---\n");
    for (k, v) in updates {
        result.push_str(&format!("{}: {}\n", k, v));
    }
    result.push_str("---\n");
    if !content.is_empty() {
        result.push('\n');
        result.push_str(content);
    }
    result
}

/// I/O wrapper: reads file, calls `update_frontmatter_content`, writes back.
pub fn update_issue_frontmatter(file_path: &str, updates: &[(&str, &str)]) -> std::io::Result<()> {
    let content = std::fs::read_to_string(file_path)?;
    let updated = update_frontmatter_content(&content, updates);
    std::fs::write(file_path, updated)?;
    Ok(())
}

/// Returns all frontmatter key-value pairs that are NOT known config override keys.
///
/// Known config keys: `agent`, `model`, `max_iterations`, `sleep_secs`, `dangerous`.
/// Everything else (e.g. `startedAt`, `endedAt`, `exitCode`, `duration`, `iteration`) is returned.
pub fn extract_extra_frontmatter_keys(content: &str) -> Vec<(String, String)> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Vec::new();
    }
    let after_open = &trimmed[3..];
    let after_open = after_open.trim_start_matches(|c: char| c == '\r' || c == '\n');
    let pos = match after_open.find("\n---") {
        Some(p) => p,
        None => return Vec::new(),
    };
    let frontmatter_block = &after_open[..pos];

    let mut extras = Vec::new();
    for line in frontmatter_block.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim();
            if !KNOWN_CONFIG_KEYS.contains(&key) {
                extras.push((key.to_string(), value.trim().to_string()));
            }
        }
    }

    extras
}

/// Format current local time as `YYYY-MM-DDTHH:MM:SS`.
pub fn iso_now() -> String {
    let now = match OffsetDateTime::now_local() {
        Ok(t) => t,
        Err(_) => OffsetDateTime::now_utc(),
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
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
        let prompt = build_prompt(dir.to_str().unwrap(), "").unwrap();
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
        let prompt = build_prompt(dir.to_str().unwrap(), "").unwrap();
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
        let overrides = parse_plan_overrides(
            "/tmp/nonexistent_kruh_overrides_12345",
            "/tmp/nonexistent_kruh_overrides_12345",
        );
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

    #[test]
    fn test_ensure_agent_md_creates_file() {
        let dir = std::env::temp_dir().join("kruh_agent_md_create");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        ensure_agent_md(dir.to_str().unwrap()).unwrap();

        let path = dir.join("AGENT.md");
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Agent Workflow Guidelines"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_agent_md_skips_existing() {
        let dir = std::env::temp_dir().join("kruh_agent_md_skip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("AGENT.md");
        std::fs::write(&path, "# My custom instructions\n").unwrap();

        ensure_agent_md(dir.to_str().unwrap()).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "# My custom instructions\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_ensure_agent_md_empty_plans_dir() {
        // Should be a no-op, not an error
        ensure_agent_md("").unwrap();
    }

    #[test]
    fn test_update_frontmatter_add_to_existing() {
        let content = "---\nmodel: gpt-4\n---\n\n# Issue";
        let result = update_frontmatter_content(content, &[("startedAt", "2026-02-25T14:30:45")]);
        assert!(result.contains("model: gpt-4"));
        assert!(result.contains("startedAt: 2026-02-25T14:30:45"));
    }

    #[test]
    fn test_update_frontmatter_overwrite_existing() {
        let content = "---\nmodel: gpt-4\n---\n\n# Issue";
        let result = update_frontmatter_content(content, &[("model", "opus")]);
        assert!(result.contains("model: opus"));
        assert!(!result.contains("gpt-4"));
    }

    #[test]
    fn test_update_frontmatter_create_new() {
        let content = "# Issue\nSome body text.";
        let result = update_frontmatter_content(content, &[("startedAt", "2026-02-25T14:30:45")]);
        assert!(result.starts_with("---\n"));
        assert!(result.contains("startedAt: 2026-02-25T14:30:45"));
        assert!(result.contains("# Issue"));
    }

    #[test]
    fn test_update_frontmatter_preserve_body() {
        let content = "---\nmodel: gpt-4\n---\n\n# Issue\n\nSome body.\n";
        let result = update_frontmatter_content(content, &[("model", "opus")]);
        assert!(result.ends_with("\n\n# Issue\n\nSome body.\n"));
    }

    #[test]
    fn test_update_frontmatter_multiple_updates() {
        let content = "---\nmodel: gpt-4\n---\n\n# Issue";
        let result = update_frontmatter_content(
            content,
            &[("startedAt", "2026-02-25T14:30:45"), ("agent", "claude"), ("iteration", "3")],
        );
        assert!(result.contains("startedAt: 2026-02-25T14:30:45"));
        assert!(result.contains("agent: claude"));
        assert!(result.contains("iteration: 3"));
    }

    #[test]
    fn test_update_frontmatter_empty_content() {
        let result = update_frontmatter_content("", &[("key", "value")]);
        assert!(result.starts_with("---\n"));
        assert!(result.contains("key: value"));
        assert!(result.contains("\n---\n"));
    }

    #[test]
    fn test_extract_extra_keys_with_extras() {
        let content =
            "---\nmodel: gpt-4\nstartedAt: 2026-02-25T14:30:45\nexitCode: 0\n---\n\n# Issue";
        let extras = extract_extra_frontmatter_keys(content);
        assert_eq!(extras.len(), 2);
        assert!(extras.iter().any(|(k, _)| k == "startedAt"));
        assert!(extras.iter().any(|(k, _)| k == "exitCode"));
        assert!(!extras.iter().any(|(k, _)| k == "model"));
    }

    #[test]
    fn test_extract_extra_keys_without_extras() {
        let content = "---\nagent: claude\nmodel: gpt-4\n---\n\n# Issue";
        let extras = extract_extra_frontmatter_keys(content);
        assert!(extras.is_empty());
    }

    #[test]
    fn test_extract_extra_keys_no_frontmatter() {
        let content = "# Issue\n\nNo frontmatter here.";
        let extras = extract_extra_frontmatter_keys(content);
        assert!(extras.is_empty());
    }

    #[test]
    fn test_iso_now_format() {
        let ts = iso_now();
        // Expected format: YYYY-MM-DDTHH:MM:SS (19 chars)
        assert_eq!(ts.len(), 19, "timestamp length: {}", ts);
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
        let separator_positions = [4usize, 7, 10, 13, 16];
        for (i, c) in ts.chars().enumerate() {
            if !separator_positions.contains(&i) {
                assert!(c.is_ascii_digit(), "Expected digit at position {i}: got '{c}' in '{ts}'");
            }
        }
    }

    #[test]
    fn test_build_prompt_with_agent_md() {
        let plans_dir = std::env::temp_dir().join("kruh_prompt_agent_md");
        let docs_dir = plans_dir.join("my-plan");
        let _ = std::fs::remove_dir_all(&plans_dir);
        std::fs::create_dir_all(&docs_dir).unwrap();

        // Create AGENT.md in plans_dir
        std::fs::write(plans_dir.join("AGENT.md"), "# Guidelines").unwrap();

        let prompt = build_prompt(
            docs_dir.to_str().unwrap(),
            plans_dir.to_str().unwrap(),
        ).unwrap();

        assert!(prompt.starts_with("First read"));
        assert!(prompt.contains("AGENT.md"));
        assert!(prompt.contains("ead")); // "...Then read INSTRUCTIONS.md..."
        assert!(prompt.contains("<done>promise</done>"));

        let _ = std::fs::remove_dir_all(&plans_dir);
    }

    #[test]
    fn test_build_prompt_without_agent_md() {
        let plans_dir = std::env::temp_dir().join("kruh_prompt_no_agent_md");
        let docs_dir = plans_dir.join("my-plan");
        let _ = std::fs::remove_dir_all(&plans_dir);
        std::fs::create_dir_all(&docs_dir).unwrap();

        // No AGENT.md
        let prompt = build_prompt(
            docs_dir.to_str().unwrap(),
            plans_dir.to_str().unwrap(),
        ).unwrap();

        assert!(prompt.starts_with("Read"));
        assert!(!prompt.contains("AGENT.md"));
        assert!(prompt.contains("<done>promise</done>"));

        let _ = std::fs::remove_dir_all(&plans_dir);
    }
}
