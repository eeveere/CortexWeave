use std::path::Path;

use tree_sitter::{Language, Node};

use crate::{
    Result,
    domain::{AnalyzedChunk, AnalyzerCapabilities, SymbolKind},
    parsing::{
        LanguageAnalyzer,
        tree_sitter_support::{StructureSpec, SymbolDescriptor, analyze, field_text},
    },
};

pub struct RustAnalyzer;

impl LanguageAnalyzer for RustAnalyzer {
    fn language_id(&self) -> &'static str {
        "rust"
    }
    fn analyzer_id(&self) -> &'static str {
        "tree-sitter-rust"
    }
    fn analyzer_version(&self) -> String {
        format!("{}-3", env!("CARGO_PKG_VERSION"))
    }
    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
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

impl StructureSpec for RustAnalyzer {
    fn language(&self, _path: &Path) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }
    fn language_id(&self) -> &'static str {
        "rust"
    }
    fn classify(&self, node: Node<'_>, source: &str) -> Option<SymbolDescriptor> {
        let (kind, key_kind, container, name_field) = match node.kind() {
            "function_item" => (SymbolKind::Function, "function", false, "name"),
            "struct_item" => (SymbolKind::Struct, "struct", true, "name"),
            "enum_item" => (SymbolKind::Enum, "enum", true, "name"),
            "trait_item" => (SymbolKind::Trait, "trait", true, "name"),
            "mod_item" => (SymbolKind::Module, "module", true, "name"),
            "type_item" => (SymbolKind::Type, "type", false, "name"),
            "const_item" | "static_item" => (SymbolKind::Constant, "constant", false, "name"),
            "impl_item" => (SymbolKind::Implementation, "impl", true, "type"),
            _ => return None,
        };
        let name = field_text(node, name_field, source)?;
        let identity_name = (node.kind() == "impl_item").then(|| {
            let trait_name = field_text(node, "trait", source).unwrap_or_default();
            if trait_name.is_empty() {
                name.clone()
            } else {
                format!("{trait_name} for {name}")
            }
        });
        Some(SymbolDescriptor {
            name,
            qualified_name: None,
            identity_name,
            kind,
            key_kind,
            is_container: container,
        })
    }
}
