//! Git diff parsing and execution.
//!
//! Provides structures and functions for parsing unified diff output
//! and executing git diff commands.

use std::path::Path;
use std::process::Command;

/// Type of a diff line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffLineType {
    /// Context line (unchanged).
    Context,
    /// Added line.
    Added,
    /// Removed line.
    Removed,
    /// Hunk header line (@@).
    Header,
}

/// A single line in a diff.
#[derive(Clone, Debug)]
pub struct DiffLine {
    /// Type of this line.
    pub line_type: DiffLineType,
    /// Content of the line (without +/- prefix).
    pub content: String,
    /// Line number in the old file (None for added lines).
    pub old_line_num: Option<usize>,
    /// Line number in the new file (None for removed lines).
    pub new_line_num: Option<usize>,
}

/// A hunk in a diff (section of changes).
#[derive(Clone, Debug)]
pub struct DiffHunk {
    /// The hunk header (e.g., "@@ -10,5 +10,7 @@ fn example()").
    #[allow(dead_code)]
    pub header: String,
    /// Starting line number in old file.
    #[allow(dead_code)]
    pub old_start: usize,
    /// Starting line number in new file.
    #[allow(dead_code)]
    pub new_start: usize,
    /// Lines in this hunk.
    pub lines: Vec<DiffLine>,
}

/// Diff for a single file.
#[derive(Clone, Debug)]
pub struct FileDiff {
    /// Old file path (None for new files).
    pub old_path: Option<String>,
    /// New file path (None for deleted files).
    pub new_path: Option<String>,
    /// Hunks in this file.
    pub hunks: Vec<DiffHunk>,
    /// Whether this is a binary file.
    pub is_binary: bool,
    /// Number of lines added.
    pub lines_added: usize,
    /// Number of lines removed.
    pub lines_removed: usize,
}

impl FileDiff {
    /// Get the display name for this file.
    pub fn display_name(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .unwrap_or("unknown")
    }

    /// Get just the filename without path.
    #[allow(dead_code)]
    pub fn filename(&self) -> &str {
        let path = self.display_name();
        path.rsplit('/').next().unwrap_or(path)
    }
}

/// Mode for git diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DiffMode {
    /// Unstaged changes (working tree vs index).
    #[default]
    WorkingTree,
    /// Staged changes (index vs HEAD).
    Staged,
}

impl DiffMode {
    /// Get the display name for this mode.
    pub fn display_name(&self) -> &'static str {
        match self {
            DiffMode::WorkingTree => "Unstaged",
            DiffMode::Staged => "Staged",
        }
    }

    /// Toggle to the other mode.
    pub fn toggle(&self) -> Self {
        match self {
            DiffMode::WorkingTree => DiffMode::Staged,
            DiffMode::Staged => DiffMode::WorkingTree,
        }
    }
}

/// Result of a diff operation.
#[derive(Clone, Debug, Default)]
pub struct DiffResult {
    /// Files with changes.
    pub files: Vec<FileDiff>,
}

impl DiffResult {
    /// Check if the diff is empty.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get total lines added across all files.
    #[allow(dead_code)]
    pub fn total_added(&self) -> usize {
        self.files.iter().map(|f| f.lines_added).sum()
    }

    /// Get total lines removed across all files.
    #[allow(dead_code)]
    pub fn total_removed(&self) -> usize {
        self.files.iter().map(|f| f.lines_removed).sum()
    }
}

/// Parse a unified diff output into structured form.
pub fn parse_unified_diff(output: &str) -> DiffResult {
    let mut files = Vec::new();
    let mut current_file: Option<FileDiff> = None;
    let mut current_hunk: Option<DiffHunk> = None;
    let mut old_line = 0usize;
    let mut new_line = 0usize;

    for line in output.lines() {
        // Check for diff header (new file)
        if line.starts_with("diff --git ") {
            // Save previous file
            if let Some(mut file) = current_file.take() {
                if let Some(hunk) = current_hunk.take() {
                    file.hunks.push(hunk);
                }
                files.push(file);
            }

            // Start new file
            current_file = Some(FileDiff {
                old_path: None,
                new_path: None,
                hunks: Vec::new(),
                is_binary: false,
                lines_added: 0,
                lines_removed: 0,
            });
            continue;
        }

        // Skip if no current file
        let file = match current_file.as_mut() {
            Some(f) => f,
            None => continue,
        };

        // Parse old file path
        if line.starts_with("--- ") {
            let path = line.strip_prefix("--- ").unwrap_or("");
            if path != "/dev/null" {
                // Strip "a/" prefix if present
                let path = path.strip_prefix("a/").unwrap_or(path);
                file.old_path = Some(path.to_string());
            }
            continue;
        }

        // Parse new file path
        if line.starts_with("+++ ") {
            let path = line.strip_prefix("+++ ").unwrap_or("");
            if path != "/dev/null" {
                // Strip "b/" prefix if present
                let path = path.strip_prefix("b/").unwrap_or(path);
                file.new_path = Some(path.to_string());
            }
            continue;
        }

        // Check for binary file
        // Git outputs "Binary files a/path and b/path differ" for binary files
        if line.starts_with("Binary files ") && line.ends_with(" differ") {
            file.is_binary = true;
            continue;
        }

        // Parse hunk header
        if line.starts_with("@@ ") {
            // Save previous hunk
            if let Some(hunk) = current_hunk.take() {
                file.hunks.push(hunk);
            }

            // Parse hunk header: @@ -old_start,old_count +new_start,new_count @@ context
            let (old_start, new_start) = parse_hunk_header(line);
            old_line = old_start;
            new_line = new_start;

            current_hunk = Some(DiffHunk {
                header: line.to_string(),
                old_start,
                new_start,
                lines: vec![DiffLine {
                    line_type: DiffLineType::Header,
                    content: line.to_string(),
                    old_line_num: None,
                    new_line_num: None,
                }],
            });
            continue;
        }

        // Skip if no current hunk
        let hunk = match current_hunk.as_mut() {
            Some(h) => h,
            None => continue,
        };

        // Parse diff lines
        if let Some(content) = line.strip_prefix('+') {
            // Added line
            hunk.lines.push(DiffLine {
                line_type: DiffLineType::Added,
                content: content.to_string(),
                old_line_num: None,
                new_line_num: Some(new_line),
            });
            file.lines_added += 1;
            new_line += 1;
        } else if let Some(content) = line.strip_prefix('-') {
            // Removed line
            hunk.lines.push(DiffLine {
                line_type: DiffLineType::Removed,
                content: content.to_string(),
                old_line_num: Some(old_line),
                new_line_num: None,
            });
            file.lines_removed += 1;
            old_line += 1;
        } else if let Some(content) = line.strip_prefix(' ') {
            // Context line
            hunk.lines.push(DiffLine {
                line_type: DiffLineType::Context,
                content: content.to_string(),
                old_line_num: Some(old_line),
                new_line_num: Some(new_line),
            });
            old_line += 1;
            new_line += 1;
        } else if line.is_empty() {
            // Empty context line
            hunk.lines.push(DiffLine {
                line_type: DiffLineType::Context,
                content: String::new(),
                old_line_num: Some(old_line),
                new_line_num: Some(new_line),
            });
            old_line += 1;
            new_line += 1;
        }
        // Skip other lines (e.g., "\ No newline at end of file")
    }

    // Save last file and hunk
    if let Some(mut file) = current_file {
        if let Some(hunk) = current_hunk {
            file.hunks.push(hunk);
        }
        files.push(file);
    }

    DiffResult { files }
}

/// Parse hunk header to extract old and new starting line numbers.
fn parse_hunk_header(header: &str) -> (usize, usize) {
    // Format: @@ -old_start,old_count +new_start,new_count @@ context
    // or: @@ -old_start +new_start @@ context (count of 1 is implicit)
    let mut old_start = 1;
    let mut new_start = 1;

    // Find the range part between @@ markers
    if let Some(range_part) = header
        .strip_prefix("@@ ")
        .and_then(|s| s.split(" @@").next())
    {
        let parts: Vec<&str> = range_part.split_whitespace().collect();
        for part in parts {
            if let Some(old) = part.strip_prefix('-') {
                // Parse "-old_start,old_count" or "-old_start"
                let num = old.split(',').next().unwrap_or("1");
                old_start = num.parse().unwrap_or(1);
            } else if let Some(new) = part.strip_prefix('+') {
                // Parse "+new_start,new_count" or "+new_start"
                let num = new.split(',').next().unwrap_or("1");
                new_start = num.parse().unwrap_or(1);
            }
        }
    }

    (old_start, new_start)
}

/// Get diff for a repository path.
#[allow(dead_code)]
pub fn get_diff(path: &Path, mode: DiffMode) -> Result<DiffResult, String> {
    get_diff_with_options(path, mode, false)
}

/// Get diff for a repository path with options.
pub fn get_diff_with_options(
    path: &Path,
    mode: DiffMode,
    ignore_whitespace: bool,
) -> Result<DiffResult, String> {
    let path_str = path.to_str().ok_or("Invalid path")?;

    // Build git diff command based on mode
    // WorkingTree: unstaged changes (working tree vs index)
    // Staged: staged changes (index vs HEAD)
    let mut args = match mode {
        DiffMode::WorkingTree => vec!["-C", path_str, "diff"],
        DiffMode::Staged => vec!["-C", path_str, "diff", "--cached"],
    };

    // Add -w flag to ignore whitespace changes
    if ignore_whitespace {
        args.push("-w");
    }

    let output = Command::new("git")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to execute git: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Empty diff is not an error
        if stderr.is_empty() || stderr.contains("Not a git repository") {
            return Err(stderr.trim().to_string());
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut result = parse_unified_diff(&stdout);

    // For unstaged mode, also include untracked files
    if mode == DiffMode::WorkingTree {
        let untracked = get_untracked_files(path);
        for file_path in untracked {
            if let Some(file_diff) = create_untracked_file_diff(path, &file_path) {
                result.files.push(file_diff);
            }
        }
    }

    Ok(result)
}

/// Get list of untracked files in a repository.
fn get_untracked_files(path: &Path) -> Vec<String> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return vec![],
    };

    let output = Command::new("git")
        .args(["-C", path_str, "ls-files", "--others", "--exclude-standard"])
        .output()
        .ok();

    match output {
        Some(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        }
        _ => vec![],
    }
}

/// Create a FileDiff for an untracked file (shows entire file as added).
fn create_untracked_file_diff(repo_path: &Path, file_path: &str) -> Option<FileDiff> {
    let full_path = repo_path.join(file_path);

    // Check if it's a binary file (simple heuristic)
    let content = match std::fs::read(&full_path) {
        Ok(bytes) => {
            // Check for binary content (null bytes in first 8KB)
            if bytes.iter().take(8192).any(|&b| b == 0) {
                return Some(FileDiff {
                    old_path: None,
                    new_path: Some(file_path.to_string()),
                    hunks: vec![],
                    is_binary: true,
                    lines_added: 0,
                    lines_removed: 0,
                });
            }
            String::from_utf8_lossy(&bytes).to_string()
        }
        Err(_) => return None,
    };

    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();

    // Create a single hunk with all lines as added
    let diff_lines: Vec<DiffLine> = lines
        .into_iter()
        .enumerate()
        .map(|(i, line)| DiffLine {
            line_type: DiffLineType::Added,
            content: line.to_string(),
            old_line_num: None,
            new_line_num: Some(i + 1),
        })
        .collect();

    let hunk = DiffHunk {
        header: format!("@@ -0,0 +1,{} @@ (new file)", line_count),
        old_start: 0,
        new_start: 1,
        lines: vec![DiffLine {
            line_type: DiffLineType::Header,
            content: format!("@@ -0,0 +1,{} @@ (new file)", line_count),
            old_line_num: None,
            new_line_num: None,
        }]
        .into_iter()
        .chain(diff_lines)
        .collect(),
    };

    Some(FileDiff {
        old_path: None,
        new_path: Some(file_path.to_string()),
        hunks: vec![hunk],
        is_binary: false,
        lines_added: line_count,
        lines_removed: 0,
    })
}

/// Check if a path is inside a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return false,
    };

    Command::new("git")
        .args(["-C", path_str, "rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Get the full content of a file from git at a specific revision.
///
/// - `revision` can be "HEAD", a commit hash, or empty for the index (staged version)
pub fn get_file_from_git(repo_path: &Path, revision: &str, file_path: &str) -> Option<String> {
    let repo_str = repo_path.to_str()?;

    // Format: revision:path (e.g., "HEAD:src/main.rs")
    // For index, use ":0:path" syntax (stage 0 = normal index entry)
    let object = if revision.is_empty() {
        format!(":0:{}", file_path)
    } else {
        format!("{}:{}", revision, file_path)
    };

    let output = Command::new("git")
        .args(["-C", repo_str, "show", &object])
        .output()
        .ok()?;

    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

/// Get the full content of a file from the working tree (filesystem).
pub fn get_file_from_working_tree(repo_path: &Path, file_path: &str) -> Option<String> {
    let full_path = repo_path.join(file_path);
    std::fs::read_to_string(full_path).ok()
}

/// Get the "old" and "new" file content for a file diff based on the diff mode.
///
/// Returns (old_content, new_content).
/// - For WorkingTree mode: old = HEAD (or index), new = working tree
/// - For Staged mode: old = HEAD, new = index
pub fn get_file_contents_for_diff(
    repo_path: &Path,
    file_path: &str,
    mode: DiffMode,
) -> (Option<String>, Option<String>) {
    match mode {
        DiffMode::WorkingTree => {
            // Unstaged: comparing index vs working tree
            // Try index first, fall back to HEAD (they're equal if nothing staged)
            let old = get_file_from_git(repo_path, "", file_path)
                .or_else(|| get_file_from_git(repo_path, "HEAD", file_path));
            let new = get_file_from_working_tree(repo_path, file_path);
            (old, new)
        }
        DiffMode::Staged => {
            // Staged: comparing HEAD vs index
            let old = get_file_from_git(repo_path, "HEAD", file_path);
            let new = get_file_from_git(repo_path, "", file_path)
                .or_else(|| get_file_from_working_tree(repo_path, file_path));
            (old, new)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hunk_header() {
        assert_eq!(parse_hunk_header("@@ -1,5 +1,7 @@ fn main()"), (1, 1));
        assert_eq!(parse_hunk_header("@@ -10,3 +15,5 @@"), (10, 15));
        assert_eq!(parse_hunk_header("@@ -1 +1 @@"), (1, 1));
        assert_eq!(parse_hunk_header("@@ -100,20 +95,15 @@ impl Foo"), (100, 95));
    }

    #[test]
    fn test_parse_unified_diff() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("Hello");
     println!("World");
 }
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].new_path, Some("src/main.rs".to_string()));
        assert_eq!(result.files[0].lines_added, 1);
        assert_eq!(result.files[0].lines_removed, 0);
        assert_eq!(result.files[0].hunks.len(), 1);
        assert_eq!(result.files[0].hunks[0].lines.len(), 5); // header + 4 lines
    }

    #[test]
    fn test_parse_new_file() {
        let diff = r#"diff --git a/new_file.txt b/new_file.txt
--- /dev/null
+++ b/new_file.txt
@@ -0,0 +1,2 @@
+line 1
+line 2
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert!(result.files[0].old_path.is_none());
        assert_eq!(result.files[0].new_path, Some("new_file.txt".to_string()));
        assert_eq!(result.files[0].lines_added, 2);
    }

    #[test]
    fn test_parse_deleted_file() {
        let diff = r#"diff --git a/deleted.txt b/deleted.txt
--- a/deleted.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-line 1
-line 2
"#;
        let result = parse_unified_diff(diff);
        assert_eq!(result.files.len(), 1);
        assert_eq!(result.files[0].old_path, Some("deleted.txt".to_string()));
        assert!(result.files[0].new_path.is_none());
        assert_eq!(result.files[0].lines_removed, 2);
    }

    #[test]
    fn test_diff_mode_toggle() {
        assert_eq!(DiffMode::WorkingTree.toggle(), DiffMode::Staged);
        assert_eq!(DiffMode::Staged.toggle(), DiffMode::WorkingTree);
    }
}
