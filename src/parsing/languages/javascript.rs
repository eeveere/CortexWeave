use crate::{
    Result,
    domain::{AnalysisResult, AnalyzerCapabilities, SymbolKind},
    parsing::{
        LanguageAnalyzer,
        language_relationships::{RelationshipLanguage, extract_relationships},
        tree_sitter_support::{
            RelationshipDraft, StructureSpec, SymbolDescriptor, analyze, field_text,
        },
    },
};
use std::path::Path;
use tree_sitter::{Language, Node};

pub struct JavaScriptAnalyzer;

impl LanguageAnalyzer for JavaScriptAnalyzer {
    fn language_id(&self) -> &'static str {
        "javascript"
    }
    fn analyzer_id(&self) -> &'static str {
        "tree-sitter-javascript"
    }
    fn analyzer_version(&self) -> String {
        format!("{}-2", env!("CARGO_PKG_VERSION"))
    }
    fn structure_version(&self) -> String {
        "javascript-structure:v2".into()
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["js", "jsx", "mjs", "cjs"]
    }
    fn capabilities(&self) -> AnalyzerCapabilities {
        AnalyzerCapabilities {
            structural_chunks: true,
            qualified_symbols: true,
            contains: true,
            declared_in: true,
            imports: true,
            exports: true,
            depends_on: true,
            references: true,
            calls: true,
            inheritance: true,
            tests: true,
            ..Default::default()
        }
    }
    fn analyze(&self, path: &Path, source: &str) -> Result<AnalysisResult> {
        analyze(self, path, source)
    }
}

impl StructureSpec for JavaScriptAnalyzer {
    fn language(&self, _path: &Path) -> Language {
        tree_sitter_javascript::LANGUAGE.into()
    }
    fn language_id(&self) -> &'static str {
        "javascript"
    }
    fn classify(&self, node: Node<'_>, source: &str) -> Option<SymbolDescriptor> {
        let (kind, key_kind, container) = match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                (SymbolKind::Function, "function", true)
            }
            "class_declaration" => (SymbolKind::Class, "class", true),
            "method_definition" => (SymbolKind::Method, "method", true),
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

    fn relationship_drafts(
        &self,
        root: Node<'_>,
        source: &str,
        normalized_path: &str,
        symbols: &mut Vec<crate::domain::AnalyzedSymbol>,
    ) -> Vec<RelationshipDraft> {
        extract_relationships(
            RelationshipLanguage::JavaScript,
            root,
            source,
            normalized_path,
            symbols,
        )
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
