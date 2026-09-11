//! Problems a discovery or read reports without failing.
//!
//! Shared by the harness views that inspect things on disk (OpenSpec roots,
//! knowledge stores): a broken registry entry or a missing checkout is shown
//! next to what still works rather than failing the whole listing.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// One problem, with a stable machine-readable `code`.
///
/// Each subsystem owns its codes. OpenSpec's reuse the CLI's own, so what okena
/// reports reads the same as `openspec doctor`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    /// A concrete next step, often a pasteable command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Diagnostic {
    pub fn error(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: code.to_string(),
            message: message.into(),
            fix: None,
        }
    }

    pub fn warning(code: &str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: code.to_string(),
            message: message.into(),
            fix: None,
        }
    }

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_is_unchanged_from_the_spec_diagnostic_it_replaced() {
        // Older clients decode this as `SpecDiagnostic`; the move must not
        // change a byte of it.
        let plain = serde_json::to_value(Diagnostic::warning("unknown_store", "m")).expect("json");
        assert_eq!(
            plain,
            serde_json::json!({ "severity": "warning", "code": "unknown_store", "message": "m" })
        );
        let fixed =
            serde_json::to_value(Diagnostic::error("e", "m").with_fix("do it")).expect("json");
        assert_eq!(
            fixed,
            serde_json::json!({ "severity": "error", "code": "e", "message": "m", "fix": "do it" })
        );
    }
}
