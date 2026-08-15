use crate::SyntaxLanguage;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

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
    SymbolCount,
    CallCount,
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
}

/// Server-selected bounded analysis limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisBudget {
    max_source_bytes: NonZeroU64,
    max_symbols: NonZeroU32,
    max_calls: NonZeroU32,
}

/// Runtime-only stop control. It is deliberately separate from serializable document facts.
#[derive(Clone, Debug)]
pub struct AnalysisControl {
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}

impl AnalysisControl {
    pub fn new(deadline: Instant) -> Self {
        Self {
            deadline,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    pub fn deadline_exceeded(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    pub fn should_stop(&self, now: Instant) -> bool {
        self.is_cancelled() || self.deadline_exceeded(now)
    }
}

impl AnalysisBudget {
    pub fn new(
        max_source_bytes: NonZeroU64,
        max_symbols: NonZeroU32,
        max_calls: NonZeroU32,
    ) -> Self {
        Self {
            max_source_bytes,
            max_symbols,
            max_calls,
        }
    }
    pub fn max_source_bytes(self) -> NonZeroU64 {
        self.max_source_bytes
    }
    pub fn max_symbols(self) -> NonZeroU32 {
        self.max_symbols
    }
    pub fn max_calls(self) -> NonZeroU32 {
        self.max_calls
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
}
