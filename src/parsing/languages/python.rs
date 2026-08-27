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

pub struct PythonAnalyzer;

impl LanguageAnalyzer for PythonAnalyzer {
    fn language_id(&self) -> &'static str {
        "python"
    }
    fn analyzer_id(&self) -> &'static str {
        "tree-sitter-python"
    }
    fn analyzer_version(&self) -> String {
        env!("CARGO_PKG_VERSION").into()
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["py", "pyi"]
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

impl StructureSpec for PythonAnalyzer {
    fn language(&self, _path: &Path) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }
    fn language_id(&self) -> &'static str {
        "python"
    }
    fn classify(&self, node: Node<'_>, source: &str) -> Option<SymbolDescriptor> {
        let (kind, key_kind, container) = match node.kind() {
            "function_definition" => (SymbolKind::Function, "function", true),
            "class_definition" => (SymbolKind::Class, "class", true),
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
