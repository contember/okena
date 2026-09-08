//! Rust declarations, via tree-sitter-rust.

use tree_sitter::Node;

use crate::model::{
    AnalysisLimits, DocumentSymbols, SymbolFact, SymbolKind, SymbolVisibility, Truncation,
};

pub(crate) fn analyze(source: &str, limits: AnalysisLimits) -> DocumentSymbols {
    let mut parser = tree_sitter::Parser::new();
    if parser
        .set_language(&tree_sitter_rust::LANGUAGE.into())
        .is_err()
    {
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

/// Declaration node kinds, and the kind each maps to.
fn symbol_kind(node: &Node<'_>, in_impl: bool) -> Option<SymbolKind> {
    Some(match node.kind() {
        "mod_item" => SymbolKind::Module,
        "function_item" | "function_signature_item" => {
            if in_impl {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            }
        }
        "struct_item" => SymbolKind::Struct,
        "enum_item" => SymbolKind::Enum,
        "union_item" => SymbolKind::Union,
        "trait_item" => SymbolKind::Trait,
        "impl_item" => SymbolKind::Impl,
        "type_item" => SymbolKind::TypeAlias,
        "const_item" | "static_item" => SymbolKind::Constant,
        _ => return None,
    })
}

/// Walk one container's children, attaching pending attributes to the
/// declaration that follows them.
///
/// Function bodies are not descended into: a declaration inside one is
/// invisible to every consumer this crate has.
fn walk(
    container: Node<'_>,
    source: &str,
    scope: &mut Vec<String>,
    document: &mut DocumentSymbols,
    limits: AnalysisLimits,
) {
    let in_impl = matches!(container.kind(), "impl_item" | "trait_item")
        || container
            .parent()
            .is_some_and(|parent| matches!(parent.kind(), "impl_item" | "trait_item"));
    let mut cursor = container.walk();
    let mut pending: Vec<Node<'_>> = Vec::new();
    for child in container.children(&mut cursor) {
        if child.kind() == "attribute_item" {
            pending.push(child);
            continue;
        }
        let Some(kind) = symbol_kind(&child, in_impl) else {
            if child.kind() == "declaration_list" || child.kind() == "field_declaration_list" {
                walk(child, source, scope, document, limits);
            }
            pending.clear();
            continue;
        };
        if document.symbols.len() >= limits.max_symbols {
            document.truncation = Some(Truncation::TooManySymbols {
                limit: limits.max_symbols,
            });
            return;
        }
        let name = declaration_name(&child, source);
        let start_line = pending.first().map_or_else(
            || child.start_position().row,
            |first| first.start_position().row,
        );
        document.symbols.push(SymbolFact {
            kind,
            name: name.clone(),
            scope: scope.clone(),
            visibility: visibility(&child, source),
            start_line: line(start_line),
            end_line: line(child.end_position().row),
            attributes: pending
                .iter()
                .map(|node| attribute_text(node, source))
                .collect(),
        });
        pending.clear();

        if matches!(
            kind,
            SymbolKind::Module | SymbolKind::Trait | SymbolKind::Impl
        ) {
            scope.push(name);
            walk(child, source, scope, document, limits);
            scope.pop();
            if document.truncation.is_some() {
                return;
            }
        }
    }
}

/// The declaration's own name: the `name` field, or the implemented type for an
/// `impl` block.
fn declaration_name(node: &Node<'_>, source: &str) -> String {
    node.child_by_field_name("name")
        .or_else(|| node.child_by_field_name("type"))
        .and_then(|name| text(&name, source))
        .unwrap_or_else(|| "_".to_string())
}

fn visibility(node: &Node<'_>, source: &str) -> SymbolVisibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "visibility_modifier" {
            continue;
        }
        return match text(&child, source).as_deref() {
            Some("pub") => SymbolVisibility::Public,
            Some(_) => SymbolVisibility::Crate,
            None => SymbolVisibility::Private,
        };
    }
    SymbolVisibility::Private
}

/// `#[cfg(test)]` without its `#[` and `]`.
fn attribute_text(node: &Node<'_>, source: &str) -> String {
    let raw = text(node, source).unwrap_or_default();
    raw.trim()
        .trim_start_matches("#[")
        .trim_end_matches(']')
        .trim()
        .to_string()
}

fn text(node: &Node<'_>, source: &str) -> Option<String> {
    source
        .get(node.start_byte()..node.end_byte())
        .map(str::to_string)
}

/// Zero-based tree-sitter row to a one-based line, saturating rather than
/// wrapping on a file longer than `u32::MAX` lines.
fn line(row: usize) -> u32 {
    u32::try_from(row).unwrap_or(u32::MAX).saturating_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbols(source: &str) -> Vec<SymbolFact> {
        analyze(source, AnalysisLimits::default()).symbols
    }

    fn find<'a>(symbols: &'a [SymbolFact], qualified: &str) -> &'a SymbolFact {
        symbols
            .iter()
            .find(|symbol| symbol.qualified() == qualified)
            .unwrap_or_else(|| panic!("no symbol {qualified} in {symbols:#?}"))
    }

    #[test]
    fn a_cfg_test_module_keeps_its_attribute_and_spans_its_lines() {
        let source = "\
pub fn run() {}

#[cfg(test)]
mod tests {
    #[test]
    fn works() {}
}
";
        let symbols = symbols(source);
        let module = find(&symbols, "tests");
        assert_eq!(module.kind, SymbolKind::Module);
        assert!(module.has_attribute("cfg"));
        assert_eq!(module.attributes, vec!["cfg(test)".to_string()]);
        // The attribute line counts as part of the module.
        assert_eq!((module.start_line, module.end_line), (3, 7));
    }

    #[test]
    fn nested_declarations_carry_their_scope() {
        let source = "\
mod engine {
    pub struct Runner;
    impl Runner {
        pub fn run(&self) {}
    }
}
";
        let symbols = symbols(source);
        assert_eq!(find(&symbols, "engine::Runner").kind, SymbolKind::Struct);
        let method = find(&symbols, "engine::Runner::run");
        assert_eq!(method.kind, SymbolKind::Method);
        assert_eq!(method.visibility, SymbolVisibility::Public);
    }

    #[test]
    fn visibility_distinguishes_public_from_restricted_and_private() {
        let source = "\
pub fn exported() {}
pub(crate) fn internal() {}
fn hidden() {}
";
        let symbols = symbols(source);
        assert_eq!(
            find(&symbols, "exported").visibility,
            SymbolVisibility::Public
        );
        assert_eq!(
            find(&symbols, "internal").visibility,
            SymbolVisibility::Crate
        );
        assert_eq!(
            find(&symbols, "hidden").visibility,
            SymbolVisibility::Private
        );
    }

    #[test]
    fn declarations_inside_a_function_body_are_not_reported() {
        let source = "\
fn outer() {
    struct Hidden;
    fn helper() {}
}
";
        let symbols = symbols(source);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "outer");
    }

    #[test]
    fn unparsable_source_yields_what_it_reached_without_failing() {
        let document = analyze("fn broken( {", AnalysisLimits::default());
        assert!(document.truncation.is_none());
    }

    #[test]
    fn the_symbol_limit_truncates_instead_of_growing() {
        let source = (0..50)
            .map(|index| format!("fn f{index}() {{}}\n"))
            .collect::<String>();
        let document = analyze(
            &source,
            AnalysisLimits {
                max_symbols: 10,
                ..AnalysisLimits::default()
            },
        );
        assert_eq!(document.symbols.len(), 10);
        assert_eq!(
            document.truncation,
            Some(Truncation::TooManySymbols { limit: 10 })
        );
    }

    #[test]
    fn an_oversized_file_is_rejected_before_parsing() {
        let document = crate::analyze(
            crate::SyntaxLanguage::Rust,
            "fn tiny() {}",
            AnalysisLimits {
                max_bytes: 4,
                ..AnalysisLimits::default()
            },
        );
        assert!(document.symbols.is_empty());
        assert_eq!(
            document.truncation,
            Some(Truncation::FileTooLarge {
                bytes: 12,
                limit: 4
            })
        );
    }
}
