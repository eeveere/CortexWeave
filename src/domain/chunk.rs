use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalyzerCapabilities {
    pub structural_chunks: bool,
    pub qualified_symbols: bool,
    pub imports: bool,
    pub doc_comments: bool,
    pub relationships: bool,
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
