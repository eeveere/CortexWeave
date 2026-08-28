use crate::{
    Result,
    domain::{AnalyzedChunk, AnalyzerCapabilities, SymbolKind},
    parsing::{
        LanguageAnalyzer,
        tree_sitter_support::{StructureSpec, SymbolDescriptor, analyze, field_text},
    },
};
use std::path::Path;
use tree_sitter::{Language, Node};

pub struct TypeScriptAnalyzer;

impl LanguageAnalyzer for TypeScriptAnalyzer {
    fn language_id(&self) -> &'static str {
        "typescript"
    }
    fn analyzer_id(&self) -> &'static str {
        "tree-sitter-typescript"
    }
    fn analyzer_version(&self) -> String {
        format!("{}-2", env!("CARGO_PKG_VERSION"))
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "mts", "cts"]
    }
    fn capabilities(&self) -> AnalyzerCapabilities {
        AnalyzerCapabilities {
            structural_chunks: true,
            qualified_symbols: true,
            ..Default::default()
        }
    }
    fn analyze(&self, path: &Path, source: &str) -> Result<Vec<AnalyzedChunk>> {
        analyze(self, path, source)
    }
}

impl StructureSpec for TypeScriptAnalyzer {
    fn language(&self, path: &Path) -> Language {
        if path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("tsx"))
        {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        }
    }
    fn language_id(&self) -> &'static str {
        "typescript"
    }
    fn classify(&self, node: Node<'_>, source: &str) -> Option<SymbolDescriptor> {
        let (kind, key_kind, container) = match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                (SymbolKind::Function, "function", true)
            }
            "class_declaration" => (SymbolKind::Class, "class", true),
            "interface_declaration" => (SymbolKind::Interface, "interface", true),
            "enum_declaration" => (SymbolKind::Enum, "enum", true),
            "type_alias_declaration" => (SymbolKind::Type, "type", false),
            "internal_module" | "module" => (SymbolKind::Module, "module", true),
            "method_definition" | "method_signature" => (SymbolKind::Method, "method", true),
            "variable_declarator" if is_callable_value(node) => {
                (SymbolKind::Function, "function", true)
            }
            _ => return None,
        };
        field_text(node, "name", source).map(|name| SymbolDescriptor {
            name,
            qualified_name: None,
            identity_name: None,
            kind,
            key_kind,
            is_container: container,
        })
    }
}

fn is_callable_value(node: Node<'_>) -> bool {
    node.child_by_field_name("value").is_some_and(|value| {
        matches!(
            value.kind(),
            "arrow_function" | "function_expression" | "generator_function"
        )
    })
}
