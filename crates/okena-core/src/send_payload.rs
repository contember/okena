//! Shared types for "Send to Terminal" — text or code-block payloads emitted by
//! viewers (file viewer, diff viewer, commit graph) and consumed by the host
//! that owns the active terminal.
//!
//! Lives in `okena-core` so both the producers (`okena-files`, `okena-views-git`)
//! and the broker queue type (`okena-workspace`) can refer to it without forming
//! a dependency cycle.

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

/// A "Send to Terminal" payload.
///
/// - `Code` blocks know their absolute paths so the dispatcher can present them
///   relative to the terminal's CWD.
/// - `Text` is pre-formatted (used for commit info).
/// - `Path` is a file/directory reference; the dispatcher writes it relative to
///   the terminal's CWD and appends a trailing space so the user can type a
///   command after.
#[derive(Clone, Debug)]
pub enum SendPayload {
    Code(Vec<CodeBlock>),
    Text(String),
    Path(PathBuf),
}

impl SendPayload {
    pub fn is_empty(&self) -> bool {
        match self {
            SendPayload::Code(blocks) => blocks.is_empty(),
            SendPayload::Text(s) => s.is_empty(),
            SendPayload::Path(p) => p.as_os_str().is_empty(),
        }
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
        let mut out = match self {
            SendPayload::Code(blocks) => {
                let rendered: Vec<String> = blocks
                    .iter()
                    .map(|b| format_code_block(b, terminal_cwd))
                    .collect();
                rendered.join("\n\n")
            }
            SendPayload::Text(s) => s.clone(),
            SendPayload::Path(p) => format!("{} ", relative_to_cwd(p, terminal_cwd)),
        };
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
    format!("{}\n```{}\n{}\n```", header, lang, block.text)
}

/// If `path` lives under `cwd`, return the path component relative to it
/// (with a leading `./` so it's unambiguously a path even when it has no
/// directory). Otherwise return the absolute path as-is.
fn relative_to_cwd(path: &Path, cwd: Option<&Path>) -> String {
    if let Some(cwd) = cwd {
        if let Ok(rel) = path.strip_prefix(cwd) {
            let rel_str = rel.to_string_lossy();
            if rel_str.is_empty() {
                return ".".into();
            }
            return format!("./{}", rel_str);
        }
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
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
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

    #[test]
    fn single_block_with_no_cwd_uses_absolute_path() {
        let p = SendPayload::Code(vec![block("/proj/src/foo.rs", 5, 5, "let x = 1;")]);
        let out = p.format(None);
        assert_eq!(out, "/proj/src/foo.rs:5\n```rust\nlet x = 1;\n```");
    }

    #[test]
    fn single_block_uses_cwd_relative_path() {
        let p = SendPayload::Code(vec![block("/proj/src/foo.rs", 5, 7, "a\nb\nc")]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(out, "./src/foo.rs:5-7\n```rust\na\nb\nc\n```");
    }

    #[test]
    fn block_outside_cwd_falls_back_to_absolute() {
        let p = SendPayload::Code(vec![block("/other/src/foo.rs", 1, 1, "x")]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(out, "/other/src/foo.rs:1\n```rust\nx\n```");
    }

    #[test]
    fn multiple_blocks_joined_with_blank_line() {
        let p = SendPayload::Code(vec![
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
        let p = SendPayload::Code(vec![block("/proj/notes.xyz", 1, 1, "hello")]);
        let out = p.format(Some(Path::new("/proj")));
        assert_eq!(out, "./notes.xyz:1\n```\nhello\n```");
    }

    #[test]
    fn special_filenames_get_language_hint() {
        assert_eq!(markdown_lang_hint(Path::new("Makefile")), "make");
        assert_eq!(markdown_lang_hint(Path::new("/proj/Dockerfile")), "dockerfile");
        assert_eq!(markdown_lang_hint(Path::new("CMakeLists.txt")), "cmake");
        assert_eq!(markdown_lang_hint(Path::new("Cargo.toml")), "toml");
    }

    #[test]
    fn text_variant_is_passthrough_minus_trailing_lf() {
        let p = SendPayload::Text("commit abc\n\n    subject\n".into());
        assert_eq!(p.format(None), "commit abc\n\n    subject");
    }

    #[test]
    fn never_ends_with_newline_anywhere() {
        let cases: Vec<SendPayload> = vec![
            SendPayload::Text("trailing\n\n\n".into()),
            SendPayload::Code(vec![block("/p/a.rs", 1, 1, "x\n")]),
            SendPayload::Code(vec![]),
        ];
        for p in cases {
            assert!(!p.format(None).ends_with('\n'), "{:?}", p);
        }
    }

    #[test]
    fn empty_code_payload_is_empty_string() {
        let p = SendPayload::Code(vec![]);
        assert_eq!(p.format(None), "");
        assert!(p.is_empty());
    }

    #[test]
    fn path_variant_resolves_cwd_relative_with_trailing_space() {
        let p = SendPayload::Path(PathBuf::from("/proj/src/foo.rs"));
        assert_eq!(p.format(Some(Path::new("/proj"))), "./src/foo.rs ");
        assert_eq!(p.format(None), "/proj/src/foo.rs ");
    }
}
