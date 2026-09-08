//! TypeScript and TSX declarations, via tree-sitter-typescript.

use tree_sitter::Node;

use crate::language::SyntaxLanguage;
use crate::model::{
    AnalysisLimits, DocumentSymbols, SymbolFact, SymbolKind, SymbolVisibility, Truncation,
};

/// Callees that open a scope holding tests. `it` and `test` name one case;
/// they group nothing but still read as test scopes.
const TEST_CALLEES: &[&str] = &["describe", "suite", "context", "it", "test"];

pub(crate) fn analyze(
    language: SyntaxLanguage,
    source: &str,
    limits: AnalysisLimits,
) -> DocumentSymbols {
    let grammar = match language {
        SyntaxLanguage::Tsx => tree_sitter_typescript::LANGUAGE_TSX,
        _ => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&grammar.into()).is_err() {
        return DocumentSymbols::truncated(Truncation::ParseFailed);
    }
    let Some(tree) = parser.parse(source, None) else {
        return DocumentSymbols::truncated(Truncation::ParseFailed);
    };

    let mut document = DocumentSymbols::default();
    walk(
        tree.root_node(),
        source,
        &mut Vec::new(),
        &mut document,
        limits,
    );
    document
}

fn symbol_kind(node: &Node<'_>) -> Option<SymbolKind> {
    Some(match node.kind() {
        "function_declaration" | "generator_function_declaration" => SymbolKind::Function,
        "method_definition" => SymbolKind::Method,
        "class_declaration" | "abstract_class_declaration" => SymbolKind::Class,
        "interface_declaration" => SymbolKind::Interface,
        "type_alias_declaration" => SymbolKind::TypeAlias,
        "enum_declaration" => SymbolKind::Enum,
        "module" | "internal_module" => SymbolKind::Module,
        _ => return None,
    })
}

fn walk(
    container: Node<'_>,
    source: &str,
    scope: &mut Vec<String>,
    document: &mut DocumentSymbols,
    limits: AnalysisLimits,
) {
    let mut cursor = container.walk();
    for child in container.children(&mut cursor) {
        if document.truncation.is_some() {
            return;
        }
        match child.kind() {
            "export_statement" => {
                let Some(declaration) = child.child_by_field_name("declaration") else {
                    continue;
                };
                record(&declaration, Some(&child), source, scope, document, limits);
            }
            "expression_statement" => {
                if let Some(call) = child
                    .named_child(0)
                    .filter(|node| node.kind() == "call_expression")
                {
                    record_test_group(&call, source, scope, document, limits);
                }
            }
            "class_body" | "object_type" | "enum_body" | "statement_block" => {
                walk(child, source, scope, document, limits);
            }
            _ => record(&child, None, source, scope, document, limits),
        }
    }
}

/// Push a declaration, then descend into the body that can hold more.
fn record(
    node: &Node<'_>,
    export: Option<&Node<'_>>,
    source: &str,
    scope: &mut Vec<String>,
    document: &mut DocumentSymbols,
    limits: AnalysisLimits,
) {
    let Some(kind) = symbol_kind(node) else {
        // A lexical declaration is only interesting when it is exported.
        if export.is_some() && matches!(node.kind(), "lexical_declaration" | "variable_declaration")
        {
            record_exported_bindings(node, export, source, scope, document, limits);
        }
        return;
    };
    if !push(
        kind,
        name_of(node, source),
        node,
        export,
        source,
        scope,
        document,
        limits,
    ) {
        return;
    }
    let name = name_of(node, source);
    if let Some(body) = node.child_by_field_name("body") {
        scope.push(name);
        walk(body, source, scope, document, limits);
        scope.pop();
    }
}

/// `export const a = 1, b = 2` — one symbol per binding.
fn record_exported_bindings(
    node: &Node<'_>,
    export: Option<&Node<'_>>,
    source: &str,
    scope: &[String],
    document: &mut DocumentSymbols,
    limits: AnalysisLimits,
) {
    let mut cursor = node.walk();
    for declarator in node.named_children(&mut cursor) {
        if declarator.kind() != "variable_declarator" {
            continue;
        }
        let name = declarator
            .child_by_field_name("name")
            .and_then(|name| text(&name, source))
            .unwrap_or_else(|| "_".to_string());
        let is_function = declarator
            .child_by_field_name("value")
            .is_some_and(|value| {
                matches!(
                    value.kind(),
                    "arrow_function" | "function_expression" | "function"
                )
            });
        let kind = if is_function {
            SymbolKind::Function
        } else {
            SymbolKind::Constant
        };
        if !push(kind, name, node, export, source, scope, document, limits) {
            return;
        }
    }
}

/// `describe("…", () => { … })` and friends.
fn record_test_group(
    call: &Node<'_>,
    source: &str,
    scope: &mut Vec<String>,
    document: &mut DocumentSymbols,
    limits: AnalysisLimits,
) {
    let callee = call
        .child_by_field_name("function")
        .and_then(|node| text(&node, source))
        .unwrap_or_default();
    // `describe.each(...)` and `it.skip(...)` keep the callee in front of the dot.
    let head = callee.split('.').next().unwrap_or(&callee);
    if !TEST_CALLEES.contains(&head) {
        return;
    }
    let arguments = call.child_by_field_name("arguments");
    let name = arguments
        .and_then(|node| node.named_child(0))
        .and_then(|node| text(&node, source))
        .map(|raw| raw.trim_matches(['"', '\'', '`']).to_string())
        .unwrap_or_else(|| head.to_string());
    if !push(
        SymbolKind::TestGroup,
        name.clone(),
        call,
        None,
        source,
        scope,
        document,
        limits,
    ) {
        return;
    }
    let Some(body) = arguments
        .and_then(|node| {
            let mut cursor = node.walk();
            node.named_children(&mut cursor).find(|child| {
                matches!(
                    child.kind(),
                    "arrow_function" | "function_expression" | "function"
                )
            })
        })
        .and_then(|function| function.child_by_field_name("body"))
    else {
        return;
    };
    scope.push(name);
    walk(body, source, scope, document, limits);
    scope.pop();
}

/// Append one symbol. Returns false once the limit stops the walk.
#[allow(clippy::too_many_arguments)]
fn push(
    kind: SymbolKind,
    name: String,
    node: &Node<'_>,
    export: Option<&Node<'_>>,
    source: &str,
    scope: &[String],
    document: &mut DocumentSymbols,
    limits: AnalysisLimits,
) -> bool {
    if document.symbols.len() >= limits.max_symbols {
        document.truncation = Some(Truncation::TooManySymbols {
            limit: limits.max_symbols,
        });
        return false;
    }
    let span = export.unwrap_or(node);
    document.symbols.push(SymbolFact {
        kind,
        name,
        scope: scope.to_vec(),
        visibility: visibility(node, export, source),
        start_line: line(span.start_position().row),
        end_line: line(node.end_position().row),
        attributes: Vec::new(),
    });
    true
}

/// Exported reads as public; a class member follows its accessibility modifier.
fn visibility(node: &Node<'_>, export: Option<&Node<'_>>, source: &str) -> SymbolVisibility {
    if export.is_some() {
        return SymbolVisibility::Public;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "accessibility_modifier" {
            continue;
        }
        return match text(&child, source).as_deref() {
            Some("public") => SymbolVisibility::Public,
            _ => SymbolVisibility::Private,
        };
    }
    SymbolVisibility::Private
}

fn name_of(node: &Node<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .and_then(|name| text(&name, source))
        .unwrap_or_else(|| "_".to_string())
}

fn text(node: &Node<'_>, source: &str) -> Option<String> {
    source
        .get(node.start_byte()..node.end_byte())
        .map(str::to_string)
}

fn line(row: usize) -> u32 {
    u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols(source: &str) -> Vec<SymbolFact> {
        analyze(
            SyntaxLanguage::TypeScript,
            source,
            AnalysisLimits::default(),
        )
        .symbols
    }

    fn find<'a>(symbols: &'a [SymbolFact], qualified: &str) -> &'a SymbolFact {
        symbols
            .iter()
            .find(|symbol| symbol.qualified() == qualified)
            .unwrap_or_else(|| panic!("no symbol {qualified} in {symbols:#?}"))
    }

    #[test]
    fn exported_declarations_read_as_public() {
        let source = "\
export function run(input: string) {}
function helper() {}
export const LIMIT = 10;
export const build = () => 1;
";
        let symbols = symbols(source);
        assert_eq!(find(&symbols, "run").visibility, SymbolVisibility::Public);
        assert_eq!(
            find(&symbols, "helper").visibility,
            SymbolVisibility::Private
        );
        assert_eq!(find(&symbols, "LIMIT").kind, SymbolKind::Constant);
        assert_eq!(find(&symbols, "build").kind, SymbolKind::Function);
    }

    #[test]
    fn class_members_carry_their_class_as_scope() {
        let source = "\
export class Engine {
  run() {}
  private reset() {}
}
";
        let symbols = symbols(source);
        assert_eq!(find(&symbols, "Engine::run").kind, SymbolKind::Method);
        assert_eq!(
            find(&symbols, "Engine::reset").visibility,
            SymbolVisibility::Private
        );
    }

    #[test]
    fn describe_blocks_become_test_scopes_that_span_their_body() {
        let source = "\
export function run() {}

describe('engine', () => {
  it('runs', () => {
    expect(run()).toBe(1);
  });
});
";
        let symbols = symbols(source);
        let group = find(&symbols, "engine");
        assert_eq!(group.kind, SymbolKind::TestGroup);
        assert_eq!((group.start_line, group.end_line), (3, 7));
        assert_eq!(find(&symbols, "engine::runs").kind, SymbolKind::TestGroup);
    }

    #[test]
    fn a_qualified_test_callee_still_opens_a_scope() {
        let symbols = symbols("describe.skip('slow', () => {});");
        assert_eq!(find(&symbols, "slow").kind, SymbolKind::TestGroup);
    }

    #[test]
    fn an_ordinary_call_is_not_a_test_scope() {
        let symbols = symbols("configure('engine', () => {});");
        assert!(symbols.is_empty());
    }

    #[test]
    fn tsx_parses_with_the_tsx_grammar() {
        let document = analyze(
            SyntaxLanguage::Tsx,
            "export const App = () => <div>hi</div>;",
            AnalysisLimits::default(),
        );
        assert_eq!(document.symbols.len(), 1);
        assert_eq!(document.symbols[0].name, "App");
    }
}
