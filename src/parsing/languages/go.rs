use std::path::Path;

use tree_sitter::{Language, Node};

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

pub struct GoAnalyzer;

impl LanguageAnalyzer for GoAnalyzer {
    fn language_id(&self) -> &'static str {
        "go"
    }

    fn analyzer_id(&self) -> &'static str {
        "tree-sitter-go"
    }

    fn analyzer_version(&self) -> String {
        format!("{}-3", env!("CARGO_PKG_VERSION"))
    }
    fn structure_version(&self) -> String {
        "go-structure:v2".into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
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
            tests: true,
            ..Default::default()
        }
    }

    fn analyze(&self, path: &Path, source: &str) -> Result<AnalysisResult> {
        analyze(self, path, source)
    }
}

impl StructureSpec for GoAnalyzer {
    fn language(&self, _path: &Path) -> Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn language_id(&self) -> &'static str {
        "go"
    }

    fn classify(&self, node: Node<'_>, source: &str) -> Option<SymbolDescriptor> {
        match node.kind() {
            "function_declaration" => {
                named_descriptor(node, source, SymbolKind::Function, "function", true)
            }
            "method_declaration" => {
                let method = field_text(node, "name", source)?;
                let receiver = field_text(node, "receiver", source)
                    .and_then(|receiver| receiver_type(&receiver));
                Some(SymbolDescriptor {
                    name: method.clone(),
                    qualified_name: receiver
                        .as_ref()
                        .map(|receiver| format!("{receiver}.{method}")),
                    identity_name: receiver.map(|receiver| format!("{receiver}.{method}")),
                    kind: SymbolKind::Method,
                    key_kind: "method",
                    is_container: true,
                })
            }
            "type_spec" => {
                let type_kind = node.child_by_field_name("type").map(|child| child.kind());
                let kind = match type_kind {
                    Some("struct_type") => SymbolKind::Struct,
                    Some("interface_type") => SymbolKind::Interface,
                    _ => SymbolKind::Type,
                };
                named_descriptor(node, source, kind, "type", true)
            }
            _ => None,
        }
    }

    fn relationship_drafts(
        &self,
        root: Node<'_>,
        source: &str,
        normalized_path: &str,
        symbols: &mut Vec<crate::domain::AnalyzedSymbol>,
    ) -> Vec<RelationshipDraft> {
        extract_relationships(
            RelationshipLanguage::Go,
            root,
            source,
            normalized_path,
            symbols,
        )
    }
}

fn named_descriptor(
    node: Node<'_>,
    source: &str,
    kind: SymbolKind,
    key_kind: &'static str,
    is_container: bool,
) -> Option<SymbolDescriptor> {
    field_text(node, "name", source).map(|name| SymbolDescriptor {
        name,
        qualified_name: None,
        identity_name: None,
        kind,
        key_kind,
        is_container,
    })
}

fn receiver_type(receiver: &str) -> Option<String> {
    let trimmed = receiver
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')');
    let type_name = trimmed.split_whitespace().last()?.trim_start_matches('*');
    let type_name = type_name.split('[').next().unwrap_or(type_name);
    (!type_name.is_empty()).then(|| type_name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::receiver_type;

    #[test]
    fn extracts_receiver_type_for_value_pointer_and_generic_receivers() {
        assert_eq!(receiver_type("(engine Engine)"), Some("Engine".into()));
        assert_eq!(receiver_type("(engine *Engine)"), Some("Engine".into()));
        assert_eq!(receiver_type("(engine *Engine[T])"), Some("Engine".into()));
    }
}
