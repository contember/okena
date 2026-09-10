use serde::{Deserialize, Serialize};

/// A language this crate can extract declarations from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxLanguage {
    Rust,
    TypeScript,
    Tsx,
}

impl SyntaxLanguage {
    /// The language of a repository-relative path, or `None` when unsupported.
    pub fn from_path(path: &str) -> Option<Self> {
        let extension = path.rsplit_once('.').map(|(_, extension)| extension)?;
        match extension.to_ascii_lowercase().as_str() {
            "rs" => Some(Self::Rust),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            _ => None,
        }
    }

    /// Display name, as it reaches the screen.
    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_map_to_languages() {
        assert_eq!(
            SyntaxLanguage::from_path("src/main.rs"),
            Some(SyntaxLanguage::Rust)
        );
        assert_eq!(
            SyntaxLanguage::from_path("web/src/app.ts"),
            Some(SyntaxLanguage::TypeScript)
        );
        assert_eq!(
            SyntaxLanguage::from_path("web/src/App.tsx"),
            Some(SyntaxLanguage::Tsx)
        );
    }

    #[test]
    fn unsupported_and_extensionless_paths_have_no_language() {
        assert_eq!(SyntaxLanguage::from_path("README.md"), None);
        assert_eq!(SyntaxLanguage::from_path("Makefile"), None);
        assert_eq!(SyntaxLanguage::from_path("scripts/build"), None);
    }

    #[test]
    fn extension_matching_ignores_case() {
        assert_eq!(
            SyntaxLanguage::from_path("src/Main.RS"),
            Some(SyntaxLanguage::Rust)
        );
    }
}
