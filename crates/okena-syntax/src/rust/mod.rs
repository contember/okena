//! Rust tree-sitter adapter.

use crate::{
    AnalysisBudget, AnalysisControl, AnalysisInput, CallFact, ControlContext, DiagnosticSeverity,
    DocumentStatus, DocumentStructure, ModelError, SourceRange, SymbolFact, SymbolKey, SymbolKind,
    SymbolVisibility, SyntaxAdapter, SyntaxDiagnostic, SyntaxLanguage, SyntaxProvenance,
    SyntaxTruncation, SyntaxTruncationReason,
};
use std::ops::ControlFlow;
use std::time::Instant;
use tree_sitter::{Node, ParseOptions, Parser, Point};

const PARSER_NAME: &str = "tree-sitter-rust@0.24.2";

#[derive(Clone, Copy, Debug, Default)]
pub struct RustAdapter;

impl RustAdapter {
    pub fn new() -> Self {
        Self
    }

    fn failed(path: &str, message: &str) -> Result<DocumentStructure, ModelError> {
        DocumentStructure::new(
            path,
            provenance()?,
            DocumentStatus::Failed,
            Vec::new(),
            Vec::new(),
            vec![SyntaxDiagnostic::new(
                DiagnosticSeverity::Error,
                message,
                None,
            )?],
            None,
        )
    }
}

impl SyntaxAdapter for RustAdapter {
    fn language(&self) -> SyntaxLanguage {
        SyntaxLanguage::Rust
    }

    fn analyze(
        &self,
        input: AnalysisInput,
        budget: AnalysisBudget,
        control: &AnalysisControl,
    ) -> Result<DocumentStructure, ModelError> {
        let provenance = provenance()?;
        if input.language() != SyntaxLanguage::Rust {
            return Self::failed(input.path(), "Rust adapter received a non-Rust document");
        }
        if let Some(truncation) = control.stop_truncation(Instant::now())? {
            return truncated_document(&input, provenance, truncation);
        }
        let source_bytes = u64::try_from(input.source().len()).unwrap_or(u64::MAX);
        if source_bytes > budget.max_source_bytes().get() {
            return truncated_document(
                &input,
                provenance,
                SyntaxTruncation::new(
                    SyntaxTruncationReason::SourceBytes,
                    Some(budget.max_source_bytes().get()),
                    Some(source_bytes),
                )?,
            );
        }

        let mut parser = Parser::new();
        if parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .is_err()
        {
            return Self::failed(input.path(), "tree-sitter rejected the Rust grammar");
        }
        let source = input.source().as_bytes();
        let mut parse_stopped = false;
        let mut progress = |_: &tree_sitter::ParseState| {
            if control.should_stop(Instant::now()) {
                parse_stopped = true;
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = ParseOptions::new().progress_callback(&mut progress);
        let Some(tree) = parser.parse_with_options(
            &mut |offset, _| source.get(offset..).unwrap_or_default(),
            None,
            Some(options),
        ) else {
            if parse_stopped && let Some(truncation) = control.stop_truncation(Instant::now())? {
                return truncated_document(&input, provenance, truncation);
            }
            return Self::failed(input.path(), "tree-sitter did not produce a Rust tree");
        };
        if parse_stopped && let Some(truncation) = control.stop_truncation(Instant::now())? {
            return truncated_document(&input, provenance, truncation);
        }

        let mut engine = Engine {
            source: input.source(),
            provenance: provenance.clone(),
            budget,
            control,
            symbols: Vec::new(),
            calls: Vec::new(),
            truncation: None,
        };
        engine.extract(tree.root_node())?;

        let mut diagnostics = Vec::new();
        if engine.truncation.is_none()
            && tree.root_node().has_error()
            && let Some(error) = engine.first_error(tree.root_node())?
        {
            diagnostics.push(SyntaxDiagnostic::new(
                DiagnosticSeverity::Error,
                "Rust syntax contains an error or missing token",
                node_range(error),
            )?);
        }
        let status = if engine.truncation.is_some() || !diagnostics.is_empty() {
            DocumentStatus::Partial
        } else {
            DocumentStatus::Parsed
        };
        DocumentStructure::new(
            input.path(),
            provenance,
            status,
            engine.symbols,
            engine.calls,
            diagnostics,
            engine.truncation,
        )
    }
}

fn provenance() -> Result<SyntaxProvenance, ModelError> {
    SyntaxProvenance::tree_sitter(SyntaxLanguage::Rust, PARSER_NAME)
}

fn truncated_document(
    input: &AnalysisInput,
    provenance: SyntaxProvenance,
    truncation: SyntaxTruncation,
) -> Result<DocumentStructure, ModelError> {
    DocumentStructure::new(
        input.path(),
        provenance,
        DocumentStatus::Partial,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Some(truncation),
    )
}

#[derive(Clone, Default)]
struct WalkContext {
    path: Vec<String>,
    enclosing_symbol: Option<SymbolKey>,
    method_parent: bool,
    member_context: Option<MemberContext>,
    controls: Vec<ControlContext>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MemberContext {
    Trait,
    TraitImpl,
    InherentImpl,
    Inherited,
}

struct Engine<'a> {
    source: &'a str,
    provenance: SyntaxProvenance,
    budget: AnalysisBudget,
    control: &'a AnalysisControl,
    symbols: Vec<SymbolFact>,
    calls: Vec<CallFact>,
    truncation: Option<SyntaxTruncation>,
}

impl Engine<'_> {
    fn extract(&mut self, root: Node<'_>) -> Result<(), ModelError> {
        let mut stack = vec![(root, WalkContext::default())];
        while let Some((node, context)) = stack.pop() {
            if self.should_stop()? {
                break;
            }

            let mut child_context = context.clone();
            if let Some(spec) = symbol_spec(node, &context, self.source) {
                if self.symbols.len() >= usize_from_u32(self.budget.max_symbols().get()) {
                    self.truncation = Some(SyntaxTruncation::new(
                        SyntaxTruncationReason::SymbolCount,
                        Some(u64::from(self.budget.max_symbols().get())),
                        Some(u64::from(self.budget.max_symbols().get()) + 1),
                    )?);
                    break;
                }
                let Some(depth) = self.syntactic_nesting_depth(body_node(node).unwrap_or(node))?
                else {
                    break;
                };
                let Some(parameters) = self.parameter_count(node)? else {
                    break;
                };
                let Some(members) = self.type_member_count(node)? else {
                    break;
                };
                let metrics = SymbolMetrics {
                    syntactic_nesting_depth: depth,
                    parameter_count: parameters,
                    type_member_count: members,
                };
                if let Some(fact) = make_symbol(
                    node,
                    &spec,
                    &context,
                    self.source,
                    &self.provenance,
                    metrics,
                )? {
                    child_context.path.push(spec.name.clone());
                    child_context.enclosing_symbol = Some(fact.key().clone());
                    child_context.method_parent =
                        matches!(spec.kind, SymbolKind::Trait | SymbolKind::Impl);
                    child_context.member_context = member_context(node, spec.kind);
                    self.symbols.push(fact);
                }
            }

            if let Some(call) = make_call(node, &context, self.source, &self.provenance)? {
                if self.calls.len() >= usize_from_u32(self.budget.max_calls().get()) {
                    self.truncation = Some(SyntaxTruncation::new(
                        SyntaxTruncationReason::CallCount,
                        Some(u64::from(self.budget.max_calls().get())),
                        Some(u64::from(self.budget.max_calls().get()) + 1),
                    )?);
                    break;
                }
                self.calls.push(call);
            }

            for index in (0..node.named_child_count()).rev() {
                if self.should_stop()? {
                    break;
                }
                let Ok(child_index) = u32::try_from(index) else {
                    continue;
                };
                let Some(child) = node.named_child(child_index) else {
                    continue;
                };
                let mut next = child_context.clone();
                if !self.add_control_context(node, child_index, &mut next.controls)? {
                    break;
                }
                stack.push((child, next));
            }
        }
        Ok(())
    }

    fn parameter_count(&mut self, node: Node<'_>) -> Result<Option<u32>, ModelError> {
        let Some(parameters) = node.child_by_field_name("parameters") else {
            return Ok(Some(0));
        };
        let mut count = 0_u32;
        for _ in 0..parameters.named_child_count() {
            if self.should_stop()? {
                return Ok(None);
            }
            count = count.saturating_add(1);
        }
        Ok(Some(count))
    }

    fn type_member_count(&mut self, node: Node<'_>) -> Result<Option<u32>, ModelError> {
        let Some(body) = body_node(node) else {
            return Ok(Some(0));
        };
        let mut count = 0_u32;
        for index in 0..body.named_child_count() {
            if self.should_stop()? {
                return Ok(None);
            }
            let Ok(index) = u32::try_from(index) else {
                continue;
            };
            if body.named_child(index).is_some_and(|child| {
                matches!(
                    child.kind(),
                    "field_declaration"
                        | "enum_variant"
                        | "function_item"
                        | "function_signature_item"
                        | "const_item"
                        | "type_item"
                )
            }) {
                count = count.saturating_add(1);
            }
        }
        Ok(Some(count))
    }

    fn syntactic_nesting_depth(&mut self, root: Node<'_>) -> Result<Option<u32>, ModelError> {
        let mut maximum = 0_u32;
        let mut stack = vec![(root, 0_u32)];
        while let Some((node, depth)) = stack.pop() {
            if self.should_stop()? {
                return Ok(None);
            }
            let next_depth = if nesting_node(node) {
                depth.saturating_add(1)
            } else {
                depth
            };
            maximum = maximum.max(next_depth);
            for index in (0..node.named_child_count()).rev() {
                if self.should_stop()? {
                    return Ok(None);
                }
                let Ok(index) = u32::try_from(index) else {
                    continue;
                };
                let Some(child) = node.named_child(index) else {
                    continue;
                };
                if symbol_kind_node(child) {
                    continue;
                }
                stack.push((child, next_depth));
            }
        }
        Ok(Some(maximum))
    }

    fn first_error<'tree>(&mut self, root: Node<'tree>) -> Result<Option<Node<'tree>>, ModelError> {
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if self.should_stop()? {
                return Ok(None);
            }
            if node.is_error() || node.is_missing() {
                return Ok(Some(node));
            }
            for index in (0..node.named_child_count()).rev() {
                if self.should_stop()? {
                    return Ok(None);
                }
                let Ok(index) = u32::try_from(index) else {
                    continue;
                };
                if let Some(child) = node.named_child(index) {
                    stack.push(child);
                }
            }
        }
        Ok(None)
    }

    fn add_control_context(
        &mut self,
        parent: Node<'_>,
        child_index: u32,
        controls: &mut Vec<ControlContext>,
    ) -> Result<bool, ModelError> {
        let field = parent.field_name_for_named_child(child_index);
        match parent.kind() {
            "if_expression" if field == Some("condition") => {
                controls.push(ControlContext::Condition);
            }
            "while_expression" => {
                controls.push(ControlContext::Loop);
                if field == Some("condition") {
                    controls.push(ControlContext::Condition);
                }
            }
            "for_expression" | "loop_expression" => controls.push(ControlContext::Loop),
            "match_arm" => {
                controls.push(ControlContext::MatchArm);
                let Some(is_error) = self.is_err_match_arm(parent)? else {
                    return Ok(false);
                };
                if is_error {
                    controls.push(ControlContext::ErrorBranch);
                }
            }
            "closure_expression" => controls.push(ControlContext::Closure),
            _ => {}
        }
        Ok(true)
    }

    fn is_err_match_arm(&mut self, arm: Node<'_>) -> Result<Option<bool>, ModelError> {
        let Some(pattern) = arm.child_by_field_name("pattern") else {
            return Ok(Some(false));
        };
        let mut stack = vec![pattern];
        while let Some(node) = stack.pop() {
            if self.should_stop()? {
                return Ok(None);
            }
            match node.kind() {
                "tuple_struct_pattern" | "struct_pattern" => {
                    if node
                        .child_by_field_name("type")
                        .is_some_and(|node| path_ends_in_err(node, self.source))
                    {
                        return Ok(Some(true));
                    }
                    continue;
                }
                "identifier" | "scoped_identifier" if path_ends_in_err(node, self.source) => {
                    return Ok(Some(true));
                }
                _ => {}
            }
            for index in (0..node.named_child_count()).rev() {
                if self.should_stop()? {
                    return Ok(None);
                }
                let Ok(index) = u32::try_from(index) else {
                    continue;
                };
                if let Some(child) = node.named_child(index) {
                    stack.push(child);
                }
            }
        }
        Ok(Some(false))
    }

    fn should_stop(&mut self) -> Result<bool, ModelError> {
        if self.truncation.is_some() {
            return Ok(true);
        }
        if let Some(truncation) = self.control.stop_truncation(Instant::now())? {
            self.truncation = Some(truncation);
            return Ok(true);
        }
        Ok(false)
    }
}

struct SymbolSpec {
    kind: SymbolKind,
    name: String,
}

#[derive(Clone, Copy)]
struct SymbolMetrics {
    syntactic_nesting_depth: u32,
    parameter_count: u32,
    type_member_count: u32,
}

fn symbol_spec(node: Node<'_>, context: &WalkContext, source: &str) -> Option<SymbolSpec> {
    let kind = match node.kind() {
        "mod_item" => SymbolKind::Module,
        "struct_item" => SymbolKind::Struct,
        "enum_item" => SymbolKind::Enum,
        "union_item" => SymbolKind::Union,
        "trait_item" => SymbolKind::Trait,
        "impl_item" => SymbolKind::Impl,
        "type_item" => SymbolKind::TypeAlias,
        "const_item" => SymbolKind::Constant,
        "static_item" => SymbolKind::Static,
        "field_declaration" => SymbolKind::Field,
        "enum_variant" => SymbolKind::Variant,
        "function_item" | "function_signature_item" if context.method_parent => SymbolKind::Method,
        "function_item" | "function_signature_item" => SymbolKind::Function,
        _ => return None,
    };
    let name = if kind == SymbolKind::Impl {
        let target = node
            .child_by_field_name("type")
            .and_then(|node| text(node, source))?;
        if let Some(trait_node) = node.child_by_field_name("trait") {
            format!(
                "impl {} for {}",
                text(trait_node, source)?.trim(),
                target.trim()
            )
        } else {
            format!("impl {}", target.trim())
        }
    } else {
        text(node.child_by_field_name("name")?, source)?
            .trim()
            .to_owned()
    };
    Some(SymbolSpec { kind, name })
}

fn make_symbol(
    node: Node<'_>,
    spec: &SymbolSpec,
    context: &WalkContext,
    source: &str,
    provenance: &SyntaxProvenance,
    metrics: SymbolMetrics,
) -> Result<Option<SymbolFact>, ModelError> {
    let Some(full_range) = node_range(node) else {
        return Ok(None);
    };
    let body = body_node(node);
    let body_range = body.and_then(node_range);
    let signature_range = body
        .and_then(|body| {
            range_between(
                node.start_byte(),
                body.start_byte(),
                node.start_position(),
                body.start_position(),
            )
        })
        .unwrap_or(full_range);
    let normalized_signature = text_for_range(signature_range, source)
        .map(normalize)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| spec.name.clone());
    let key = SymbolKey::new(context.path.clone(), spec.kind, spec.name.clone())?;
    SymbolFact::new(
        provenance.clone(),
        key,
        visibility(node, spec.kind, context.member_context, source),
        full_range,
        signature_range,
        body_range,
        normalized_signature,
        metrics.parameter_count,
        metrics.syntactic_nesting_depth,
        metrics.type_member_count,
    )
    .map(Some)
}

fn make_call(
    node: Node<'_>,
    context: &WalkContext,
    source: &str,
    provenance: &SyntaxProvenance,
) -> Result<Option<CallFact>, ModelError> {
    let (callee, arguments) = match node.kind() {
        "call_expression" => (
            node.child_by_field_name("function"),
            node.child_by_field_name("arguments"),
        ),
        "macro_invocation" => (
            node.child_by_field_name("macro"),
            first_named_child_of_kind(node, "token_tree"),
        ),
        _ => return Ok(None),
    };
    let (Some(callee), Some(arguments)) = (callee, arguments) else {
        return Ok(None);
    };
    let Some(argument_range) = node_range(arguments) else {
        return Ok(None);
    };
    let Some(call_site_range) = node_range(node) else {
        return Ok(None);
    };
    let Some(mut callee_text) = text(callee, source) else {
        return Ok(None);
    };
    if node.kind() == "macro_invocation" {
        callee_text.push('!');
    }
    Ok(Some(CallFact::new(
        provenance.clone(),
        callee_text,
        text(arguments, source).unwrap_or_default(),
        argument_range,
        call_site_range,
        context.enclosing_symbol.clone(),
        context.controls.clone(),
    )?))
}

fn body_node(node: Node<'_>) -> Option<Node<'_>> {
    node.child_by_field_name("body").or_else(|| {
        if matches!(node.kind(), "const_item" | "static_item") {
            node.child_by_field_name("value")
        } else {
            None
        }
    })
}

fn member_context(node: Node<'_>, kind: SymbolKind) -> Option<MemberContext> {
    match kind {
        SymbolKind::Trait => Some(MemberContext::Trait),
        SymbolKind::Impl if node.child_by_field_name("trait").is_some() => {
            Some(MemberContext::TraitImpl)
        }
        SymbolKind::Impl => Some(MemberContext::InherentImpl),
        SymbolKind::Variant => Some(MemberContext::Inherited),
        _ => None,
    }
}

fn visibility(
    node: Node<'_>,
    kind: SymbolKind,
    member_context: Option<MemberContext>,
    source: &str,
) -> SymbolVisibility {
    if matches!(kind, SymbolKind::Impl | SymbolKind::Variant)
        || matches!(
            member_context,
            Some(MemberContext::Trait | MemberContext::TraitImpl | MemberContext::Inherited)
        )
    {
        return SymbolVisibility::Unknown;
    }
    let mut cursor = node.walk();
    let modifier = node
        .named_children(&mut cursor)
        .find(|child| child.kind() == "visibility_modifier")
        .and_then(|child| text(child, source));
    match modifier.as_deref().map(str::trim) {
        Some("pub") => SymbolVisibility::Public,
        Some(_) => SymbolVisibility::Restricted,
        None => SymbolVisibility::Private,
    }
}

fn nesting_node(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "if_expression"
            | "match_expression"
            | "while_expression"
            | "for_expression"
            | "loop_expression"
            | "closure_expression"
    )
}

fn symbol_kind_node(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "mod_item"
            | "struct_item"
            | "enum_item"
            | "union_item"
            | "trait_item"
            | "impl_item"
            | "type_item"
            | "const_item"
            | "static_item"
            | "function_item"
            | "function_signature_item"
    )
}

fn path_ends_in_err(mut node: Node<'_>, source: &str) -> bool {
    loop {
        match node.kind() {
            "identifier" => return text(node, source).is_some_and(|name| name == "Err"),
            "scoped_identifier" => {
                let Some(name) = node.child_by_field_name("name") else {
                    return false;
                };
                node = name;
            }
            _ => return false,
        }
    }
}

fn first_named_child_of_kind<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

fn text(node: Node<'_>, source: &str) -> Option<String> {
    source
        .get(node.start_byte()..node.end_byte())
        .map(str::to_owned)
}

fn text_for_range(range: SourceRange, source: &str) -> Option<&str> {
    let start = usize::try_from(range.start_byte()).ok()?;
    let end = usize::try_from(range.end_byte()).ok()?;
    source.get(start..end)
}

fn normalize(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn node_range(node: Node<'_>) -> Option<SourceRange> {
    range_between(
        node.start_byte(),
        node.end_byte(),
        node.start_position(),
        node.end_position(),
    )
}

fn range_between(
    start_byte: usize,
    end_byte: usize,
    start: Point,
    end: Point,
) -> Option<SourceRange> {
    SourceRange::from_tree_sitter(
        u64::try_from(start_byte).ok()?,
        u64::try_from(end_byte).ok()?,
        u32::try_from(start.row).ok()?,
        u32::try_from(end.row).ok()?,
        u32::try_from(end.column).ok()?,
    )
    .ok()
}

fn usize_from_u32(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::{NonZeroU32, NonZeroU64};
    use std::time::Duration;

    fn budget(bytes: u64, symbols: u32, calls: u32) -> AnalysisBudget {
        AnalysisBudget::new(
            NonZeroU64::new(bytes).unwrap(),
            NonZeroU32::new(symbols).unwrap(),
            NonZeroU32::new(calls).unwrap(),
            NonZeroU32::new(64).unwrap(),
        )
    }

    fn analyze(source: &str) -> DocumentStructure {
        RustAdapter::new()
            .analyze(
                AnalysisInput::new("src/lib.rs", SyntaxLanguage::Rust, source.to_owned()).unwrap(),
                budget(100_000, 1_000, 1_000),
                &AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap()),
            )
            .unwrap()
    }

    #[test]
    fn rust_extracts_hierarchy_signatures_visibility_and_ranges() {
        let source = r#"
// Unicode before symbols: žluťoučký
pub(crate) mod outer {
    pub struct Boxed<T>
    where T: Clone
    {
        pub value: T,
    }

    enum Choice { One, Two(u32) }
    union Bits { integer: u32, float: f32 }
    type Alias<T> = Option<T>;
    const LIMIT: usize = 10;
    static ENABLED: bool = true;

    trait Runner<T> {
        fn run(&self, value: T) -> Result<(), ()>;
    }

    impl<T> Runner<T> for Boxed<T> where T: Clone {
        fn run(&self, value: T) -> Result<(), ()> { Ok(()) }
    }

    fn host() { fn café(value: u32) { consume(value); } }
}
"#;
        let document = analyze(source);
        assert_eq!(document.status(), DocumentStatus::Parsed);
        for fact in document.symbols() {
            fact.full_range().validate_source(source).unwrap();
            fact.signature_range().validate_source(source).unwrap();
            if let Some(body) = fact.body_range() {
                body.validate_source(source).unwrap();
            }
        }

        let outer = document
            .symbols()
            .iter()
            .find(|fact| fact.key().name() == "outer")
            .unwrap();
        assert_eq!(outer.visibility(), SymbolVisibility::Restricted);
        let boxed = document
            .symbols()
            .iter()
            .find(|fact| fact.key().name() == "Boxed")
            .unwrap();
        assert!(boxed.normalized_signature().contains("where T: Clone"));
        assert_eq!(boxed.type_member_count(), 1);
        let field = document
            .symbols()
            .iter()
            .find(|fact| fact.key().kind() == SymbolKind::Field)
            .unwrap();
        assert_eq!(field.key().qualified_path(), &["outer", "Boxed"]);
        assert_eq!(field.visibility(), SymbolVisibility::Public);

        let methods: Vec<_> = document
            .symbols()
            .iter()
            .filter(|fact| fact.key().kind() == SymbolKind::Method && fact.key().name() == "run")
            .collect();
        assert_eq!(methods.len(), 2);
        assert_ne!(
            methods[0].key().qualified_path(),
            methods[1].key().qualified_path()
        );
        assert_eq!(methods[0].parameter_count(), 2);
        let nested = document
            .symbols()
            .iter()
            .find(|fact| fact.key().name() == "café")
            .unwrap();
        assert_eq!(nested.key().qualified_path(), &["outer", "host"]);
        assert!(
            document
                .symbols()
                .iter()
                .any(|fact| fact.key().kind() == SymbolKind::Variant)
        );
        for kind in [
            SymbolKind::Module,
            SymbolKind::Struct,
            SymbolKind::Enum,
            SymbolKind::Union,
            SymbolKind::Trait,
            SymbolKind::Impl,
            SymbolKind::TypeAlias,
            SymbolKind::Constant,
            SymbolKind::Static,
        ] {
            assert!(
                document
                    .symbols()
                    .iter()
                    .any(|fact| fact.key().kind() == kind),
                "missing {kind:?}"
            );
        }
    }

    #[test]
    fn rust_extracts_calls_macros_and_conservative_control_contexts() {
        let source = r#"
fn review(result: Result<u32, E>, items: Vec<u32>) {
    if ready() { work!(&items); }
    for item in items() { consume(item); }
    match result {
        Err(error) => report(error),
        Ok(value) => use_value(value),
    }
    let callback = |value| transform(value);
}
"#;
        let document = analyze(source);
        let review = document
            .symbols()
            .iter()
            .find(|fact| fact.key().name() == "review")
            .unwrap();
        assert!(review.syntactic_nesting_depth() >= 1);
        let call = |name: &str| {
            document
                .calls()
                .iter()
                .find(|call| call.callee_text() == name)
                .unwrap()
        };
        assert!(
            call("ready")
                .control_context()
                .contains(&ControlContext::Condition)
        );
        assert!(
            !call("work!")
                .control_context()
                .contains(&ControlContext::Condition)
        );
        assert!(
            call("items")
                .control_context()
                .contains(&ControlContext::Loop)
        );
        assert!(
            call("report")
                .control_context()
                .contains(&ControlContext::MatchArm)
        );
        assert!(
            call("report")
                .control_context()
                .contains(&ControlContext::ErrorBranch)
        );
        assert!(
            call("transform")
                .control_context()
                .contains(&ControlContext::Closure)
        );
        assert_eq!(call("work!").argument_text(), "(&items)");
        for call in document.calls() {
            call.argument_range().validate_source(source).unwrap();
            call.call_site_range().validate_source(source).unwrap();
            assert_eq!(call.enclosing_symbol().unwrap().name(), "review");
        }
    }

    #[test]
    fn rust_does_not_invent_visibility_for_inherited_members() {
        let source = r#"
pub enum PublicChoice { Item { value: u32 } }
pub trait PublicTrait { fn inherited(&self); }
pub struct Value;
impl PublicTrait for Value { fn inherited(&self) {} }
impl Value { pub fn open(&self) {} fn closed(&self) {} }
"#;
        let document = analyze(source);
        let find = |kind, name: &str, parent: &str| {
            document
                .symbols()
                .iter()
                .find(|fact| {
                    fact.key().kind() == kind
                        && fact.key().name() == name
                        && fact
                            .key()
                            .qualified_path()
                            .last()
                            .is_some_and(|segment| segment == parent)
                })
                .unwrap()
        };
        assert_eq!(
            find(SymbolKind::Variant, "Item", "PublicChoice").visibility(),
            SymbolVisibility::Unknown
        );
        assert_eq!(
            find(SymbolKind::Field, "value", "Item").visibility(),
            SymbolVisibility::Unknown
        );
        assert_eq!(
            find(SymbolKind::Method, "inherited", "PublicTrait").visibility(),
            SymbolVisibility::Unknown
        );
        let impls: Vec<_> = document
            .symbols()
            .iter()
            .filter(|fact| fact.key().kind() == SymbolKind::Impl)
            .collect();
        assert_eq!(impls.len(), 2);
        assert!(
            impls
                .iter()
                .all(|fact| fact.visibility() == SymbolVisibility::Unknown)
        );
        assert_eq!(
            find(
                SymbolKind::Method,
                "inherited",
                "impl PublicTrait for Value"
            )
            .visibility(),
            SymbolVisibility::Unknown
        );
        assert_eq!(
            find(SymbolKind::Method, "open", "impl Value").visibility(),
            SymbolVisibility::Public
        );
        assert_eq!(
            find(SymbolKind::Method, "closed", "impl Value").visibility(),
            SymbolVisibility::Private
        );
    }

    #[test]
    fn rust_error_branch_requires_exact_err_path_segment() {
        let source = r#"
fn inspect(value: Value) {
    match value {
        Err(error) => exact(error),
        Result::Err(error) => qualified(error),
        Erratic(error) => wrong_one(error),
        Errata(error) => wrong_two(error),
    }
}
"#;
        let document = analyze(source);
        let has_error_context = |callee: &str| {
            document
                .calls()
                .iter()
                .find(|call| call.callee_text() == callee)
                .unwrap()
                .control_context()
                .contains(&ControlContext::ErrorBranch)
        };
        assert!(has_error_context("exact"));
        assert!(has_error_context("qualified"));
        assert!(!has_error_context("wrong_one"));
        assert!(!has_error_context("wrong_two"));
    }

    #[test]
    fn rust_deep_valid_and_broken_trees_do_not_use_the_call_stack() {
        let depth = 2_000;
        let mut valid = String::from("fn deep() {");
        valid.push_str(&"if true {".repeat(depth));
        valid.push_str("work();");
        valid.push_str(&"}".repeat(depth + 1));
        let document = analyze(&valid);
        assert_eq!(document.status(), DocumentStatus::Parsed);
        assert_eq!(
            document
                .symbols()
                .iter()
                .find(|symbol| symbol.key().name() == "deep")
                .unwrap()
                .syntactic_nesting_depth(),
            u32::try_from(depth).unwrap()
        );
        assert!(
            document
                .calls()
                .iter()
                .any(|call| call.callee_text() == "work")
        );

        let mut broken = String::from("fn deep() {");
        broken.push_str(&"if true {".repeat(depth));
        broken.push_str("work();");
        broken.push_str(&"}".repeat(depth));
        let document = analyze(&broken);
        assert_eq!(document.status(), DocumentStatus::Partial);
        assert!(
            document
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
        );
    }

    #[test]
    fn rust_engine_checks_stop_control_during_large_heap_walk() {
        let mut source = String::from("fn work() {");
        source.push_str(&"call();".repeat(50_000));
        source.push('}');
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(&source, None).unwrap();

        let cancelled = AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap());
        let cancellation_signal = cancelled.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1));
            cancellation_signal.cancel();
        });
        let mut engine = Engine {
            source: &source,
            provenance: provenance().unwrap(),
            budget: budget(1_000_000, 100_000, 100_000),
            control: &cancelled,
            symbols: Vec::new(),
            calls: Vec::new(),
            truncation: None,
        };
        engine.extract(tree.root_node()).unwrap();
        canceller.join().unwrap();
        assert_eq!(
            engine.truncation.unwrap().reason(),
            SyntaxTruncationReason::Cancelled
        );

        let expired = AnalysisControl::new(NonZeroU64::new(500).unwrap());
        let mut engine = Engine {
            source: &source,
            provenance: provenance().unwrap(),
            budget: budget(1_000_000, 100_000, 100_000),
            control: &expired,
            symbols: Vec::new(),
            calls: Vec::new(),
            truncation: None,
        };
        engine.extract(tree.root_node()).unwrap();
        assert_eq!(
            engine.truncation.unwrap().reason(),
            SyntaxTruncationReason::Time
        );
    }

    #[test]
    fn rust_broken_syntax_is_partial_with_error_evidence() {
        let document = analyze("fn broken( { let value = ; }");
        assert_eq!(document.status(), DocumentStatus::Partial);
        assert!(document.truncation().is_none());
        assert!(
            document
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.severity() == DiagnosticSeverity::Error)
        );
    }

    #[test]
    fn rust_honors_source_symbol_and_call_budgets() {
        let adapter = RustAdapter::new();
        let control = AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap());
        let input = || {
            AnalysisInput::new(
                "src/lib.rs",
                SyntaxLanguage::Rust,
                "fn one() { first(); second(); } fn two() {}".to_owned(),
            )
            .unwrap()
        };
        let source_limited = adapter
            .analyze(input(), budget(10, 10, 10), &control)
            .unwrap();
        assert_eq!(
            source_limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::SourceBytes
        );
        let symbol_limited = adapter
            .analyze(input(), budget(1_000, 1, 10), &control)
            .unwrap();
        assert_eq!(symbol_limited.symbols().len(), 1);
        assert_eq!(
            symbol_limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::SymbolCount
        );
        let call_limited = adapter
            .analyze(input(), budget(1_000, 10, 1), &control)
            .unwrap();
        assert_eq!(call_limited.calls().len(), 1);
        assert_eq!(
            call_limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::CallCount
        );
    }

    #[test]
    fn rust_honors_cancellation_and_deadline() {
        let adapter = RustAdapter::new();
        let input = || {
            AnalysisInput::new(
                "src/lib.rs",
                SyntaxLanguage::Rust,
                "fn work() {}".to_owned(),
            )
            .unwrap()
        };
        let cancelled = AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap());
        cancelled.cancel();
        let document = adapter
            .analyze(input(), budget(1_000, 10, 10), &cancelled)
            .unwrap();
        assert_eq!(
            document.truncation().unwrap().reason(),
            SyntaxTruncationReason::Cancelled
        );

        let expired = AnalysisControl::new(NonZeroU64::new(1_000).unwrap());
        std::thread::sleep(Duration::from_millis(3));
        let document = adapter
            .analyze(input(), budget(1_000, 10, 10), &expired)
            .unwrap();
        let truncation = document.truncation().unwrap();
        assert_eq!(truncation.reason(), SyntaxTruncationReason::Time);
        assert_eq!(truncation.limit(), Some(1_000));
        assert!(truncation.observed().unwrap() >= 1_000);
    }
}
