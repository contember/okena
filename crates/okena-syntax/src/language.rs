use serde::{Deserialize, Serialize};
use std::path::Path;

/// Languages with structured-review support.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyntaxLanguage {
    #[serde(rename = "rust")]
    Rust,
    #[serde(rename = "typescript")]
    TypeScript,
    #[serde(rename = "tsx")]
    Tsx,
}

impl SyntaxLanguage {
    /// Detect a supported language from a source path.
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::from_extension(extension)
    }

    /// Detect a supported language from a file extension, with or without a dot.
    pub fn from_extension(extension: &str) -> Option<Self> {
        let extension = extension.strip_prefix('.').unwrap_or(extension);
        if extension.eq_ignore_ascii_case("rs") {
            Some(Self::Rust)
        } else if ["ts", "mts", "cts"]
            .iter()
            .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        {
            Some(Self::TypeScript)
        } else if extension.eq_ignore_ascii_case("tsx") {
            Some(Self::Tsx)
        } else {
            None
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SyntaxLanguage;
    use std::path::Path;

    #[test]
    fn detects_supported_extensions() {
        assert_eq!(
            SyntaxLanguage::from_path(Path::new("src/lib.rs")),
            Some(SyntaxLanguage::Rust)
        );
        assert_eq!(
            SyntaxLanguage::from_path(Path::new("src/types.d.ts")),
            Some(SyntaxLanguage::TypeScript)
        );
        assert_eq!(
            SyntaxLanguage::from_path(Path::new("src/module.MTS")),
            Some(SyntaxLanguage::TypeScript)
        );
        assert_eq!(
            SyntaxLanguage::from_path(Path::new("src/view.TSX")),
            Some(SyntaxLanguage::Tsx)
        );
    }

    #[test]
    fn rejects_unsupported_and_non_utf8_free_paths() {
        assert_eq!(SyntaxLanguage::from_path(Path::new("README.md")), None);
        assert_eq!(SyntaxLanguage::from_path(Path::new("Makefile")), None);
        assert_eq!(SyntaxLanguage::from_extension(".jsx"), None);
    }

    #[test]
    fn language_serde_uses_stable_wire_names() {
        let json = serde_json::to_string(&SyntaxLanguage::TypeScript).unwrap();
        assert_eq!(json, "\"typescript\"");
        assert_eq!(
            serde_json::from_str::<SyntaxLanguage>(&json).unwrap(),
            SyntaxLanguage::TypeScript
        );
    }
}
