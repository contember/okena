use crate::SyntaxLanguage;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelError(String);

impl ModelError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ModelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ModelError {}

/// UTF-8 source range. Bytes are zero-based and end-exclusive. Lines are one-based and inclusive.
///
/// Tree-sitter rows are zero-based and its end position is exclusive. Adapters must convert an
/// end position at column zero to the preceding inclusive line when the range spans lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(try_from = "SourceRangeWire")]
pub struct SourceRange {
    start_byte: u64,
    end_byte: u64,
    start_line: NonZeroU32,
    end_line: NonZeroU32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceRangeWire {
    start_byte: u64,
    end_byte: u64,
    start_line: NonZeroU32,
    end_line: NonZeroU32,
}

impl TryFrom<SourceRangeWire> for SourceRange {
    type Error = ModelError;

    fn try_from(value: SourceRangeWire) -> Result<Self, Self::Error> {
        Self::new(
            value.start_byte,
            value.end_byte,
            value.start_line,
            value.end_line,
        )
    }
}

impl<'de> Deserialize<'de> for SourceRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SourceRangeWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl SourceRange {
    pub fn new(
        start_byte: u64,
        end_byte: u64,
        start_line: NonZeroU32,
        end_line: NonZeroU32,
    ) -> Result<Self, ModelError> {
        if start_byte > end_byte {
            return Err(ModelError::new("source range starts after it ends"));
        }
        if start_line > end_line {
            return Err(ModelError::new("source range start line exceeds end line"));
        }
        Ok(Self {
            start_byte,
            end_byte,
            start_line,
            end_line,
        })
    }

    /// Convert zero-based tree-sitter rows and its exclusive end position.
    pub fn from_tree_sitter(
        start_byte: u64,
        end_byte: u64,
        start_row: u32,
        end_row: u32,
        end_column: u32,
    ) -> Result<Self, ModelError> {
        let start_line = start_row
            .checked_add(1)
            .and_then(NonZeroU32::new)
            .ok_or_else(|| ModelError::new("tree-sitter start row exceeds wire line range"))?;
        let inclusive_end = if end_byte > start_byte && end_column == 0 && end_row > start_row {
            end_row
        } else {
            end_row
                .checked_add(1)
                .ok_or_else(|| ModelError::new("tree-sitter end row exceeds wire line range"))?
        };
        let end_line = NonZeroU32::new(inclusive_end)
            .ok_or_else(|| ModelError::new("tree-sitter range has no inclusive end line"))?;
        Self::new(start_byte, end_byte, start_line, end_line)
    }

    pub fn start_byte(self) -> u64 {
        self.start_byte
    }

    pub fn end_byte(self) -> u64 {
        self.end_byte
    }

    pub fn start_line(self) -> NonZeroU32 {
        self.start_line
    }

    pub fn end_line(self) -> NonZeroU32 {
        self.end_line
    }

    pub fn line_count(self) -> u32 {
        self.end_line.get() - self.start_line.get() + 1
    }

    pub fn contains(self, other: Self) -> bool {
        self.start_byte <= other.start_byte
            && other.end_byte <= self.end_byte
            && self.start_line <= other.start_line
            && other.end_line <= self.end_line
    }

    pub fn validate_source(self, source: &str) -> Result<(), ModelError> {
        let start = usize::try_from(self.start_byte)
            .map_err(|_| ModelError::new("source range start does not fit this platform"))?;
        let end = usize::try_from(self.end_byte)
            .map_err(|_| ModelError::new("source range end does not fit this platform"))?;
        if end > source.len() {
            return Err(ModelError::new("source range exceeds source length"));
        }
        if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            return Err(ModelError::new("source range splits a UTF-8 code point"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(try_from = "SyntaxProvenanceWire")]
pub struct SyntaxProvenance {
    language: SyntaxLanguage,
    parser: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntaxProvenanceWire {
    language: SyntaxLanguage,
    parser: String,
}

impl TryFrom<SyntaxProvenanceWire> for SyntaxProvenance {
    type Error = ModelError;

    fn try_from(value: SyntaxProvenanceWire) -> Result<Self, Self::Error> {
        Self::tree_sitter(value.language, value.parser)
    }
}

impl<'de> Deserialize<'de> for SyntaxProvenance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SyntaxProvenanceWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl SyntaxProvenance {
    pub fn tree_sitter(
        language: SyntaxLanguage,
        parser: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let parser = parser.into();
        if parser.trim().is_empty() {
            return Err(ModelError::new("syntax parser name must not be empty"));
        }
        Ok(Self { language, parser })
    }

    pub fn language(&self) -> SyntaxLanguage {
        self.language
    }

    pub fn parser(&self) -> &str {
        &self.parser
    }

    /// Owned UTF-8 payload bytes retained by this value.
    pub fn estimated_owned_bytes(&self) -> u64 {
        string_owned_bytes(&self.parser)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Module,
    Function,
    Method,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Class,
    Interface,
    TypeAlias,
    Constant,
    Static,
    Field,
    Variant,
    Macro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolVisibility {
    Public,
    Restricted,
    Private,
    Exported,
    Unknown,
}

/// Descriptive symbol key within one file. Duplicate keys are intentionally ambiguous.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(try_from = "SymbolKeyWire")]
pub struct SymbolKey {
    qualified_path: Vec<String>,
    kind: SymbolKind,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolKeyWire {
    qualified_path: Vec<String>,
    kind: SymbolKind,
    name: String,
}

impl TryFrom<SymbolKeyWire> for SymbolKey {
    type Error = ModelError;

    fn try_from(value: SymbolKeyWire) -> Result<Self, Self::Error> {
        Self::new(value.qualified_path, value.kind, value.name)
    }
}

impl<'de> Deserialize<'de> for SymbolKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SymbolKeyWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl SymbolKey {
    pub fn new(
        qualified_path: Vec<String>,
        kind: SymbolKind,
        name: impl Into<String>,
    ) -> Result<Self, ModelError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(ModelError::new("symbol name must not be empty"));
        }
        if qualified_path.iter().any(|part| part.trim().is_empty()) {
            return Err(ModelError::new(
                "symbol qualified path must not contain empty segments",
            ));
        }
        Ok(Self {
            qualified_path,
            kind,
            name,
        })
    }

    pub fn qualified_path(&self) -> &[String] {
        &self.qualified_path
    }

    pub fn kind(&self) -> SymbolKind {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn qualified_name(&self) -> String {
        self.qualified_path
            .iter()
            .chain(std::iter::once(&self.name))
            .cloned()
            .collect::<Vec<_>>()
            .join("::")
    }

    /// Owned UTF-8 payload bytes, excluding vector and allocator overhead.
    pub fn estimated_owned_bytes(&self) -> u64 {
        self.qualified_path
            .iter()
            .map(|part| string_owned_bytes(part))
            .chain(std::iter::once(string_owned_bytes(&self.name)))
            .fold(0, u64::saturating_add)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(try_from = "SymbolFactWire")]
pub struct SymbolFact {
    provenance: SyntaxProvenance,
    key: SymbolKey,
    visibility: SymbolVisibility,
    full_range: SourceRange,
    signature_range: SourceRange,
    body_range: Option<SourceRange>,
    normalized_signature: String,
    parameter_count: u32,
    syntactic_nesting_depth: u32,
    type_member_count: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SymbolFactWire {
    provenance: SyntaxProvenance,
    key: SymbolKey,
    visibility: SymbolVisibility,
    full_range: SourceRange,
    signature_range: SourceRange,
    body_range: Option<SourceRange>,
    normalized_signature: String,
    parameter_count: u32,
    syntactic_nesting_depth: u32,
    type_member_count: u32,
}

impl TryFrom<SymbolFactWire> for SymbolFact {
    type Error = ModelError;

    fn try_from(value: SymbolFactWire) -> Result<Self, Self::Error> {
        Self::new(
            value.provenance,
            value.key,
            value.visibility,
            value.full_range,
            value.signature_range,
            value.body_range,
            value.normalized_signature,
            value.parameter_count,
            value.syntactic_nesting_depth,
            value.type_member_count,
        )
    }
}

impl<'de> Deserialize<'de> for SymbolFact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SymbolFactWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl SymbolFact {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provenance: SyntaxProvenance,
        key: SymbolKey,
        visibility: SymbolVisibility,
        full_range: SourceRange,
        signature_range: SourceRange,
        body_range: Option<SourceRange>,
        normalized_signature: impl Into<String>,
        parameter_count: u32,
        syntactic_nesting_depth: u32,
        type_member_count: u32,
    ) -> Result<Self, ModelError> {
        if !full_range.contains(signature_range) {
            return Err(ModelError::new(
                "signature range must be inside symbol range",
            ));
        }
        if body_range.is_some_and(|body| !full_range.contains(body)) {
            return Err(ModelError::new("body range must be inside symbol range"));
        }
        let normalized_signature = normalized_signature.into();
        if normalized_signature.trim().is_empty() {
            return Err(ModelError::new("normalized signature must not be empty"));
        }
        Ok(Self {
            provenance,
            key,
            visibility,
            full_range,
            signature_range,
            body_range,
            normalized_signature,
            parameter_count,
            syntactic_nesting_depth,
            type_member_count,
        })
    }

    pub fn provenance(&self) -> &SyntaxProvenance {
        &self.provenance
    }
    pub fn key(&self) -> &SymbolKey {
        &self.key
    }
    pub fn visibility(&self) -> SymbolVisibility {
        self.visibility
    }
    pub fn full_range(&self) -> SourceRange {
        self.full_range
    }
    pub fn signature_range(&self) -> SourceRange {
        self.signature_range
    }
    pub fn body_range(&self) -> Option<SourceRange> {
        self.body_range
    }
    pub fn normalized_signature(&self) -> &str {
        &self.normalized_signature
    }
    pub fn parameter_count(&self) -> u32 {
        self.parameter_count
    }
    pub fn syntactic_nesting_depth(&self) -> u32 {
        self.syntactic_nesting_depth
    }
    pub fn type_member_count(&self) -> u32 {
        self.type_member_count
    }

    /// Owned UTF-8 payload bytes retained by this fact, including duplicated provenance and key.
    pub fn estimated_owned_bytes(&self) -> u64 {
        saturating_owned_sum([
            self.provenance.estimated_owned_bytes(),
            self.key.estimated_owned_bytes(),
            string_owned_bytes(&self.normalized_signature),
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlContext {
    Condition,
    Loop,
    MatchArm,
    ErrorBranch,
    Callback,
    Closure,
    Other(String),
}

impl ControlContext {
    /// Owned UTF-8 payload bytes retained by this context.
    pub fn estimated_owned_bytes(&self) -> u64 {
        match self {
            Self::Other(value) => string_owned_bytes(value),
            _ => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(try_from = "CallFactWire")]
pub struct CallFact {
    provenance: SyntaxProvenance,
    callee_text: String,
    argument_text: String,
    argument_range: SourceRange,
    call_site_range: SourceRange,
    enclosing_symbol: Option<SymbolKey>,
    control_context: Vec<ControlContext>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CallFactWire {
    provenance: SyntaxProvenance,
    callee_text: String,
    argument_text: String,
    argument_range: SourceRange,
    call_site_range: SourceRange,
    enclosing_symbol: Option<SymbolKey>,
    control_context: Vec<ControlContext>,
}

impl TryFrom<CallFactWire> for CallFact {
    type Error = ModelError;
    fn try_from(value: CallFactWire) -> Result<Self, Self::Error> {
        Self::new(
            value.provenance,
            value.callee_text,
            value.argument_text,
            value.argument_range,
            value.call_site_range,
            value.enclosing_symbol,
            value.control_context,
        )
    }
}

impl<'de> Deserialize<'de> for CallFact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        CallFactWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl CallFact {
    pub fn new(
        provenance: SyntaxProvenance,
        callee_text: impl Into<String>,
        argument_text: impl Into<String>,
        argument_range: SourceRange,
        call_site_range: SourceRange,
        enclosing_symbol: Option<SymbolKey>,
        control_context: Vec<ControlContext>,
    ) -> Result<Self, ModelError> {
        let callee_text = callee_text.into();
        let argument_text = argument_text.into();
        if callee_text.trim().is_empty() {
            return Err(ModelError::new("callee text must not be empty"));
        }
        if !call_site_range.contains(argument_range) {
            return Err(ModelError::new("argument range must be inside call site"));
        }
        Ok(Self {
            provenance,
            callee_text,
            argument_text,
            argument_range,
            call_site_range,
            enclosing_symbol,
            control_context,
        })
    }
    pub fn provenance(&self) -> &SyntaxProvenance {
        &self.provenance
    }
    pub fn callee_text(&self) -> &str {
        &self.callee_text
    }
    pub fn argument_text(&self) -> &str {
        &self.argument_text
    }
    pub fn argument_range(&self) -> SourceRange {
        self.argument_range
    }
    pub fn call_site_range(&self) -> SourceRange {
        self.call_site_range
    }
    pub fn enclosing_symbol(&self) -> Option<&SymbolKey> {
        self.enclosing_symbol.as_ref()
    }
    pub fn control_context(&self) -> &[ControlContext] {
        &self.control_context
    }

    /// Owned UTF-8 payload bytes retained by this fact, including duplicated provenance and key.
    pub fn estimated_owned_bytes(&self) -> u64 {
        let direct = saturating_owned_sum([
            self.provenance.estimated_owned_bytes(),
            string_owned_bytes(&self.callee_text),
            string_owned_bytes(&self.argument_text),
            self.enclosing_symbol
                .as_ref()
                .map_or(0, SymbolKey::estimated_owned_bytes),
        ]);
        self.control_context
            .iter()
            .map(ControlContext::estimated_owned_bytes)
            .fold(direct, u64::saturating_add)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    Parsed,
    Partial,
    Unsupported,
    Failed,
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyntaxTruncationReason {
    SourceBytes,
    CaptureBytes,
    SymbolCount,
    CallCount,
    DiagnosticCount,
    Time,
    Cancelled,
}

/// The concrete bounded resource which stopped syntax analysis.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SyntaxTruncation {
    reason: SyntaxTruncationReason,
    limit: Option<u64>,
    observed: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntaxTruncationWire {
    reason: SyntaxTruncationReason,
    limit: Option<u64>,
    observed: Option<u64>,
}

impl TryFrom<SyntaxTruncationWire> for SyntaxTruncation {
    type Error = ModelError;

    fn try_from(value: SyntaxTruncationWire) -> Result<Self, Self::Error> {
        Self::new(value.reason, value.limit, value.observed)
    }
}

impl<'de> Deserialize<'de> for SyntaxTruncation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SyntaxTruncationWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl SyntaxTruncation {
    pub fn new(
        reason: SyntaxTruncationReason,
        limit: Option<u64>,
        observed: Option<u64>,
    ) -> Result<Self, ModelError> {
        match reason {
            SyntaxTruncationReason::Cancelled if limit.is_some() || observed.is_some() => {
                return Err(ModelError::new(
                    "cancelled truncation cannot carry numeric measurements",
                ));
            }
            SyntaxTruncationReason::Cancelled => {}
            SyntaxTruncationReason::CaptureBytes => match (limit, observed) {
                (Some(limit), Some(observed))
                    if limit > 0
                        && (observed > limit || limit == u64::MAX && observed == u64::MAX) => {}
                _ => {
                    return Err(ModelError::new(
                        "capture-byte truncation requires a positive limit and observed value above it",
                    ));
                }
            },
            _ => match (limit, observed) {
                (Some(limit), Some(observed)) if limit > 0 && observed >= limit => {}
                _ => {
                    return Err(ModelError::new(
                        "bounded truncation requires a positive limit and observed value at least equal to it",
                    ));
                }
            },
        }
        Ok(Self {
            reason,
            limit,
            observed,
        })
    }

    pub fn reason(&self) -> SyntaxTruncationReason {
        self.reason
    }
    pub fn limit(&self) -> Option<u64> {
        self.limit
    }
    pub fn observed(&self) -> Option<u64> {
        self.observed
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(try_from = "SyntaxDiagnosticWire")]
pub struct SyntaxDiagnostic {
    severity: DiagnosticSeverity,
    message: String,
    range: Option<SourceRange>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SyntaxDiagnosticWire {
    severity: DiagnosticSeverity,
    message: String,
    range: Option<SourceRange>,
}

impl TryFrom<SyntaxDiagnosticWire> for SyntaxDiagnostic {
    type Error = ModelError;
    fn try_from(value: SyntaxDiagnosticWire) -> Result<Self, Self::Error> {
        Self::new(value.severity, value.message, value.range)
    }
}
impl<'de> Deserialize<'de> for SyntaxDiagnostic {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        SyntaxDiagnosticWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl SyntaxDiagnostic {
    pub fn new(
        severity: DiagnosticSeverity,
        message: impl Into<String>,
        range: Option<SourceRange>,
    ) -> Result<Self, ModelError> {
        let message = message.into();
        if message.trim().is_empty() {
            return Err(ModelError::new("diagnostic must not be empty"));
        }
        Ok(Self {
            severity,
            message,
            range,
        })
    }
    pub fn severity(&self) -> DiagnosticSeverity {
        self.severity
    }
    pub fn message(&self) -> &str {
        &self.message
    }
    pub fn range(&self) -> Option<SourceRange> {
        self.range
    }

    /// Owned UTF-8 payload bytes retained by this diagnostic.
    pub fn estimated_owned_bytes(&self) -> u64 {
        string_owned_bytes(&self.message)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(try_from = "DocumentStructureWire")]
pub struct DocumentStructure {
    path: String,
    provenance: SyntaxProvenance,
    status: DocumentStatus,
    symbols: Vec<SymbolFact>,
    calls: Vec<CallFact>,
    diagnostics: Vec<SyntaxDiagnostic>,
    truncation: Option<SyntaxTruncation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DocumentStructureWire {
    path: String,
    provenance: SyntaxProvenance,
    status: DocumentStatus,
    symbols: Vec<SymbolFact>,
    calls: Vec<CallFact>,
    diagnostics: Vec<SyntaxDiagnostic>,
    truncation: Option<SyntaxTruncation>,
}

impl TryFrom<DocumentStructureWire> for DocumentStructure {
    type Error = ModelError;
    fn try_from(value: DocumentStructureWire) -> Result<Self, Self::Error> {
        Self::new(
            value.path,
            value.provenance,
            value.status,
            value.symbols,
            value.calls,
            value.diagnostics,
            value.truncation,
        )
    }
}
impl<'de> Deserialize<'de> for DocumentStructure {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        DocumentStructureWire::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
impl DocumentStructure {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: impl Into<String>,
        provenance: SyntaxProvenance,
        status: DocumentStatus,
        symbols: Vec<SymbolFact>,
        calls: Vec<CallFact>,
        diagnostics: Vec<SyntaxDiagnostic>,
        truncation: Option<SyntaxTruncation>,
    ) -> Result<Self, ModelError> {
        let path = path.into();
        if path.trim().is_empty() {
            return Err(ModelError::new("document path must not be empty"));
        }
        if symbols.iter().any(|fact| fact.provenance() != &provenance)
            || calls.iter().any(|fact| fact.provenance() != &provenance)
        {
            return Err(ModelError::new("document facts must share its provenance"));
        }
        let no_facts = symbols.is_empty() && calls.is_empty();
        if matches!(
            status,
            DocumentStatus::Unsupported | DocumentStatus::Failed | DocumentStatus::Skipped
        ) && !no_facts
        {
            return Err(ModelError::new(
                "unsuccessful documents cannot contain syntax facts",
            ));
        }
        if status == DocumentStatus::Parsed
            && (truncation.is_some()
                || diagnostics
                    .iter()
                    .any(|d| d.severity() == DiagnosticSeverity::Error))
        {
            return Err(ModelError::new(
                "parsed status cannot carry truncation or errors",
            ));
        }
        if status == DocumentStatus::Partial
            && truncation.is_none()
            && !diagnostics.iter().any(|diagnostic| {
                matches!(
                    diagnostic.severity(),
                    DiagnosticSeverity::Warning | DiagnosticSeverity::Error
                )
            })
        {
            return Err(ModelError::new(
                "partial status requires truncation or warning/error evidence",
            ));
        }
        if status == DocumentStatus::Failed
            && !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
        {
            return Err(ModelError::new(
                "failed status requires an error diagnostic",
            ));
        }
        Ok(Self {
            path,
            provenance,
            status,
            symbols,
            calls,
            diagnostics,
            truncation,
        })
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn provenance(&self) -> &SyntaxProvenance {
        &self.provenance
    }
    pub fn status(&self) -> DocumentStatus {
        self.status
    }
    pub fn symbols(&self) -> &[SymbolFact] {
        &self.symbols
    }
    pub fn calls(&self) -> &[CallFact] {
        &self.calls
    }
    pub fn diagnostics(&self) -> &[SyntaxDiagnostic] {
        &self.diagnostics
    }
    pub fn truncation(&self) -> Option<&SyntaxTruncation> {
        self.truncation.as_ref()
    }

    /// Estimated retained UTF-8 payload bytes in this complete document.
    ///
    /// Every owned string is counted at each storage location, including cloned provenance and
    /// symbol keys. Fixed-size fields, vector capacity, and allocator overhead are excluded. If a
    /// sum exceeds the fixed-width wire measurement, the result saturates at `u64::MAX`.
    pub fn estimated_owned_bytes(&self) -> u64 {
        let base = saturating_owned_sum([
            string_owned_bytes(&self.path),
            self.provenance.estimated_owned_bytes(),
        ]);
        let with_symbols = self
            .symbols
            .iter()
            .map(SymbolFact::estimated_owned_bytes)
            .fold(base, u64::saturating_add);
        let with_calls = self
            .calls
            .iter()
            .map(CallFact::estimated_owned_bytes)
            .fold(with_symbols, u64::saturating_add);
        self.diagnostics
            .iter()
            .map(SyntaxDiagnostic::estimated_owned_bytes)
            .fold(with_calls, u64::saturating_add)
    }
}

/// Server-selected bounded analysis limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisBudget {
    max_source_bytes: NonZeroU64,
    max_capture_bytes: NonZeroU64,
    max_symbols: NonZeroU32,
    max_calls: NonZeroU32,
    max_diagnostics: NonZeroU32,
}

/// Incremental retained-payload accounting for one syntax document.
///
/// Adapters initialize this with [`Self::for_document`], then account each candidate with the
/// matching typed method before pushing it into a retained collection. Rejected candidates do not
/// change the retained count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureByteTracker {
    limit: NonZeroU64,
    retained: u64,
}

impl CaptureByteTracker {
    fn new(limit: NonZeroU64) -> Self {
        Self { limit, retained: 0 }
    }

    pub fn for_document(
        limit: NonZeroU64,
        path: &str,
        provenance: &SyntaxProvenance,
    ) -> Result<Self, SyntaxTruncation> {
        let mut tracker = Self::new(limit);
        tracker.try_account(saturating_owned_sum([
            string_owned_bytes(path),
            provenance.estimated_owned_bytes(),
        ]))?;
        Ok(tracker)
    }

    pub fn limit(self) -> NonZeroU64 {
        self.limit
    }

    pub fn retained_bytes(self) -> u64 {
        self.retained
    }

    pub fn try_account_symbol(&mut self, candidate: &SymbolFact) -> Result<(), SyntaxTruncation> {
        self.try_account(candidate.estimated_owned_bytes())
    }

    pub fn try_account_call(&mut self, candidate: &CallFact) -> Result<(), SyntaxTruncation> {
        self.try_account(candidate.estimated_owned_bytes())
    }

    pub fn try_account_diagnostic(
        &mut self,
        candidate: &SyntaxDiagnostic,
    ) -> Result<(), SyntaxTruncation> {
        self.try_account(candidate.estimated_owned_bytes())
    }

    /// Account a candidate before retaining it. Exact-limit candidates are accepted.
    ///
    /// On fixed-width addition overflow, `observed` saturates at `u64::MAX` and the candidate is
    /// rejected even when the configured limit is also `u64::MAX`.
    fn try_account(&mut self, candidate_bytes: u64) -> Result<(), SyntaxTruncation> {
        let observed = match self.retained.checked_add(candidate_bytes) {
            Some(observed) => observed,
            None => return Err(capture_byte_truncation(self.limit, u64::MAX)),
        };
        if observed > self.limit.get() {
            return Err(capture_byte_truncation(self.limit, observed));
        }
        self.retained = observed;
        Ok(())
    }
}

/// Runtime-only stop control. It is deliberately separate from serializable document facts.
#[derive(Clone, Debug)]
pub struct AnalysisControl {
    started_at: Instant,
    deadline: Option<Instant>,
    time_limit_micros: NonZeroU64,
    cancelled_at: Arc<Mutex<Option<Instant>>>,
}

impl AnalysisControl {
    pub fn new(time_limit_micros: NonZeroU64) -> Self {
        Self::new_at(Instant::now(), time_limit_micros)
    }

    fn new_at(started_at: Instant, time_limit_micros: NonZeroU64) -> Self {
        let deadline = started_at.checked_add(Duration::from_micros(time_limit_micros.get()));
        Self {
            started_at,
            deadline,
            time_limit_micros,
            cancelled_at: Arc::new(Mutex::new(None)),
        }
    }

    pub fn cancel(&self) {
        self.cancel_at(Instant::now());
    }

    fn cancel_at(&self, cancelled_at: Instant) {
        let mut stored = match self.cancelled_at.lock() {
            Ok(stored) => stored,
            Err(poisoned) => poisoned.into_inner(),
        };
        if stored.is_none_or(|existing| cancelled_at < existing) {
            *stored = Some(cancelled_at);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation_instant().is_some()
    }

    fn cancellation_instant(&self) -> Option<Instant> {
        match self.cancelled_at.lock() {
            Ok(stored) => *stored,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    pub fn time_limit_micros(&self) -> NonZeroU64 {
        self.time_limit_micros
    }

    pub fn elapsed_micros(&self, now: Instant) -> u64 {
        duration_micros(now.saturating_duration_since(self.started_at))
    }

    pub fn deadline_exceeded(&self, now: Instant) -> bool {
        time_limit_reached(
            self.deadline,
            now,
            self.elapsed_micros(now),
            self.time_limit_micros,
        )
    }

    pub fn should_stop(&self, now: Instant) -> bool {
        self.cancellation_instant()
            .is_some_and(|cancelled_at| cancelled_at <= now)
            || self.deadline_exceeded(now)
    }

    /// Return inspectable stop evidence in the same microsecond unit as the configured limit.
    pub fn stop_truncation(&self, now: Instant) -> Result<Option<SyntaxTruncation>, ModelError> {
        let cancellation = self
            .cancellation_instant()
            .filter(|cancelled_at| *cancelled_at <= now);
        let time_reached = self.deadline_exceeded(now);
        let cancellation_first = cancellation.is_some_and(|cancelled_at| match self.deadline {
            Some(deadline) => cancelled_at < deadline,
            None => self.elapsed_micros(cancelled_at) < self.time_limit_micros.get(),
        });
        if cancellation_first || cancellation.is_some() && !time_reached {
            return SyntaxTruncation::new(SyntaxTruncationReason::Cancelled, None, None).map(Some);
        }
        if time_reached {
            let limit = self.time_limit_micros.get();
            let observed = self.elapsed_micros(now);
            if observed < limit {
                return Err(ModelError::new(
                    "expired analysis control measured less elapsed time than its configured limit",
                ));
            }
            return SyntaxTruncation::new(
                SyntaxTruncationReason::Time,
                Some(limit),
                Some(observed),
            )
            .map(Some);
        }
        Ok(None)
    }
}

fn time_limit_reached(
    deadline: Option<Instant>,
    now: Instant,
    elapsed_micros: u64,
    limit_micros: NonZeroU64,
) -> bool {
    deadline.map_or_else(
        || elapsed_micros >= limit_micros.get(),
        |deadline| now >= deadline,
    )
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn string_owned_bytes(value: &str) -> u64 {
    u64::try_from(value.len()).unwrap_or(u64::MAX)
}

fn saturating_owned_sum<const N: usize>(values: [u64; N]) -> u64 {
    values.into_iter().fold(0, u64::saturating_add)
}

fn capture_byte_truncation(limit: NonZeroU64, observed: u64) -> SyntaxTruncation {
    SyntaxTruncation {
        reason: SyntaxTruncationReason::CaptureBytes,
        limit: Some(limit.get()),
        observed: Some(observed),
    }
}

impl AnalysisBudget {
    pub fn new(
        max_source_bytes: NonZeroU64,
        max_symbols: NonZeroU32,
        max_calls: NonZeroU32,
        max_diagnostics: NonZeroU32,
    ) -> Self {
        Self {
            max_source_bytes,
            max_capture_bytes: max_source_bytes,
            max_symbols,
            max_calls,
            max_diagnostics,
        }
    }

    /// Override the default capture limit, which equals `max_source_bytes`.
    pub fn with_max_capture_bytes(mut self, max_capture_bytes: NonZeroU64) -> Self {
        self.max_capture_bytes = max_capture_bytes;
        self
    }

    pub fn max_source_bytes(self) -> NonZeroU64 {
        self.max_source_bytes
    }
    pub fn max_capture_bytes(self) -> NonZeroU64 {
        self.max_capture_bytes
    }
    pub fn max_symbols(self) -> NonZeroU32 {
        self.max_symbols
    }
    pub fn max_calls(self) -> NonZeroU32 {
        self.max_calls
    }
    pub fn max_diagnostics(self) -> NonZeroU32 {
        self.max_diagnostics
    }
}

/// Owned adapter input. The server owns both the source snapshot and analysis budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisInput {
    path: String,
    language: SyntaxLanguage,
    source: String,
}

impl AnalysisInput {
    pub fn new(
        path: impl Into<String>,
        language: SyntaxLanguage,
        source: String,
    ) -> Result<Self, ModelError> {
        let path = path.into();
        if path.trim().is_empty() {
            return Err(ModelError::new("analysis path must not be empty"));
        }
        Ok(Self {
            path,
            language,
            source,
        })
    }
    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn language(&self) -> SyntaxLanguage {
        self.language
    }
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Shared adapter seam. Implementations must return explicit partial/failed output.
pub trait SyntaxAdapter: Send + Sync {
    fn language(&self) -> SyntaxLanguage;
    fn supports(&self, language: SyntaxLanguage) -> bool {
        language == self.language()
    }
    fn analyze(
        &self,
        input: AnalysisInput,
        budget: AnalysisBudget,
        control: &AnalysisControl,
    ) -> Result<DocumentStructure, ModelError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn nz(value: u32) -> NonZeroU32 {
        NonZeroU32::new(value).unwrap()
    }
    fn nz64(value: u64) -> NonZeroU64 {
        NonZeroU64::new(value).unwrap()
    }
    fn range(start: u64, end: u64, start_line: u32, end_line: u32) -> SourceRange {
        SourceRange::new(start, end, nz(start_line), nz(end_line)).unwrap()
    }
    fn provenance() -> SyntaxProvenance {
        SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, "tree-sitter-rust@0.24").unwrap()
    }
    fn symbol() -> SymbolFact {
        SymbolFact::new(
            provenance(),
            SymbolKey::new(vec!["worker".into()], SymbolKind::Function, "run").unwrap(),
            SymbolVisibility::Public,
            range(0, 30, 1, 3),
            range(0, 12, 1, 1),
            Some(range(13, 30, 1, 3)),
            "pub fn run()",
            0,
            1,
            0,
        )
        .unwrap()
    }

    fn call(argument_text: impl Into<String>) -> CallFact {
        CallFact::new(
            provenance(),
            "work",
            argument_text,
            range(4, 96, 1, 1),
            range(0, 100, 1, 1),
            Some(SymbolKey::new(vec!["worker".into()], SymbolKind::Function, "run").unwrap()),
            vec![
                ControlContext::Condition,
                ControlContext::Other("guard".into()),
            ],
        )
        .unwrap()
    }

    #[test]
    fn validates_utf8_and_wire_ranges() {
        assert!(range(1, 3, 1, 1).validate_source("aéz").is_ok());
        assert!(range(2, 3, 1, 1).validate_source("aéz").is_err());
        assert!(
            serde_json::from_value::<SourceRange>(
                json!({"start_byte": 3, "end_byte": 2, "start_line": 1, "end_line": 1})
            )
            .is_err()
        );
        assert_eq!(
            SourceRange::from_tree_sitter(0, 8, 0, 2, 0)
                .unwrap()
                .end_line()
                .get(),
            2
        );
        assert!(
            serde_json::from_value::<SourceRange>(
                json!({"start_byte": 0, "end_byte": 2, "start_line": 0, "end_line": 1})
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_nested_wire_models_are_rejected() {
        assert!(
            serde_json::from_value::<SyntaxProvenance>(json!({"language":"rust","parser":""}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<SymbolKey>(
                json!({"qualified_path":[""],"kind":"function","name":"run"})
            )
            .is_err()
        );
        let mut value = serde_json::to_value(symbol()).unwrap();
        value["signature_range"]["end_byte"] = json!(99);
        assert!(serde_json::from_value::<SymbolFact>(value).is_err());
        assert!(
            serde_json::from_value::<SyntaxDiagnostic>(json!({
                "severity": "error",
                "message": "",
                "range": null
            }))
            .is_err()
        );
    }

    #[test]
    fn failed_documents_reject_successful_facts_on_the_wire() {
        let value = json!({"path":"src/lib.rs","provenance":provenance(),"status":"failed","symbols":[symbol()],"calls":[],"diagnostics":[],"truncation":null});
        assert!(serde_json::from_value::<DocumentStructure>(value).is_err());
    }

    #[test]
    fn document_structure_serde_round_trips() {
        let document = DocumentStructure::new(
            "src/lib.rs",
            provenance(),
            DocumentStatus::Parsed,
            vec![symbol()],
            Vec::new(),
            Vec::new(),
            None,
        )
        .unwrap();
        let json = serde_json::to_string(&document).unwrap();
        assert_eq!(
            serde_json::from_str::<DocumentStructure>(&json).unwrap(),
            document
        );
    }

    #[test]
    fn truncation_and_status_evidence_are_validated_on_the_wire() {
        assert!(
            serde_json::from_value::<SyntaxTruncation>(json!({
                "reason": "source_bytes", "limit": 100, "observed": 99
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SyntaxTruncation>(json!({
                "reason": "cancelled", "limit": 100, "observed": 100
            }))
            .is_err()
        );

        let failed_without_error = json!({
            "path":"src/lib.rs", "provenance":provenance(), "status":"failed",
            "symbols":[], "calls":[], "diagnostics":[], "truncation":null
        });
        assert!(serde_json::from_value::<DocumentStructure>(failed_without_error).is_err());

        let partial_with_info = json!({
            "path":"src/lib.rs", "provenance":provenance(), "status":"partial",
            "symbols":[], "calls":[],
            "diagnostics":[{"severity":"info","message":"note","range":null}],
            "truncation":null
        });
        assert!(serde_json::from_value::<DocumentStructure>(partial_with_info).is_err());

        let partial_with_limit = json!({
            "path":"src/lib.rs", "provenance":provenance(), "status":"partial",
            "symbols":[], "calls":[], "diagnostics":[],
            "truncation":{"reason":"call_count","limit":10,"observed":10}
        });
        assert!(serde_json::from_value::<DocumentStructure>(partial_with_limit).is_ok());
    }

    #[test]
    fn capture_byte_truncation_has_validated_golden_wire_evidence() {
        assert!(
            serde_json::from_value::<SyntaxTruncation>(json!({
                "reason": "capture_bytes", "limit": 100, "observed": 100
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<SyntaxTruncation>(json!({
                "reason": "capture_bytes", "limit": 0, "observed": 1
            }))
            .is_err()
        );
        let truncation =
            SyntaxTruncation::new(SyntaxTruncationReason::CaptureBytes, Some(100), Some(101))
                .unwrap();
        assert_eq!(
            serde_json::to_string(&truncation).unwrap(),
            r#"{"reason":"capture_bytes","limit":100,"observed":101}"#
        );
        assert_eq!(
            serde_json::from_str::<SyntaxTruncation>(
                r#"{"reason":"capture_bytes","limit":100,"observed":101}"#
            )
            .unwrap(),
            truncation
        );
    }

    #[test]
    fn owned_byte_estimates_count_each_retained_string_copy() {
        let provenance = provenance();
        let symbol = symbol();
        let call = call("(nested(value))");
        let diagnostic =
            SyntaxDiagnostic::new(DiagnosticSeverity::Info, "parser note", None).unwrap();
        assert_eq!(
            provenance.estimated_owned_bytes(),
            provenance.parser().len() as u64
        );
        assert_eq!(
            symbol.key().estimated_owned_bytes(),
            ("worker".len() + "run".len()) as u64
        );
        assert_eq!(
            symbol.estimated_owned_bytes(),
            (provenance.parser().len() + "worker".len() + "run".len() + "pub fn run()".len())
                as u64
        );
        assert_eq!(
            call.estimated_owned_bytes(),
            (provenance.parser().len()
                + "work".len()
                + "(nested(value))".len()
                + "worker".len()
                + "run".len()
                + "guard".len()) as u64
        );
        assert_eq!(
            diagnostic.estimated_owned_bytes(),
            "parser note".len() as u64
        );

        let expected = "src/lib.rs".len() as u64
            + provenance.estimated_owned_bytes()
            + symbol.estimated_owned_bytes()
            + call.estimated_owned_bytes()
            + diagnostic.estimated_owned_bytes();
        let mut tracker =
            CaptureByteTracker::for_document(nz64(expected), "src/lib.rs", &provenance).unwrap();
        tracker.try_account_symbol(&symbol).unwrap();
        tracker.try_account_call(&call).unwrap();
        tracker.try_account_diagnostic(&diagnostic).unwrap();
        assert_eq!(tracker.retained_bytes(), expected);
        let document = DocumentStructure::new(
            "src/lib.rs",
            provenance,
            DocumentStatus::Parsed,
            vec![symbol],
            vec![call],
            vec![diagnostic],
            None,
        )
        .unwrap();
        assert_eq!(document.estimated_owned_bytes(), expected);
    }

    #[test]
    fn overlapping_call_arguments_are_bounded_independently_of_fact_count() {
        let outer_argument = format!("({})", "nested(".repeat(64));
        let inner_argument = outer_argument[1..].to_string();
        let outer = call(outer_argument.clone());
        let inner = call(inner_argument.clone());
        assert_eq!(outer.argument_text().len(), outer_argument.len());
        assert_eq!(inner.argument_text().len(), inner_argument.len());
        assert!(outer.argument_text().len() + inner.argument_text().len() > outer_argument.len());

        let provenance = provenance();
        let base = string_owned_bytes("src/lib.rs") + provenance.estimated_owned_bytes();
        let exact = base + outer.estimated_owned_bytes() + inner.estimated_owned_bytes();
        let mut exact_tracker =
            CaptureByteTracker::for_document(nz64(exact), "src/lib.rs", &provenance).unwrap();
        exact_tracker.try_account_call(&outer).unwrap();
        exact_tracker.try_account_call(&inner).unwrap();
        assert_eq!(exact_tracker.retained_bytes(), exact);

        let mut short_tracker =
            CaptureByteTracker::for_document(nz64(exact - 1), "src/lib.rs", &provenance).unwrap();
        short_tracker.try_account_call(&outer).unwrap();
        let truncation = short_tracker.try_account_call(&inner).unwrap_err();
        assert_eq!(truncation.reason(), SyntaxTruncationReason::CaptureBytes);
        assert_eq!(truncation.limit(), Some(exact - 1));
        assert_eq!(truncation.observed(), Some(exact));
    }

    #[test]
    fn capture_accounting_saturates_evidence_and_never_accepts_overflow() {
        assert_eq!(saturating_owned_sum([u64::MAX, 1]), u64::MAX);
        let mut tracker = CaptureByteTracker::new(nz64(u64::MAX));
        tracker.try_account(u64::MAX).unwrap();
        let truncation = tracker.try_account(1).unwrap_err();
        assert_eq!(truncation.limit(), Some(u64::MAX));
        assert_eq!(truncation.observed(), Some(u64::MAX));
        assert_eq!(tracker.retained_bytes(), u64::MAX);
        assert!(
            SyntaxTruncation::new(
                SyntaxTruncationReason::CaptureBytes,
                Some(u64::MAX),
                Some(u64::MAX),
            )
            .is_ok()
        );
    }

    #[test]
    fn analysis_control_reports_configured_and_elapsed_microseconds() {
        assert_eq!(
            AnalysisControl::new(nz64(100)).time_limit_micros(),
            nz64(100)
        );
        let started_at = Instant::now();
        let control = AnalysisControl::new_at(started_at, nz64(100));
        let before_deadline = started_at.checked_add(Duration::from_micros(40)).unwrap();

        assert_eq!(control.time_limit_micros(), nz64(100));
        assert_eq!(control.elapsed_micros(before_deadline), 40);
        assert!(!control.deadline_exceeded(before_deadline));
        assert!(!control.should_stop(before_deadline));
        assert_eq!(control.stop_truncation(before_deadline).unwrap(), None);
    }

    #[test]
    fn expired_control_reports_truthful_time_truncation() {
        let started_at = Instant::now();
        let control = AnalysisControl::new_at(started_at, nz64(100));
        let after_deadline = started_at.checked_add(Duration::from_micros(175)).unwrap();

        assert!(control.deadline_exceeded(after_deadline));
        assert!(control.should_stop(after_deadline));
        let truncation = control.stop_truncation(after_deadline).unwrap().unwrap();
        assert_eq!(truncation.reason(), SyntaxTruncationReason::Time);
        assert_eq!(truncation.limit(), Some(100));
        assert_eq!(truncation.observed(), Some(175));
    }

    #[test]
    fn cancellation_before_deadline_is_the_shared_first_stop_cause() {
        let started_at = Instant::now();
        let control = AnalysisControl::new_at(started_at, nz64(100));
        let cloned = control.clone();
        let cancelled_at = started_at.checked_add(Duration::from_micros(40)).unwrap();
        let observed_at = started_at.checked_add(Duration::from_micros(175)).unwrap();
        cloned.cancel_at(cancelled_at);

        assert!(control.is_cancelled());
        assert!(control.should_stop(observed_at));
        let truncation = control.stop_truncation(observed_at).unwrap().unwrap();
        assert_eq!(
            cloned.stop_truncation(observed_at).unwrap().as_ref(),
            Some(&truncation)
        );
        assert_eq!(truncation.reason(), SyntaxTruncationReason::Cancelled);
        assert_eq!(truncation.limit(), None);
        assert_eq!(truncation.observed(), None);
    }

    #[test]
    fn deadline_before_cancellation_remains_the_first_stop_cause() {
        let started_at = Instant::now();
        let control = AnalysisControl::new_at(started_at, nz64(100));
        control.cancel_at(started_at.checked_add(Duration::from_micros(150)).unwrap());
        let observed_at = started_at.checked_add(Duration::from_micros(175)).unwrap();

        let truncation = control.stop_truncation(observed_at).unwrap().unwrap();
        assert_eq!(truncation.reason(), SyntaxTruncationReason::Time);
        assert_eq!(truncation.limit(), Some(100));
        assert_eq!(truncation.observed(), Some(175));
    }

    #[test]
    fn cancellation_at_deadline_deterministically_reports_time() {
        let started_at = Instant::now();
        let control = AnalysisControl::new_at(started_at, nz64(100));
        let boundary = started_at.checked_add(Duration::from_micros(100)).unwrap();
        control.cancel_at(boundary);

        let truncation = control.stop_truncation(boundary).unwrap().unwrap();
        assert_eq!(truncation.reason(), SyntaxTruncationReason::Time);
        assert_eq!(truncation.limit(), Some(100));
        assert_eq!(truncation.observed(), Some(100));
    }

    #[test]
    fn analysis_control_tests_overflow_fallback_and_fixed_width_saturation() {
        assert_eq!(duration_micros(Duration::MAX), u64::MAX);
        let now = Instant::now();
        assert!(!time_limit_reached(None, now, 99, nz64(100)));
        assert!(time_limit_reached(None, now, 100, nz64(100)));
    }

    #[test]
    fn time_truncation_requires_matching_positive_microsecond_evidence() {
        assert!(SyntaxTruncation::new(SyntaxTruncationReason::Time, Some(100), Some(100)).is_ok());
        assert!(SyntaxTruncation::new(SyntaxTruncationReason::Time, Some(100), Some(99)).is_err());
        assert!(SyntaxTruncation::new(SyntaxTruncationReason::Time, None, None).is_err());
        assert!(
            SyntaxTruncation::new(SyntaxTruncationReason::DiagnosticCount, Some(64), Some(65))
                .is_ok()
        );
        assert!(
            SyntaxTruncation::new(SyntaxTruncationReason::DiagnosticCount, Some(64), Some(63))
                .is_err()
        );
    }

    #[test]
    fn analysis_budget_exposes_every_positive_fixed_width_limit() {
        let budget = AnalysisBudget::new(nz64(1_000), nz(10), nz(20), nz(30));
        assert_eq!(budget.max_source_bytes(), nz64(1_000));
        assert_eq!(budget.max_capture_bytes(), nz64(1_000));
        assert_eq!(budget.max_symbols(), nz(10));
        assert_eq!(budget.max_calls(), nz(20));
        assert_eq!(budget.max_diagnostics(), nz(30));

        let overridden = budget.with_max_capture_bytes(nz64(750));
        assert_eq!(overridden.max_source_bytes(), nz64(1_000));
        assert_eq!(overridden.max_capture_bytes(), nz64(750));
    }

    #[test]
    fn syntax_adapter_default_support_matches_its_primary_language() {
        struct RustOnly;
        impl SyntaxAdapter for RustOnly {
            fn language(&self) -> SyntaxLanguage {
                SyntaxLanguage::Rust
            }

            fn analyze(
                &self,
                _input: AnalysisInput,
                _budget: AnalysisBudget,
                _control: &AnalysisControl,
            ) -> Result<DocumentStructure, ModelError> {
                Err(ModelError::new("not used by this contract test"))
            }
        }

        assert!(RustOnly.supports(SyntaxLanguage::Rust));
        assert!(!RustOnly.supports(SyntaxLanguage::TypeScript));
        assert!(!RustOnly.supports(SyntaxLanguage::Tsx));
    }
}
