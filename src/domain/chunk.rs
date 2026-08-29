use serde::{Deserialize, Serialize};

use super::{GraphEdgeType, RelationshipTargetKind};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalyzerCapabilities {
    pub structural_chunks: bool,
    pub qualified_symbols: bool,
    pub contains: bool,
    pub declared_in: bool,
    pub imports: bool,
    pub exports: bool,
    pub depends_on: bool,
    pub references: bool,
    pub calls: bool,
    pub inheritance: bool,
    pub implementations: bool,
    pub overrides: bool,
    pub tests: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Module,
    Namespace,
    Type,
    Constant,
    Variable,
    Implementation,
    Other(String),
}

impl SymbolKind {
    pub fn storage_name(&self) -> String {
        match self {
            Self::Function => "function".into(),
            Self::Method => "method".into(),
            Self::Class => "class".into(),
            Self::Struct => "struct".into(),
            Self::Enum => "enum".into(),
            Self::Trait => "trait".into(),
            Self::Interface => "interface".into(),
            Self::Module => "module".into(),
            Self::Namespace => "namespace".into(),
            Self::Type => "type".into(),
            Self::Constant => "constant".into(),
            Self::Variable => "variable".into(),
            Self::Implementation => "implementation".into(),
            Self::Other(value) => format!("other:{value}"),
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "function" => Self::Function,
            "method" => Self::Method,
            "class" => Self::Class,
            "struct" => Self::Struct,
            "enum" => Self::Enum,
            "trait" => Self::Trait,
            "interface" => Self::Interface,
            "module" => Self::Module,
            "namespace" => Self::Namespace,
            "type" => Self::Type,
            "constant" => Self::Constant,
            "variable" => Self::Variable,
            "implementation" => Self::Implementation,
            other => Self::Other(other.strip_prefix("other:").unwrap_or(other).to_owned()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyzedChunk {
    pub stable_key: String,
    pub language: String,
    pub symbol: Option<String>,
    pub qualified_symbol: Option<String>,
    pub symbol_kind: Option<SymbolKind>,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyzedSymbol {
    pub stable_key: String,
    pub name: String,
    pub qualified_name: Option<String>,
    pub symbol_kind: SymbolKind,
    pub parent_key: Option<String>,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub metadata: serde_json::Value,
}

impl AnalyzedSymbol {
    pub fn from_chunk(chunk: &AnalyzedChunk) -> Option<Self> {
        Some(Self {
            stable_key: chunk.stable_key.clone(),
            name: chunk.symbol.clone()?,
            qualified_name: chunk.qualified_symbol.clone(),
            symbol_kind: chunk.symbol_kind.clone()?,
            parent_key: chunk
                .metadata
                .get("parent_stable_key")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            start_byte: chunk.start_byte,
            end_byte: chunk.end_byte,
            start_line: chunk.start_line,
            end_line: chunk.end_line,
            metadata: chunk.metadata.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum RelationshipTarget {
    LocalStableKey(String),
    QualifiedSymbol(String),
    ModulePath(String),
    External(String),
    Unresolved(String),
}

impl RelationshipTarget {
    pub fn kind(&self) -> RelationshipTargetKind {
        match self {
            Self::LocalStableKey(_) => RelationshipTargetKind::LocalStableKey,
            Self::QualifiedSymbol(_) => RelationshipTargetKind::QualifiedSymbol,
            Self::ModulePath(_) => RelationshipTargetKind::ModulePath,
            Self::External(_) => RelationshipTargetKind::External,
            Self::Unresolved(_) => RelationshipTargetKind::Unresolved,
        }
    }

    pub fn value(&self) -> &str {
        match self {
            Self::LocalStableKey(value)
            | Self::QualifiedSymbol(value)
            | Self::ModulePath(value)
            | Self::External(value)
            | Self::Unresolved(value) => value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalyzedRelationship {
    pub relationship_key: String,
    pub from_key: String,
    pub target: RelationshipTarget,
    pub relationship: GraphEdgeType,
    pub confidence: f32,
    pub start_byte: Option<usize>,
    pub end_byte: Option<usize>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub chunks: Vec<AnalyzedChunk>,
    pub symbols: Vec<AnalyzedSymbol>,
    pub relationships: Vec<AnalyzedRelationship>,
}

impl AnalysisResult {
    pub fn chunks_only(chunks: Vec<AnalyzedChunk>) -> Self {
        Self {
            chunks,
            symbols: Vec::new(),
            relationships: Vec::new(),
        }
    }

    pub fn structured_chunks(chunks: Vec<AnalyzedChunk>) -> Self {
        let symbols = chunks
            .iter()
            .filter_map(AnalyzedSymbol::from_chunk)
            .collect();
        Self {
            chunks,
            symbols,
            relationships: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relationship_targets_preserve_their_resolution_category() {
        let cases = [
            (
                RelationshipTarget::LocalStableKey("local".into()),
                RelationshipTargetKind::LocalStableKey,
            ),
            (
                RelationshipTarget::QualifiedSymbol("crate::run".into()),
                RelationshipTargetKind::QualifiedSymbol,
            ),
            (
                RelationshipTarget::ModulePath("crate::storage".into()),
                RelationshipTargetKind::ModulePath,
            ),
            (
                RelationshipTarget::External("crate:serde".into()),
                RelationshipTargetKind::External,
            ),
            (
                RelationshipTarget::Unresolved("run".into()),
                RelationshipTargetKind::Unresolved,
            ),
        ];
        for (target, expected_kind) in cases {
            assert_eq!(target.kind(), expected_kind);
            assert!(!target.value().is_empty());
        }
    }
}
