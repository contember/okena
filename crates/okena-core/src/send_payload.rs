//! Shared types for "Send to Terminal" — text, code-block or terminal-output
//! payloads emitted by viewers (file viewer, diff viewer, commit graph) and by
//! terminal panes, and consumed by the host that owns the target terminal.
//!
//! Lives in `okena-core` so both the producers (`okena-files`, `okena-views-git`,
//! `okena-views-terminal`) and the broker queue type (`okena-workspace`) can
//! refer to it without forming a dependency cycle.

use std::path::{Path, PathBuf};

/// One file's contribution to a code payload: a path-with-range header followed
/// by a fenced code block of the selected lines.
#[derive(Clone, Debug)]
pub struct CodeBlock {
    /// Absolute path to the source file. The dispatcher rewrites it relative
    /// to the receiving terminal's CWD before formatting.
    pub absolute_path: PathBuf,
    pub first: usize,
    pub last: usize,
    pub text: String,
}

/// A slice of terminal output: fenced verbatim, with no path to resolve.
#[derive(Clone, Debug)]
pub struct OutputBlock {
    pub text: String,
    /// Terminal name shown above the fence. `None` when the target terminal is
    /// also the source — naming it there would just be noise.
    pub source_label: Option<String>,
}

/// One part of a payload.
///
/// - `Code` blocks know their absolute paths so the dispatcher can present them
///   relative to the terminal's CWD.
/// - `Output` is terminal output, fenced as-is.
/// - `Text` is pre-formatted (used for commit info).
/// - `Path` is a file/directory reference; the dispatcher writes it relative to
///   the terminal's CWD and appends a trailing space so the user can type a
///   command after.
#[derive(Clone, Debug)]
pub enum SendChunk {
    Code(CodeBlock),
    Output(OutputBlock),
    Text(String),
    Path(PathBuf),
}

impl SendChunk {
    fn is_empty(&self) -> bool {
        match self {
            SendChunk::Code(b) => b.text.is_empty(),
            SendChunk::Output(b) => b.text.is_empty(),
            SendChunk::Text(s) => s.is_empty(),
            SendChunk::Path(p) => p.as_os_str().is_empty(),
        }
    }

    fn format(&self, terminal_cwd: Option<&Path>) -> String {
        match self {
            SendChunk::Code(b) => format_code_block(b, terminal_cwd),
            SendChunk::Output(b) => format_output_block(b),
            SendChunk::Text(s) => s.clone(),
            SendChunk::Path(p) => format!("{} ", relative_to_cwd(p, terminal_cwd)),
        }
    }
}

/// A "Send to Terminal" payload: quoted chunks plus an optional user note.
///
/// The note is rendered *after* the chunks — receivers follow an instruction
/// better when it trails the material it refers to.
#[derive(Clone, Debug, Default)]
pub struct SendPayload {
    pub chunks: Vec<SendChunk>,
    pub note: Option<String>,
}

impl SendPayload {
    pub fn code(blocks: Vec<CodeBlock>) -> Self {
        Self {
            chunks: blocks.into_iter().map(SendChunk::Code).collect(),
            note: None,
        }
    }

    pub fn output(block: OutputBlock) -> Self {
        Self {
            chunks: vec![SendChunk::Output(block)],
            note: None,
        }
    }

    pub fn text(text: String) -> Self {
        Self {
            chunks: vec![SendChunk::Text(text)],
            note: None,
        }
    }

    pub fn path(path: PathBuf) -> Self {
        Self {
            chunks: vec![SendChunk::Path(path)],
            note: None,
        }
    }

    /// Attach a user note. Blank notes are dropped so callers can pass an
    /// unedited input field straight through.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        let note = note.into();
        let trimmed = note.trim();
        self.note = (!trimmed.is_empty()).then(|| trimmed.to_string());
        self
    }

    pub fn is_empty(&self) -> bool {
        self.note.is_none() && self.chunks.iter().all(SendChunk::is_empty)
    }

    /// Render the payload into the bytes to paste into the terminal.
    ///
    /// `terminal_cwd` (if known) is used to express each `CodeBlock`'s or
    /// `Path` value relative to where the user's shell is sitting, so
    /// `cat path:5-7` style references work without copy-pasting from
    /// elsewhere. Falls back to the absolute path when no CWD is available
    /// or the file isn't under it.
    ///
    /// Trailing newlines are stripped: receivers like Claude/Codex TUIs treat a
    /// trailing LF inside a bracketed paste as Enter and submit the prompt.
    /// This is the single home for that invariant — callers don't repeat it.
    pub fn format(&self, terminal_cwd: Option<&Path>) -> String {
        let mut parts: Vec<String> = self.chunks.iter().map(|c| c.format(terminal_cwd)).collect();
        if let Some(ref note) = self.note {
            parts.push(note.clone());
        }
        let mut out = parts.join("\n\n");
        while out.ends_with('\n') {
            out.pop();
        }
        out
    }
}

fn format_code_block(block: &CodeBlock, terminal_cwd: Option<&Path>) -> String {
    let display_path = relative_to_cwd(&block.absolute_path, terminal_cwd);
    let lang = markdown_lang_hint(&block.absolute_path);
    let header = if block.first == block.last {
        format!("{}:{}", display_path, block.first)
    } else {
        format!("{}:{}-{}", display_path, block.first, block.last)
    };
    let fence = fence_for(&block.text);
    format!("{}\n{}{}\n{}\n{}", header, fence, lang, block.text, fence)
}

fn format_output_block(block: &OutputBlock) -> String {
    let fence = fence_for(&block.text);
    match block.source_label {
        Some(ref label) => format!("{}:\n{}\n{}\n{}", label, fence, block.text, fence),
        None => format!("{}\n{}\n{}", fence, block.text, fence),
    }
}

/// A fence long enough to survive the content it wraps.
///
/// Agent output routinely contains ``` fences of its own; a fixed 3-backtick
/// wrapper would end the quote early and mangle everything after it.
fn fence_for(text: &str) -> String {
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    "`".repeat(longest_run.max(2) + 1)
}

/// If `path` lives under `cwd`, return the path component relative to it
/// (with a leading `./` so it's unambiguously a path even when it has no
/// directory). Otherwise return the absolute path as-is.
fn relative_to_cwd(path: &Path, cwd: Option<&Path>) -> String {
    if let Some(cwd) = cwd
        && let Ok(rel) = path.strip_prefix(cwd)
    {
        let rel_str = rel.to_string_lossy();
        if rel_str.is_empty() {
            return ".".into();
        }
        return format!("./{}", rel_str);
    }
    path.to_string_lossy().into_owned()
}

/// Best-effort language hint for a Markdown code fence.
///
/// Tries the file name first (so `Makefile`, `Dockerfile`, `CMakeLists.txt`
/// get a useful hint), then falls back to the extension. Returns an empty
/// string when no useful hint applies — yields a bare ```` ``` ```` fence.
pub fn markdown_lang_hint(path: &Path) -> &'static str {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        match name {
            "Makefile" | "makefile" | "GNUmakefile" => return "make",
            "Dockerfile" | "Containerfile" => return "dockerfile",
            "CMakeLists.txt" => return "cmake",
            "Cargo.toml" | "Cargo.lock" => return "toml",
            "package.json" | "tsconfig.json" | "deno.json" => return "json",
            _ => {}
        }
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "tsx",
        "js" => "javascript",
        "jsx" => "jsx",
        "py" => "python",
        "go" => "go",
        "rb" => "ruby",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "sh" | "bash" | "zsh" => "bash",
        "fish" => "fish",
        "ps1" | "psm1" | "psd1" => "powershell",
        "sql" => "sql",
        "json" | "jsonc" | "json5" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" | "sass" => "scss",
        "xml" => "xml",
        "lua" => "lua",
        "ex" | "exs" => "elixir",
        "elm" => "elm",
        "hs" => "haskell",
        "ml" | "mli" => "ocaml",
        "scala" => "scala",
        "dart" => "dart",
        "vue" => "vue",
        "svelte" => "svelte",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(path: &str, first: usize, last: usize, text: &str) -> CodeBlock {
        CodeBlock {
            absolute_path: PathBuf::from(path),
            first,
            last,
            text: text.into(),
        }
    }

    fn output(text: &str, label: Option<&str>) -> OutputBlock {
        OutputBlock {
            text: text.into(),
            source_label: label.map(Into::into),
        }
    }

    #[test]
    fn single_block_with_no_cwd_uses_absolute_path() {
        let p = SendPayload::code(vec![block("/proj/src/foo.rs", 5, 5, "let x = 1;")]);
        let out = p.format(None);
        assert_eq!(out, "/proj/src/foo.rs:5\n```rust\nlet x = 1;\n```");
    }

    #[test]
    fn single_block_uses_cwd_relative_path() {
        let p = SendPayload::code(vec![block("/proj/src/foo.rs", 5, 7, "a\nb\nc")]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(out, "./src/foo.rs:5-7\n```rust\na\nb\nc\n```");
    }

    #[test]
    fn block_outside_cwd_falls_back_to_absolute() {
        let p = SendPayload::code(vec![block("/other/src/foo.rs", 1, 1, "x")]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(out, "/other/src/foo.rs:1\n```rust\nx\n```");
    }

    #[test]
    fn multiple_blocks_joined_with_blank_line() {
        let p = SendPayload::code(vec![
            block("/proj/a.rs", 1, 1, "x"),
            block("/proj/b.rs", 2, 3, "y\nz"),
        ]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(
            out,
            "./a.rs:1\n```rust\nx\n```\n\n./b.rs:2-3\n```rust\ny\nz\n```"
        );
    }

    #[test]
    fn unknown_extension_yields_empty_lang_label() {
        let p = SendPayload::code(vec![block("/proj/notes.xyz", 1, 1, "hello")]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(out, "./notes.xyz:1\n```\nhello\n```");
    }

    #[test]
    fn output_block_is_fenced_without_a_header() {
        let p = SendPayload::output(output("error: boom\n  at line 3", None));
        assert_eq!(p.format(None), "```\nerror: boom\n  at line 3\n```");
    }

    #[test]
    fn output_block_with_label_names_the_source_terminal() {
        let p = SendPayload::output(output("boom", Some("dev server")));
        assert_eq!(p.format(None), "dev server:\n```\nboom\n```");
    }

    #[test]
    fn note_follows_the_quoted_chunks() {
        let p = SendPayload::output(output("boom", None)).with_note("why?");
        assert_eq!(p.format(None), "```\nboom\n```\n\nwhy?");
    }

    #[test]
    fn blank_note_is_dropped() {
        let p = SendPayload::output(output("boom", None)).with_note("  \n ");
        assert_eq!(p.format(None), "```\nboom\n```");
        assert!(p.note.is_none());
    }

    #[test]
    fn note_is_trimmed() {
        let p = SendPayload::output(output("boom", None)).with_note("  fix it\n");
        assert_eq!(p.format(None), "```\nboom\n```\n\nfix it");
    }

    /// Agent output usually contains fences of its own — a 3-backtick wrapper
    /// would end the quote early and leak the rest as prose.
    #[test]
    fn fence_outgrows_backticks_in_the_quoted_text() {
        let p = SendPayload::output(output("here:\n```rust\nlet x = 1;\n```", None));
        assert_eq!(
            p.format(None),
            "````\nhere:\n```rust\nlet x = 1;\n```\n````"
        );
    }

    #[test]
    fn fence_outgrows_backticks_in_code_blocks_too() {
        let p = SendPayload::code(vec![block("/p/a.md", 1, 1, "````\nnested\n````")]);
        assert_eq!(
            p.format(None),
            "/p/a.md:1\n`````markdown\n````\nnested\n````\n`````"
        );
    }

    #[test]
    fn note_only_payload_sends_just_the_note() {
        let p = SendPayload::default().with_note("hello");
        assert_eq!(p.format(None), "hello");
        assert!(!p.is_empty());
    }

    #[test]
    fn special_filenames_get_language_hint() {
        assert_eq!(markdown_lang_hint(Path::new("Makefile")), "make");
        assert_eq!(
            markdown_lang_hint(Path::new("/proj/Dockerfile")),
            "dockerfile"
        );
        assert_eq!(markdown_lang_hint(Path::new("CMakeLists.txt")), "cmake");
        assert_eq!(markdown_lang_hint(Path::new("Cargo.toml")), "toml");
    }

    #[test]
    fn text_variant_is_passthrough_minus_trailing_lf() {
        let p = SendPayload::text("commit abc\n\n    subject\n".into());
        assert_eq!(p.format(None), "commit abc\n\n    subject");
    }

    #[test]
    fn never_ends_with_newline_anywhere() {
        let cases: Vec<SendPayload> = vec![
            SendPayload::text("trailing\n\n\n".into()),
            SendPayload::code(vec![block("/p/a.rs", 1, 1, "x\n")]),
            SendPayload::code(vec![]),
            SendPayload::output(output("boom\n", None)).with_note("why\n\n"),
        ];
        for p in cases {
            assert!(!p.format(None).ends_with('\n'), "{:?}", p);
        }
    }

    #[test]
    fn empty_code_payload_is_empty_string() {
        let p = SendPayload::code(vec![]);
        assert_eq!(p.format(None), "");
        assert!(p.is_empty());
    }

    #[test]
    fn path_variant_resolves_cwd_relative_with_trailing_space() {
        let p = SendPayload::path(PathBuf::from("/proj/src/foo.rs"));
        assert_eq!(p.format(Some(Path::new("/proj"))), "./src/foo.rs ");
        assert_eq!(p.format(None), "/proj/src/foo.rs ");
    }
}
