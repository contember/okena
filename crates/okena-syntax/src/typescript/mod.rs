//! Deterministic TypeScript and TSX tree-sitter adapter.

use std::collections::{HashMap, HashSet};
use std::ops::ControlFlow;
use std::time::Instant;

use tree_sitter::{Node, Parser};

use crate::{
    AnalysisBudget, AnalysisControl, AnalysisInput, CallFact, CaptureByteTracker, ControlContext,
    DiagnosticSeverity, DocumentStatus, DocumentStructure, ModelError, SourceRange, SymbolFact,
    SymbolKey, SymbolKind, SymbolVisibility, SyntaxAdapter, SyntaxDiagnostic, SyntaxLanguage,
    SyntaxProvenance, SyntaxTruncation, SyntaxTruncationReason,
};

const TYPESCRIPT_PARSER: &str = "tree-sitter-typescript@0.23.2";
const TSX_PARSER: &str = "tree-sitter-tsx@0.23.2";

/// Syntax adapter for both the TypeScript and TSX grammars.
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeScriptAdapter;

impl TypeScriptAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl SyntaxAdapter for TypeScriptAdapter {
    fn language(&self) -> SyntaxLanguage {
        SyntaxLanguage::TypeScript
    }

    fn supports(&self, language: SyntaxLanguage) -> bool {
        matches!(language, SyntaxLanguage::TypeScript | SyntaxLanguage::Tsx)
    }

    fn analyze(
        &self,
        input: AnalysisInput,
        budget: AnalysisBudget,
        control: &AnalysisControl,
    ) -> Result<DocumentStructure, ModelError> {
        let language = input.language();
        let parser_name = match language {
            SyntaxLanguage::TypeScript => TYPESCRIPT_PARSER,
            SyntaxLanguage::Tsx => TSX_PARSER,
            SyntaxLanguage::Rust => {
                return unsupported_document(&input, "typescript-adapter");
            }
        };
        let provenance = SyntaxProvenance::tree_sitter(language, parser_name)?;

        if let Some(truncation) = control.stop_truncation(Instant::now())? {
            return partial_document(
                &input,
                provenance,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                truncation,
            );
        }
        let source_bytes = u64::try_from(input.source().len()).unwrap_or(u64::MAX);
        let source_limit = budget.max_source_bytes().get();
        if source_bytes > source_limit {
            return partial_document(
                &input,
                provenance,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                SyntaxTruncation::new(
                    SyntaxTruncationReason::SourceBytes,
                    Some(source_limit),
                    Some(source_bytes),
                )?,
            );
        }
        let mut capture = match CaptureByteTracker::for_document(
            budget.max_capture_bytes(),
            input.path(),
            &provenance,
        ) {
            Ok(capture) => capture,
            Err(truncation) => {
                return partial_document(
                    &input,
                    provenance,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    truncation,
                );
            }
        };
        let mut parser = Parser::new();
        let grammar = match language {
            SyntaxLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SyntaxLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            SyntaxLanguage::Rust => {
                return unsupported_document(&input, "typescript-adapter");
            }
        };
        if let Err(error) = parser.set_language(&grammar) {
            return failed_document(
                &input,
                provenance,
                format!("failed to load {parser_name}: {error}"),
                &mut capture,
                control,
            );
        }

        let source = input.source().as_bytes();
        let mut read = |offset: usize, _| source.get(offset..).unwrap_or_default();
        let mut progress = |_: &tree_sitter::ParseState| {
            if control.should_stop(Instant::now()) {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let options = tree_sitter::ParseOptions::new().progress_callback(&mut progress);
        let Some(tree) = parser.parse_with_options(&mut read, None, Some(options)) else {
            if let Some(truncation) = control.stop_truncation(Instant::now())? {
                return partial_document(
                    &input,
                    provenance,
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    truncation,
                );
            }
            return failed_document(
                &input,
                provenance,
                "tree-sitter returned no syntax tree",
                &mut capture,
                control,
            );
        };

        let (exported_names, export_scan_truncation) =
            collect_exported_names(tree.root_node(), input.source(), control)?;
        if let Some(truncation) = export_scan_truncation {
            return partial_document(
                &input,
                provenance,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                truncation,
            );
        }
        let mut extractor = Extractor::new(
            input.source(),
            provenance.clone(),
            budget,
            control,
            exported_names,
            capture,
        );
        extractor.extract(tree.root_node())?;

        let diagnostics = if extractor.truncation.is_none() {
            let (diagnostics, truncation) = parse_diagnostics(
                tree.root_node(),
                control,
                budget.max_diagnostics().get(),
                &mut extractor.capture,
            )?;
            extractor.truncation = truncation;
            diagnostics
        } else {
            Vec::new()
        };

        let truncation = extractor.truncation;
        let status = if truncation.is_some() || !diagnostics.is_empty() {
            DocumentStatus::Partial
        } else {
            DocumentStatus::Parsed
        };
        let retained_bytes = extractor.capture.retained_bytes();
        let document = DocumentStructure::new(
            input.path(),
            provenance,
            status,
            extractor.symbols,
            extractor.calls,
            diagnostics,
            truncation,
        )?;
        debug_assert_eq!(document.estimated_owned_bytes(), retained_bytes);
        Ok(document)
    }
}

fn unsupported_document(
    input: &AnalysisInput,
    parser: &str,
) -> Result<DocumentStructure, ModelError> {
    DocumentStructure::new(
        input.path(),
        SyntaxProvenance::tree_sitter(input.language(), parser)?,
        DocumentStatus::Unsupported,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        None,
    )
}

fn failed_document(
    input: &AnalysisInput,
    provenance: SyntaxProvenance,
    message: impl Into<String>,
    capture: &mut CaptureByteTracker,
    control: &AnalysisControl,
) -> Result<DocumentStructure, ModelError> {
    if let Some(truncation) = control.stop_truncation(Instant::now())? {
        return partial_document(
            input,
            provenance,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            truncation,
        );
    }
    let diagnostic = SyntaxDiagnostic::new(DiagnosticSeverity::Error, message, None)?;
    if let Err(truncation) = capture.try_account_diagnostic(&diagnostic) {
        return partial_document(
            input,
            provenance,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            truncation,
        );
    }
    let retained_bytes = capture.retained_bytes();
    let document = DocumentStructure::new(
        input.path(),
        provenance,
        DocumentStatus::Failed,
        Vec::new(),
        Vec::new(),
        vec![diagnostic],
        None,
    )?;
    debug_assert_eq!(document.estimated_owned_bytes(), retained_bytes);
    Ok(document)
}

fn partial_document(
    input: &AnalysisInput,
    provenance: SyntaxProvenance,
    symbols: Vec<SymbolFact>,
    calls: Vec<CallFact>,
    diagnostics: Vec<SyntaxDiagnostic>,
    truncation: SyntaxTruncation,
) -> Result<DocumentStructure, ModelError> {
    DocumentStructure::new(
        input.path(),
        provenance,
        DocumentStatus::Partial,
        symbols,
        calls,
        diagnostics,
        Some(truncation),
    )
}

#[derive(Clone)]
struct WorkItem<'tree> {
    node: Node<'tree>,
    parent_path: Vec<String>,
    enclosing_symbol: Option<SymbolKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ScopeKey {
    start_byte: usize,
    end_byte: usize,
}

impl ScopeKey {
    fn new(node: Node<'_>) -> Self {
        Self {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
        }
    }
}

type ExportedNames = HashMap<ScopeKey, HashSet<String>>;

struct Extractor<'source, 'control> {
    source: &'source str,
    provenance: SyntaxProvenance,
    budget: AnalysisBudget,
    control: &'control AnalysisControl,
    exported_names: ExportedNames,
    symbols: Vec<SymbolFact>,
    calls: Vec<CallFact>,
    truncation: Option<SyntaxTruncation>,
    capture: CaptureByteTracker,
}

impl<'source, 'control> Extractor<'source, 'control> {
    fn new(
        source: &'source str,
        provenance: SyntaxProvenance,
        budget: AnalysisBudget,
        control: &'control AnalysisControl,
        exported_names: ExportedNames,
        capture: CaptureByteTracker,
    ) -> Self {
        Self {
            source,
            provenance,
            budget,
            control,
            exported_names,
            symbols: Vec::new(),
            calls: Vec::new(),
            truncation: None,
            capture,
        }
    }

    fn extract(&mut self, root: Node<'_>) -> Result<(), ModelError> {
        let mut stack = vec![WorkItem {
            node: root,
            parent_path: Vec::new(),
            enclosing_symbol: None,
        }];
        while let Some(item) = stack.pop() {
            if self.should_stop()? {
                break;
            }

            let mut child_path = item.parent_path.clone();
            let mut child_enclosing = item.enclosing_symbol.clone();
            if let Some(spec) = symbol_spec(item.node, self.source, &item.parent_path)? {
                let Some(fact) = self.build_symbol(item.node, &spec)? else {
                    break;
                };
                let key = fact.key().clone();
                if !self.push_symbol(fact)? {
                    break;
                }
                child_path.push(spec.name);
                child_enclosing = Some(key);
            }

            if item.node.kind() == "call_expression"
                && let Some(call) = self.build_call(item.node, item.enclosing_symbol)?
                && !self.push_call(call)?
            {
                break;
            }
            if self.truncation.is_some() {
                break;
            }

            for index in (0..item.node.named_child_count()).rev() {
                if self.should_stop()? {
                    break;
                }
                let Some(child) = named_child(item.node, index) else {
                    continue;
                };
                stack.push(WorkItem {
                    node: child,
                    parent_path: child_path.clone(),
                    enclosing_symbol: child_enclosing.clone(),
                });
            }
        }
        Ok(())
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

    fn push_symbol(&mut self, fact: SymbolFact) -> Result<bool, ModelError> {
        if self.should_stop()? {
            return Ok(false);
        }
        let limit = usize::try_from(self.budget.max_symbols().get()).unwrap_or(usize::MAX);
        if self.symbols.len() >= limit {
            let observed = u64::try_from(self.symbols.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            self.truncation = Some(SyntaxTruncation::new(
                SyntaxTruncationReason::SymbolCount,
                Some(self.budget.max_symbols().get().into()),
                Some(observed),
            )?);
            return Ok(false);
        }
        if let Err(truncation) = self.capture.try_account_symbol(&fact) {
            self.truncation = Some(truncation);
            return Ok(false);
        }
        self.symbols.push(fact);
        Ok(true)
    }

    fn push_call(&mut self, fact: CallFact) -> Result<bool, ModelError> {
        if self.should_stop()? {
            return Ok(false);
        }
        let limit = usize::try_from(self.budget.max_calls().get()).unwrap_or(usize::MAX);
        if self.calls.len() >= limit {
            let observed = u64::try_from(self.calls.len())
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            self.truncation = Some(SyntaxTruncation::new(
                SyntaxTruncationReason::CallCount,
                Some(self.budget.max_calls().get().into()),
                Some(observed),
            )?);
            return Ok(false);
        }
        if let Err(truncation) = self.capture.try_account_call(&fact) {
            self.truncation = Some(truncation);
            return Ok(false);
        }
        self.calls.push(fact);
        Ok(true)
    }

    fn build_symbol(
        &mut self,
        node: Node<'_>,
        spec: &SymbolSpec,
    ) -> Result<Option<SymbolFact>, ModelError> {
        let Some(full_node) = self.evidence_node(node)? else {
            return Ok(None);
        };
        let full_range = node_range(full_node)?;
        let signature_range = if let Some(body) = spec.body {
            range_between(full_node, body)?
        } else {
            full_range
        };
        let signature = source_text(self.source, signature_range)?;
        let normalized_signature = normalize_signature(signature);
        let body_range = spec.body.map(node_range).transpose()?;
        let Some(visibility) = self.visibility(node)? else {
            return Ok(None);
        };
        let Some(parameter_count) = self.parameter_count(spec.parameters)? else {
            return Ok(None);
        };
        let Some(syntactic_nesting_depth) = self.max_nesting_depth(spec.body)? else {
            return Ok(None);
        };
        let Some(type_member_count) = self.type_member_count(spec.body)? else {
            return Ok(None);
        };
        SymbolFact::new(
            self.provenance.clone(),
            SymbolKey::new(spec.parent_path.clone(), spec.kind, spec.name.clone())?,
            visibility,
            full_range,
            signature_range,
            body_range,
            normalized_signature,
            parameter_count,
            syntactic_nesting_depth,
            type_member_count,
        )
        .map(Some)
    }

    fn build_call(
        &mut self,
        node: Node<'_>,
        enclosing_symbol: Option<SymbolKey>,
    ) -> Result<Option<CallFact>, ModelError> {
        if self.should_stop()? {
            return Ok(None);
        }
        let Some(function) = node.child_by_field_name("function") else {
            return Ok(None);
        };
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return Ok(None);
        };
        let Some(contexts) = self.call_contexts(node)? else {
            return Ok(None);
        };
        Ok(Some(CallFact::new(
            self.provenance.clone(),
            node_text(self.source, function)?.to_string(),
            node_text(self.source, arguments)?.to_string(),
            node_range(arguments)?,
            node_range(node)?,
            enclosing_symbol,
            contexts,
        )?))
    }

    fn evidence_node<'tree>(
        &mut self,
        node: Node<'tree>,
    ) -> Result<Option<Node<'tree>>, ModelError> {
        let mut evidence = node;
        if node.kind() == "variable_declarator"
            && let Some(declaration) = node.parent().filter(|parent| {
                matches!(
                    parent.kind(),
                    "lexical_declaration" | "variable_declaration"
                )
            })
        {
            let mut declarators = 0_u32;
            for index in 0..declaration.named_child_count() {
                if self.should_stop()? {
                    return Ok(None);
                }
                if named_child(declaration, index)
                    .is_some_and(|child| child.kind() == "variable_declarator")
                {
                    declarators = declarators.saturating_add(1);
                }
            }
            if declarators > 1 {
                return Ok(Some(node));
            }
            evidence = declaration;
        }
        while let Some(parent) = evidence.parent() {
            if self.should_stop()? {
                return Ok(None);
            }
            if matches!(parent.kind(), "ambient_declaration" | "export_statement") {
                evidence = parent;
            } else {
                break;
            }
        }
        Ok(Some(evidence))
    }

    fn visibility(&mut self, node: Node<'_>) -> Result<Option<SymbolVisibility>, ModelError> {
        if node
            .child_by_field_name("name")
            .is_some_and(|name| name.kind() == "private_property_identifier")
        {
            return Ok(Some(SymbolVisibility::Private));
        }
        for index in 0..node.named_child_count() {
            if self.should_stop()? {
                return Ok(None);
            }
            let Some(child) = named_child(node, index) else {
                continue;
            };
            if child.kind() != "accessibility_modifier" {
                continue;
            }
            return Ok(Some(match node_text(self.source, child).ok() {
                Some("private") => SymbolVisibility::Private,
                Some("protected") => SymbolVisibility::Restricted,
                _ => SymbolVisibility::Public,
            }));
        }
        let mut current = node.parent();
        while let Some(candidate) = current {
            if self.should_stop()? {
                return Ok(None);
            }
            if candidate.kind() == "export_statement" {
                return Ok(Some(SymbolVisibility::Exported));
            }
            if is_nested_symbol_boundary(candidate.kind()) {
                break;
            }
            current = candidate.parent();
        }
        let declaration_scope = self.declaration_scope(node)?;
        if self.truncation.is_some() {
            return Ok(None);
        }
        if let Some(scope) = declaration_scope
            && let Some(name) = node
                .child_by_field_name("name")
                .and_then(|name| node_text(self.source, name).ok())
            && self
                .exported_names
                .get(&scope)
                .is_some_and(|names| names.contains(name))
        {
            return Ok(Some(SymbolVisibility::Exported));
        }
        Ok(Some(match node.kind() {
            "method_definition"
            | "method_signature"
            | "abstract_method_signature"
            | "public_field_definition"
            | "property_signature"
            | "enum_assignment" => SymbolVisibility::Public,
            "property_identifier" | "number" | "string" | "computed_property_name"
                if node
                    .parent()
                    .is_some_and(|parent| parent.kind() == "enum_body") =>
            {
                SymbolVisibility::Public
            }
            _ => SymbolVisibility::Private,
        }))
    }

    fn declaration_scope(&mut self, node: Node<'_>) -> Result<Option<ScopeKey>, ModelError> {
        let mut current = node;
        if current.kind() == "variable_declarator"
            && let Some(declaration) = current.parent()
            && matches!(
                declaration.kind(),
                "lexical_declaration" | "variable_declaration"
            )
        {
            current = declaration;
        }
        while let Some(parent) = current.parent() {
            if self.should_stop()? {
                return Ok(None);
            }
            if matches!(parent.kind(), "ambient_declaration" | "export_statement") {
                current = parent;
                continue;
            }
            if parent.kind() == "program" {
                return Ok(Some(ScopeKey::new(parent)));
            }
            if parent.kind() == "statement_block"
                && let Some(owner) = parent.parent()
                && matches!(owner.kind(), "internal_module" | "module")
            {
                return Ok(Some(ScopeKey::new(owner)));
            }
            return Ok(None);
        }
        Ok(None)
    }

    fn parameter_count(&mut self, parameters: Option<Node<'_>>) -> Result<Option<u32>, ModelError> {
        let Some(parameters) = parameters else {
            return Ok(Some(0));
        };
        if parameters.kind() == "identifier" {
            return Ok(Some(1));
        }
        let mut count = 0_u32;
        for index in 0..parameters.named_child_count() {
            if self.should_stop()? {
                return Ok(None);
            }
            if named_child(parameters, index).is_some_and(|parameter| {
                matches!(
                    parameter.kind(),
                    "required_parameter" | "optional_parameter"
                )
            }) {
                count = count.saturating_add(1);
            }
        }
        Ok(Some(count))
    }

    fn type_member_count(&mut self, body: Option<Node<'_>>) -> Result<Option<u32>, ModelError> {
        let Some(body) = body else {
            return Ok(Some(0));
        };
        if !matches!(
            body.kind(),
            "class_body" | "interface_body" | "object_type" | "enum_body"
        ) {
            return Ok(Some(0));
        }
        let mut count = 0_u32;
        for index in 0..body.named_child_count() {
            if self.should_stop()? {
                return Ok(None);
            }
            if named_child(body, index).is_some_and(|child| {
                matches!(
                    child.kind(),
                    "method_definition"
                        | "method_signature"
                        | "abstract_method_signature"
                        | "public_field_definition"
                        | "property_signature"
                        | "call_signature"
                        | "construct_signature"
                        | "index_signature"
                        | "enum_assignment"
                        | "property_identifier"
                        | "number"
                        | "string"
                        | "computed_property_name"
                )
            }) {
                count = count.saturating_add(1);
            }
        }
        Ok(Some(count))
    }

    fn max_nesting_depth(&mut self, root: Option<Node<'_>>) -> Result<Option<u32>, ModelError> {
        let Some(root) = root else {
            return Ok(Some(0));
        };
        let mut maximum = 0_u32;
        let mut stack = vec![(root, 0_u32)];
        while let Some((node, depth)) = stack.pop() {
            if self.should_stop()? {
                return Ok(None);
            }
            if node != root && is_nested_symbol_boundary(node.kind()) {
                continue;
            }
            let next_depth = if is_nesting_node(node.kind()) {
                depth.saturating_add(1)
            } else {
                depth
            };
            maximum = maximum.max(next_depth);
            for index in (0..node.named_child_count()).rev() {
                if self.should_stop()? {
                    return Ok(None);
                }
                if let Some(child) = named_child(node, index) {
                    stack.push((child, next_depth));
                }
            }
        }
        Ok(Some(maximum))
    }

    fn call_contexts(&mut self, node: Node<'_>) -> Result<Option<Vec<ControlContext>>, ModelError> {
        let mut condition = false;
        let mut loop_context = false;
        let mut match_arm = false;
        let mut error_branch = false;
        let mut callback = false;
        let mut closure = false;
        let mut child = node;
        let mut current = node.parent();
        while let Some(ancestor) = current {
            if self.should_stop()? {
                return Ok(None);
            }
            match ancestor.kind() {
                "if_statement" | "while_statement" | "do_statement" | "for_statement"
                    if ancestor
                        .child_by_field_name("condition")
                        .is_some_and(|range| range_contains(range, child)) =>
                {
                    condition = true;
                }
                "conditional_expression"
                    if ancestor
                        .child_by_field_name("condition")
                        .is_some_and(|range| range_contains(range, child)) =>
                {
                    condition = true;
                }
                "for_in_statement" | "for_of_statement" => loop_context = true,
                "switch_case" | "switch_default" => match_arm = true,
                "catch_clause" => error_branch = true,
                "arrow_function" | "function_expression" => {
                    closure = true;
                    let Some(is_callback) = self.is_callback_function(ancestor)? else {
                        return Ok(None);
                    };
                    if is_callback {
                        callback = true;
                    }
                }
                _ => {}
            }
            if matches!(ancestor.kind(), "for_statement") {
                loop_context = true;
            }
            if matches!(ancestor.kind(), "while_statement" | "do_statement") {
                loop_context = true;
            }
            child = ancestor;
            current = ancestor.parent();
        }
        let mut contexts = Vec::new();
        if condition {
            contexts.push(ControlContext::Condition);
        }
        if loop_context {
            contexts.push(ControlContext::Loop);
        }
        if match_arm {
            contexts.push(ControlContext::MatchArm);
        }
        if error_branch {
            contexts.push(ControlContext::ErrorBranch);
        }
        if callback {
            contexts.push(ControlContext::Callback);
        }
        if closure {
            contexts.push(ControlContext::Closure);
        }
        Ok(Some(contexts))
    }

    fn is_callback_function(&mut self, node: Node<'_>) -> Result<Option<bool>, ModelError> {
        let mut current = node.parent();
        while let Some(parent) = current {
            if self.should_stop()? {
                return Ok(None);
            }
            if parent.kind() == "arguments" {
                return Ok(Some(
                    parent
                        .parent()
                        .is_some_and(|node| node.kind() == "call_expression"),
                ));
            }
            if is_nested_symbol_boundary(parent.kind()) || parent.kind() == "statement_block" {
                return Ok(Some(false));
            }
            current = parent.parent();
        }
        Ok(Some(false))
    }
}

struct SymbolSpec<'tree> {
    name: String,
    kind: SymbolKind,
    parent_path: Vec<String>,
    parameters: Option<Node<'tree>>,
    body: Option<Node<'tree>>,
}

fn symbol_spec<'tree>(
    node: Node<'tree>,
    source: &str,
    parent_path: &[String],
) -> Result<Option<SymbolSpec<'tree>>, ModelError> {
    let direct = match node.kind() {
        "function_declaration" | "function_signature" | "generator_function_declaration" => {
            Some(SymbolKind::Function)
        }
        "class_declaration" | "abstract_class_declaration" => Some(SymbolKind::Class),
        "interface_declaration" => Some(SymbolKind::Interface),
        "enum_declaration" => Some(SymbolKind::Enum),
        "internal_module" | "module" => Some(SymbolKind::Module),
        "method_definition" | "method_signature" | "abstract_method_signature" => {
            Some(SymbolKind::Method)
        }
        "type_alias_declaration" => Some(SymbolKind::TypeAlias),
        "public_field_definition" | "property_signature" => Some(SymbolKind::Field),
        "enum_assignment" => Some(SymbolKind::Variant),
        "property_identifier" | "number" | "string" | "computed_property_name"
            if node
                .parent()
                .is_some_and(|parent| parent.kind() == "enum_body") =>
        {
            Some(SymbolKind::Variant)
        }
        _ => None,
    };
    if let Some(kind) = direct {
        let name_node = if kind == SymbolKind::Variant
            && node.kind() != "enum_assignment"
            && node
                .parent()
                .is_some_and(|parent| parent.kind() == "enum_body")
        {
            node
        } else {
            let Some(name) = node.child_by_field_name("name") else {
                return Ok(None);
            };
            name
        };
        let name = node_text(source, name_node)?.trim().to_string();
        if name.is_empty() {
            return Ok(None);
        }
        return Ok(Some(SymbolSpec {
            name,
            kind,
            parent_path: parent_path.to_vec(),
            parameters: node.child_by_field_name("parameters"),
            body: match node.kind() {
                "public_field_definition" | "enum_assignment" => node.child_by_field_name("value"),
                _ => node.child_by_field_name("body"),
            },
        }));
    }

    if node.kind() != "variable_declarator" {
        return Ok(None);
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        return Ok(None);
    };
    if name_node.kind() != "identifier" {
        return Ok(None);
    }
    let Some(value) = node.child_by_field_name("value") else {
        return Ok(None);
    };
    if !matches!(
        value.kind(),
        "arrow_function" | "function_expression" | "generator_function"
    ) {
        return Ok(None);
    }
    let name = node_text(source, name_node)?.to_string();
    let parameters = value
        .child_by_field_name("parameters")
        .or_else(|| value.child_by_field_name("parameter"));
    Ok(Some(SymbolSpec {
        name,
        kind: SymbolKind::Function,
        parent_path: parent_path.to_vec(),
        parameters,
        body: value.child_by_field_name("body"),
    }))
}

fn collect_exported_names(
    root: Node<'_>,
    source: &str,
    control: &AnalysisControl,
) -> Result<(ExportedNames, Option<SyntaxTruncation>), ModelError> {
    let mut exported = HashMap::<ScopeKey, HashSet<String>>::new();
    let mut stack = vec![(root, ScopeKey::new(root))];
    while let Some((node, scope)) = stack.pop() {
        if let Some(truncation) = control.stop_truncation(Instant::now())? {
            return Ok((exported, Some(truncation)));
        }
        if node.kind() == "export_statement" && node.child_by_field_name("source").is_none() {
            for index in 0..node.named_child_count() {
                if let Some(truncation) = control.stop_truncation(Instant::now())? {
                    return Ok((exported, Some(truncation)));
                }
                let Some(clause) = named_child(node, index) else {
                    continue;
                };
                if clause.kind() != "export_clause" {
                    continue;
                }
                for specifier_index in 0..clause.named_child_count() {
                    if let Some(truncation) = control.stop_truncation(Instant::now())? {
                        return Ok((exported, Some(truncation)));
                    }
                    let Some(specifier) = named_child(clause, specifier_index) else {
                        continue;
                    };
                    if specifier.kind() != "export_specifier" {
                        continue;
                    }
                    if let Some(name) = specifier.child_by_field_name("name")
                        && name.kind() == "identifier"
                        && let Ok(name) = node_text(source, name)
                    {
                        exported.entry(scope).or_default().insert(name.to_string());
                    }
                }
            }
        }
        let child_scope = if matches!(node.kind(), "internal_module" | "module") {
            ScopeKey::new(node)
        } else {
            scope
        };
        for index in (0..node.named_child_count()).rev() {
            if let Some(truncation) = control.stop_truncation(Instant::now())? {
                return Ok((exported, Some(truncation)));
            }
            if let Some(child) = named_child(node, index) {
                stack.push((child, child_scope));
            }
        }
    }
    Ok((exported, None))
}

fn is_nested_symbol_boundary(kind: &str) -> bool {
    matches!(
        kind,
        "function_declaration"
            | "function_expression"
            | "function_signature"
            | "generator_function_declaration"
            | "generator_function"
            | "arrow_function"
            | "method_definition"
            | "method_signature"
            | "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "internal_module"
            | "module"
    )
}

fn is_nesting_node(kind: &str) -> bool {
    matches!(
        kind,
        "if_statement"
            | "switch_statement"
            | "switch_case"
            | "switch_default"
            | "for_statement"
            | "for_in_statement"
            | "while_statement"
            | "do_statement"
            | "try_statement"
            | "catch_clause"
            | "conditional_expression"
    )
}

fn range_contains(outer: Node<'_>, inner: Node<'_>) -> bool {
    outer.start_byte() <= inner.start_byte() && inner.end_byte() <= outer.end_byte()
}

fn named_child(node: Node<'_>, index: usize) -> Option<Node<'_>> {
    u32::try_from(index)
        .ok()
        .and_then(|index| node.named_child(index))
}

fn parse_diagnostics(
    root: Node<'_>,
    control: &AnalysisControl,
    diagnostic_limit: u32,
    capture: &mut CaptureByteTracker,
) -> Result<(Vec<SyntaxDiagnostic>, Option<SyntaxTruncation>), ModelError> {
    if !root.has_error() {
        return Ok((Vec::new(), control.stop_truncation(Instant::now())?));
    }
    let mut diagnostics = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(truncation) = control.stop_truncation(Instant::now())? {
            return Ok((diagnostics, Some(truncation)));
        }
        if node.is_error() || node.is_missing() {
            if diagnostics.len() >= usize::try_from(diagnostic_limit).unwrap_or(usize::MAX) {
                return Ok((
                    diagnostics,
                    Some(SyntaxTruncation::new(
                        SyntaxTruncationReason::DiagnosticCount,
                        Some(u64::from(diagnostic_limit)),
                        Some(u64::from(diagnostic_limit).saturating_add(1)),
                    )?),
                ));
            }
            let message = if node.is_missing() {
                format!("tree-sitter inserted missing {}", node.kind())
            } else {
                "tree-sitter recovered from invalid syntax".to_string()
            };
            let diagnostic = SyntaxDiagnostic::new(
                DiagnosticSeverity::Warning,
                message,
                Some(node_range(node)?),
            )?;
            if let Some(truncation) = control.stop_truncation(Instant::now())? {
                return Ok((diagnostics, Some(truncation)));
            }
            if let Err(truncation) = capture.try_account_diagnostic(&diagnostic) {
                return Ok((diagnostics, Some(truncation)));
            }
            diagnostics.push(diagnostic);
            continue;
        }
        for index in (0..node.named_child_count()).rev() {
            if let Some(truncation) = control.stop_truncation(Instant::now())? {
                return Ok((diagnostics, Some(truncation)));
            }
            if let Some(child) = named_child(node, index) {
                stack.push(child);
            }
        }
    }
    if diagnostics.is_empty() {
        if let Some(truncation) = control.stop_truncation(Instant::now())? {
            return Ok((diagnostics, Some(truncation)));
        }
        let diagnostic = SyntaxDiagnostic::new(
            DiagnosticSeverity::Warning,
            "tree-sitter reported an incomplete syntax tree",
            Some(node_range(root)?),
        )?;
        if let Some(truncation) = control.stop_truncation(Instant::now())? {
            return Ok((diagnostics, Some(truncation)));
        }
        if let Err(truncation) = capture.try_account_diagnostic(&diagnostic) {
            return Ok((diagnostics, Some(truncation)));
        }
        diagnostics.push(diagnostic);
    }
    Ok((diagnostics, None))
}

fn node_range(node: Node<'_>) -> Result<SourceRange, ModelError> {
    SourceRange::from_tree_sitter(
        u64::try_from(node.start_byte()).unwrap_or(u64::MAX),
        u64::try_from(node.end_byte()).unwrap_or(u64::MAX),
        u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
        u32::try_from(node.end_position().row).unwrap_or(u32::MAX),
        u32::try_from(node.end_position().column).unwrap_or(u32::MAX),
    )
}

fn range_between(start: Node<'_>, end: Node<'_>) -> Result<SourceRange, ModelError> {
    SourceRange::from_tree_sitter(
        u64::try_from(start.start_byte()).unwrap_or(u64::MAX),
        u64::try_from(end.start_byte()).unwrap_or(u64::MAX),
        u32::try_from(start.start_position().row).unwrap_or(u32::MAX),
        u32::try_from(end.start_position().row).unwrap_or(u32::MAX),
        u32::try_from(end.start_position().column).unwrap_or(u32::MAX),
    )
}

fn node_text<'a>(source: &'a str, node: Node<'_>) -> Result<&'a str, ModelError> {
    source_text(source, node_range(node)?)
}

fn source_text(source: &str, range: SourceRange) -> Result<&str, ModelError> {
    range.validate_source(source)?;
    let start = usize::try_from(range.start_byte()).unwrap_or(usize::MAX);
    let end = usize::try_from(range.end_byte()).unwrap_or(usize::MAX);
    Ok(&source[start..end])
}

fn normalize_signature(signature: &str) -> String {
    signature.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU32, NonZeroU64};
    use std::time::Duration;

    use super::*;

    fn analyze(path: &str, language: SyntaxLanguage, source: &str) -> DocumentStructure {
        TypeScriptAdapter
            .analyze(
                AnalysisInput::new(path, language, source.to_string()).unwrap(),
                budget(1_000_000, 1_000, 1_000),
                &AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap()),
            )
            .unwrap()
    }

    fn budget(source: u64, symbols: u32, calls: u32) -> AnalysisBudget {
        budget_with_diagnostics(source, symbols, calls, 1_000)
    }

    fn budget_with_diagnostics(
        source: u64,
        symbols: u32,
        calls: u32,
        diagnostics: u32,
    ) -> AnalysisBudget {
        AnalysisBudget::new(
            NonZeroU64::new(source).unwrap(),
            NonZeroU32::new(symbols).unwrap(),
            NonZeroU32::new(calls).unwrap(),
            NonZeroU32::new(diagnostics).unwrap(),
        )
    }

    fn budget_with_capture(
        source: u64,
        symbols: u32,
        calls: u32,
        diagnostics: u32,
        capture: u64,
    ) -> AnalysisBudget {
        budget_with_diagnostics(source, symbols, calls, diagnostics)
            .with_max_capture_bytes(NonZeroU64::new(capture).unwrap())
    }

    fn analyze_with_budget(
        path: &str,
        language: SyntaxLanguage,
        source: &str,
        budget: AnalysisBudget,
    ) -> DocumentStructure {
        TypeScriptAdapter
            .analyze(
                AnalysisInput::new(path, language, source.to_string()).unwrap(),
                budget,
                &AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap()),
            )
            .unwrap()
    }

    fn symbol<'a>(document: &'a DocumentStructure, name: &str) -> &'a SymbolFact {
        document
            .symbols()
            .iter()
            .find(|symbol| symbol.key().name() == name)
            .unwrap_or_else(|| panic!("missing symbol {name}"))
    }

    #[test]
    fn extracts_typescript_declarations_and_owned_calls() {
        let document = analyze(
            "fixtures/review.mts",
            SyntaxLanguage::TypeScript,
            include_str!("fixtures/review.ts"),
        );
        assert_eq!(document.status(), DocumentStatus::Parsed);
        let run = symbol(&document, "run");
        assert_eq!(run.visibility(), SymbolVisibility::Exported);
        assert_eq!(run.parameter_count(), 2);
        assert!(run.normalized_signature().contains("<T>"));
        assert!(run.normalized_signature().contains(": Promise<T>"));
        assert!(run.syntactic_nesting_depth() >= 2);
        run.full_range()
            .validate_source(include_str!("fixtures/review.ts"))
            .unwrap();

        let worker = symbol(&document, "Worker");
        assert_eq!(worker.type_member_count(), 2);
        let method = symbol(&document, "execute");
        assert_eq!(method.key().qualified_path(), &["Worker"]);
        let interface_method = document
            .symbols()
            .iter()
            .find(|fact| {
                fact.key().name() == "execute" && fact.key().qualified_path() == ["Executor"]
            })
            .unwrap();
        assert_eq!(interface_method.key().kind(), SymbolKind::Method);

        let call = document
            .calls()
            .iter()
            .find(|call| call.callee_text() == "service.call")
            .unwrap();
        assert_eq!(call.argument_text(), "(value)");
        assert_eq!(call.enclosing_symbol().unwrap().name(), "run");
        assert!(call.control_context().contains(&ControlContext::Loop));
        let condition_call = document
            .calls()
            .iter()
            .find(|call| call.callee_text() == "service.ready")
            .unwrap();
        assert!(
            condition_call
                .control_context()
                .contains(&ControlContext::Condition)
        );
    }

    #[test]
    fn extracts_named_arrows_unicode_and_cts_input() {
        let source = "export const převeď = <T>(hodnota: T): T => hodnota;\nconst named = function inner(x: number) { return x; };";
        let document = analyze("src/value.cts", SyntaxLanguage::TypeScript, source);
        let arrow = symbol(&document, "převeď");
        assert_eq!(arrow.visibility(), SymbolVisibility::Exported);
        assert_eq!(arrow.parameter_count(), 1);
        assert_eq!(arrow.body_range().unwrap().line_count(), 1);
        assert!(
            source_text(source, arrow.full_range())
                .unwrap()
                .starts_with("export const převeď")
        );
        assert!(
            arrow
                .normalized_signature()
                .starts_with("export const převeď")
        );
        assert!(
            symbol(&document, "named")
                .normalized_signature()
                .contains("function inner")
        );
        assert!(
            source_text(source, symbol(&document, "named").full_range())
                .unwrap()
                .starts_with("const named")
        );
        for fact in document.symbols() {
            fact.full_range().validate_source(source).unwrap();
            fact.signature_range().validate_source(source).unwrap();
        }
    }

    #[test]
    fn multi_declarator_ranges_do_not_claim_sibling_bodies() {
        let source = "export const first = () => firstBody(), second = () => secondBody();\n";
        let document = analyze("src/multiple.ts", SyntaxLanguage::TypeScript, source);
        let second = symbol(&document, "second");
        let full = source_text(source, second.full_range()).unwrap();
        let signature = source_text(source, second.signature_range()).unwrap();

        assert_eq!(second.visibility(), SymbolVisibility::Exported);
        assert!(full.starts_with("second ="));
        assert!(!full.contains("firstBody"));
        assert!(!signature.contains("firstBody"));
        assert_eq!(
            source_text(source, second.body_range().unwrap()).unwrap(),
            "secondBody()"
        );
    }

    #[test]
    fn extracts_overload_signatures_without_inventing_identity() {
        let source = r#"
export function convert(value: string): string;
export function convert(value: string): string { return value; }
"#;
        let document = analyze("src/overloads.ts", SyntaxLanguage::TypeScript, source);
        let overloads: Vec<_> = document
            .symbols()
            .iter()
            .filter(|fact| fact.key().name() == "convert")
            .collect();
        assert_eq!(overloads.len(), 2);
        assert!(
            overloads
                .iter()
                .all(|fact| fact.key().qualified_path().is_empty())
        );
        assert!(
            overloads
                .iter()
                .all(|fact| fact.visibility() == SymbolVisibility::Exported)
        );
        assert!(overloads.iter().any(|fact| fact.body_range().is_none()));
        assert!(overloads.iter().any(|fact| fact.body_range().is_some()));
    }

    #[test]
    fn uses_tsx_grammar_for_components() {
        assert!(TypeScriptAdapter.supports(SyntaxLanguage::TypeScript));
        assert!(TypeScriptAdapter.supports(SyntaxLanguage::Tsx));
        assert!(!TypeScriptAdapter.supports(SyntaxLanguage::Rust));
        let document = analyze(
            "src/card.tsx",
            SyntaxLanguage::Tsx,
            include_str!("fixtures/component.tsx"),
        );
        assert_eq!(document.status(), DocumentStatus::Parsed);
        assert_eq!(document.provenance().language(), SyntaxLanguage::Tsx);
        assert_eq!(symbol(&document, "Card").parameter_count(), 1);
        assert!(
            document
                .calls()
                .iter()
                .any(|call| call.callee_text() == "format")
        );
    }

    #[test]
    fn extracts_generators_enums_namespaces_private_members_and_initializers() {
        let source = include_str!("fixtures/advanced.ts");
        let document = analyze("src/advanced.ts", SyntaxLanguage::TypeScript, source);
        assert_eq!(document.status(), DocumentStatus::Parsed);

        assert_eq!(
            symbol(&document, "listed").visibility(),
            SymbolVisibility::Exported
        );
        let generator = symbol(&document, "generate");
        assert_eq!(generator.key().kind(), SymbolKind::Function);
        assert!(
            generator
                .normalized_signature()
                .contains("function* generate<T>")
        );
        let assigned_generator = symbol(&document, "generated");
        assert!(
            assigned_generator
                .normalized_signature()
                .starts_with("export const generated = function* named")
        );

        let state = symbol(&document, "State");
        assert_eq!(state.key().kind(), SymbolKind::Enum);
        assert_eq!(state.type_member_count(), 2);
        for variant_name in ["Idle", "Busy"] {
            let variant = document
                .symbols()
                .iter()
                .find(|fact| {
                    fact.key().kind() == SymbolKind::Variant && fact.key().name() == variant_name
                })
                .unwrap();
            assert_eq!(variant.key().qualified_path(), &["State"]);
        }

        let namespace = symbol(&document, "Tools");
        assert_eq!(namespace.key().kind(), SymbolKind::Module);
        let parse = symbol(&document, "parse");
        assert_eq!(parse.key().qualified_path(), &["Tools"]);

        assert_eq!(
            symbol(&document, "#secret").visibility(),
            SymbolVisibility::Private
        );
        assert_eq!(
            symbol(&document, "#hide").visibility(),
            SymbolVisibility::Private
        );
        let visible = symbol(&document, "visible");
        let visible_signature = source_text(source, visible.signature_range()).unwrap();
        assert!(!visible_signature.contains("makeVisible"));
        assert_eq!(
            source_text(source, visible.body_range().unwrap()).unwrap(),
            "makeVisible()"
        );
        assert_eq!(symbol(&document, "method").parameter_count(), 2);
    }

    #[test]
    fn export_lists_are_scoped_to_their_lexical_module() {
        let source = r#"
function shared() {}
namespace Local {
    function shared() {}
    export { shared };
}
"#;
        let document = analyze("src/scoped.ts", SyntaxLanguage::TypeScript, source);
        let top_level = document
            .symbols()
            .iter()
            .find(|fact| fact.key().name() == "shared" && fact.key().qualified_path().is_empty())
            .unwrap();
        let namespace_local = document
            .symbols()
            .iter()
            .find(|fact| fact.key().name() == "shared" && fact.key().qualified_path() == ["Local"])
            .unwrap();

        assert_eq!(top_level.visibility(), SymbolVisibility::Private);
        assert_eq!(namespace_local.visibility(), SymbolVisibility::Exported);
    }

    #[test]
    fn broken_syntax_is_partial_with_diagnostics() {
        let document = analyze(
            "src/broken.ts",
            SyntaxLanguage::TypeScript,
            include_str!("fixtures/broken.ts"),
        );
        assert_eq!(document.status(), DocumentStatus::Partial);
        assert!(!document.diagnostics().is_empty());
    }

    #[test]
    fn diagnostic_budget_is_explicit_and_truncated() {
        let source = "const first = ;\nconst second = ;\nconst third = ;\n";
        let document = TypeScriptAdapter
            .analyze(
                AnalysisInput::new(
                    "src/many-errors.ts",
                    SyntaxLanguage::TypeScript,
                    source.to_string(),
                )
                .unwrap(),
                budget_with_diagnostics(1_000, 100, 100, 1),
                &AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap()),
            )
            .unwrap();
        assert_eq!(document.status(), DocumentStatus::Partial);
        assert_eq!(document.diagnostics().len(), 1);
        let truncation = document.truncation().unwrap();
        assert_eq!(truncation.reason(), SyntaxTruncationReason::DiagnosticCount);
        assert_eq!(truncation.limit(), Some(1));
        assert_eq!(truncation.observed(), Some(2));
    }

    #[test]
    fn retained_capture_exact_limit_succeeds_and_one_byte_under_is_partial() {
        let source = "export function run(value: string) { return work(value); }";
        let path = "src/capture.ts";
        let generous = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            source,
            budget_with_capture(10_000, 100, 100, 100, 10_000),
        );
        assert_eq!(generous.status(), DocumentStatus::Parsed);
        let exact = generous.estimated_owned_bytes();

        let at_limit = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            source,
            budget_with_capture(10_000, 100, 100, 100, exact),
        );
        assert_eq!(at_limit.status(), DocumentStatus::Parsed);
        assert_eq!(at_limit.estimated_owned_bytes(), exact);

        let below = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            source,
            budget_with_capture(10_000, 100, 100, 100, exact - 1),
        );
        assert_eq!(below.status(), DocumentStatus::Partial);
        assert_eq!(
            below.truncation().unwrap().reason(),
            SyntaxTruncationReason::CaptureBytes
        );
        assert_eq!(below.truncation().unwrap().limit(), Some(exact - 1));
        assert_eq!(below.truncation().unwrap().observed(), Some(exact));
        assert!(below.estimated_owned_bytes() < exact);
    }

    #[test]
    fn overlapping_nested_call_arguments_stop_on_capture_bytes_before_count() {
        let source = format!(
            "export function run() {{ return outer(inner(deep(\"{}\"))); }}",
            "payload".repeat(64)
        );
        let path = "src/nested-capture.ts";
        let generous = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            &source,
            budget_with_capture(100_000, 100, 100, 100, 100_000),
        );
        assert_eq!(generous.status(), DocumentStatus::Parsed);
        assert_eq!(generous.calls().len(), 3);
        assert!(
            generous.calls()[0]
                .argument_text()
                .contains(generous.calls()[1].argument_text())
        );
        assert!(
            generous.calls()[0].argument_text().len() + generous.calls()[1].argument_text().len()
                > generous.calls()[0].argument_text().len()
        );

        let calls_bytes: u64 = generous
            .calls()
            .iter()
            .map(CallFact::estimated_owned_bytes)
            .sum();
        let without_calls = generous.estimated_owned_bytes() - calls_bytes;
        let first_two = generous.calls()[0].estimated_owned_bytes()
            + generous.calls()[1].estimated_owned_bytes();
        let limit = without_calls + first_two - 1;
        let limited = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            &source,
            budget_with_capture(100_000, 100, 100, 100, limit),
        );
        assert_eq!(limited.status(), DocumentStatus::Partial);
        assert_eq!(limited.calls().len(), 1);
        assert_eq!(
            limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::CaptureBytes
        );
        assert_eq!(limited.truncation().unwrap().limit(), Some(limit));
        assert_eq!(limited.truncation().unwrap().observed(), Some(limit + 1));
        assert_eq!(
            limited.estimated_owned_bytes(),
            without_calls + generous.calls()[0].estimated_owned_bytes()
        );
    }

    #[test]
    fn retained_diagnostics_are_capture_accounted() {
        let source = "const first = ;\nconst second = ;\nconst third = ;\n";
        let path = "src/capture-errors.ts";
        let generous = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            source,
            budget_with_capture(10_000, 100, 100, 100, 10_000),
        );
        assert_eq!(generous.status(), DocumentStatus::Partial);
        assert!(generous.truncation().is_none());
        assert!(!generous.diagnostics().is_empty());
        let exact = generous.estimated_owned_bytes();

        let at_limit = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            source,
            budget_with_capture(10_000, 100, 100, 100, exact),
        );
        assert_eq!(at_limit.estimated_owned_bytes(), exact);
        assert!(at_limit.truncation().is_none());

        let limited = analyze_with_budget(
            path,
            SyntaxLanguage::TypeScript,
            source,
            budget_with_capture(10_000, 100, 100, 100, exact - 1),
        );
        assert_eq!(limited.status(), DocumentStatus::Partial);
        assert_eq!(
            limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::CaptureBytes
        );
        assert!(limited.diagnostics().len() < generous.diagnostics().len());
        assert!(limited.estimated_owned_bytes() < exact);
    }

    #[test]
    fn records_callback_closure_and_error_contexts_conservatively() {
        let source = r#"
export function run(items: string[]) {
  try { items.map((item) => transform(item)); }
  catch (error) { report(error); }
  switch (items.length) { case 1: notify(); }
}
"#;
        let document = analyze("src/contexts.ts", SyntaxLanguage::TypeScript, source);
        let transform = document
            .calls()
            .iter()
            .find(|call| call.callee_text() == "transform")
            .unwrap();
        assert!(
            transform
                .control_context()
                .contains(&ControlContext::Callback)
        );
        assert!(
            transform
                .control_context()
                .contains(&ControlContext::Closure)
        );
        let report = document
            .calls()
            .iter()
            .find(|call| call.callee_text() == "report")
            .unwrap();
        assert!(
            report
                .control_context()
                .contains(&ControlContext::ErrorBranch)
        );
        let notify = document
            .calls()
            .iter()
            .find(|call| call.callee_text() == "notify")
            .unwrap();
        assert!(notify.control_context().contains(&ControlContext::MatchArm));
    }

    #[test]
    fn deep_valid_and_broken_trees_use_bounded_heap_walks() {
        let depth = 2_000;
        let mut valid = String::from("function deep() {");
        valid.push_str(&"if (true) {".repeat(depth));
        valid.push_str("work();");
        valid.push_str(&"}".repeat(depth + 1));
        let document = analyze("src/deep.ts", SyntaxLanguage::TypeScript, &valid);
        assert_eq!(document.status(), DocumentStatus::Parsed);
        assert_eq!(
            symbol(&document, "deep").syntactic_nesting_depth(),
            u32::try_from(depth).unwrap()
        );

        let mut broken = String::from("function deep() {");
        broken.push_str(&"if (true) {".repeat(depth));
        broken.push_str("work();");
        broken.push_str(&"}".repeat(depth));
        let document = analyze("src/deep-broken.ts", SyntaxLanguage::TypeScript, &broken);
        assert_eq!(document.status(), DocumentStatus::Partial);
        assert!(!document.diagnostics().is_empty());
    }

    #[test]
    fn extractor_checks_cancellation_and_deadline_inside_large_walks() {
        let mut source = String::from("function work() {");
        source.push_str(&"call();".repeat(100_000));
        source.push('}');
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .unwrap();
        let tree = parser.parse(&source, None).unwrap();

        let cancelled = AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap());
        let cancellation_signal = cancelled.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1));
            cancellation_signal.cancel();
        });
        let cancelled_provenance =
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, TYPESCRIPT_PARSER).unwrap();
        let cancelled_budget = budget(10_000_000, 200_000, 200_000);
        let cancelled_capture = CaptureByteTracker::for_document(
            cancelled_budget.max_capture_bytes(),
            "src/large.ts",
            &cancelled_provenance,
        )
        .unwrap();
        let mut extractor = Extractor::new(
            &source,
            cancelled_provenance,
            cancelled_budget,
            &cancelled,
            ExportedNames::new(),
            cancelled_capture,
        );
        extractor.extract(tree.root_node()).unwrap();
        canceller.join().unwrap();
        assert_eq!(
            extractor.truncation.unwrap().reason(),
            SyntaxTruncationReason::Cancelled
        );

        let expired = AnalysisControl::new(NonZeroU64::new(500).unwrap());
        let expired_provenance =
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, TYPESCRIPT_PARSER).unwrap();
        let expired_budget = budget(10_000_000, 200_000, 200_000);
        let expired_capture = CaptureByteTracker::for_document(
            expired_budget.max_capture_bytes(),
            "src/large.ts",
            &expired_provenance,
        )
        .unwrap();
        let mut extractor = Extractor::new(
            &source,
            expired_provenance,
            expired_budget,
            &expired,
            ExportedNames::new(),
            expired_capture,
        );
        extractor.extract(tree.root_node()).unwrap();
        assert_eq!(
            extractor.truncation.unwrap().reason(),
            SyntaxTruncationReason::Time
        );
    }

    #[test]
    fn reports_each_budget_and_stop_path() {
        let source = "const one = () => first(); const two = () => second();";
        let input = || {
            AnalysisInput::new(
                "src/budget.ts",
                SyntaxLanguage::TypeScript,
                source.to_string(),
            )
            .unwrap()
        };
        let future = || AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap());

        let source_limited = TypeScriptAdapter
            .analyze(input(), budget(1, 10, 10), &future())
            .unwrap();
        assert_eq!(
            source_limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::SourceBytes
        );

        let symbol_limited = TypeScriptAdapter
            .analyze(input(), budget(1_000, 1, 10), &future())
            .unwrap();
        assert_eq!(symbol_limited.symbols().len(), 1);
        assert_eq!(
            symbol_limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::SymbolCount
        );

        let call_limited = TypeScriptAdapter
            .analyze(input(), budget(1_000, 10, 1), &future())
            .unwrap();
        assert_eq!(call_limited.calls().len(), 1);
        assert_eq!(
            call_limited.truncation().unwrap().reason(),
            SyntaxTruncationReason::CallCount
        );

        let cancelled = future();
        cancelled.cancel();
        let cancelled_document = TypeScriptAdapter
            .analyze(
                input(),
                budget_with_capture(1_000, 10, 10, 10, 1),
                &cancelled,
            )
            .unwrap();
        assert_eq!(
            cancelled_document.truncation().unwrap().reason(),
            SyntaxTruncationReason::Cancelled
        );

        let expired = AnalysisControl::new(NonZeroU64::new(1).unwrap());
        while !expired.deadline_exceeded(Instant::now()) {
            std::hint::spin_loop();
        }
        let timed_out = TypeScriptAdapter
            .analyze(input(), budget_with_capture(1_000, 10, 10, 10, 1), &expired)
            .unwrap();
        assert_eq!(
            timed_out.truncation().unwrap().reason(),
            SyntaxTruncationReason::Time
        );
        assert_eq!(timed_out.truncation().unwrap().limit(), Some(1));
        assert!(timed_out.truncation().unwrap().observed().unwrap() >= 1);
    }

    #[test]
    fn grammar_load_failure_preserves_control_stop_precedence() {
        let input = AnalysisInput::new("src/grammar.ts", SyntaxLanguage::TypeScript, String::new())
            .unwrap();
        let provenance =
            SyntaxProvenance::tree_sitter(SyntaxLanguage::TypeScript, TYPESCRIPT_PARSER).unwrap();
        let mut capture = CaptureByteTracker::for_document(
            NonZeroU64::new(1_000).unwrap(),
            input.path(),
            &provenance,
        )
        .unwrap();
        let cancelled = AnalysisControl::new(NonZeroU64::new(5_000_000).unwrap());
        cancelled.cancel();

        let document = failed_document(
            &input,
            provenance,
            "failed to load grammar",
            &mut capture,
            &cancelled,
        )
        .unwrap();

        assert_eq!(document.status(), DocumentStatus::Partial);
        assert!(document.diagnostics().is_empty());
        assert_eq!(
            document.truncation().unwrap().reason(),
            SyntaxTruncationReason::Cancelled
        );
    }

    #[test]
    fn rejects_rust_without_parsing_it_as_typescript() {
        let document = analyze("src/lib.rs", SyntaxLanguage::Rust, "fn main() {}\n");
        assert_eq!(document.status(), DocumentStatus::Unsupported);
        assert!(document.symbols().is_empty());
    }
}
