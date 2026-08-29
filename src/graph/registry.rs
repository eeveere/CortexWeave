use std::{collections::HashSet, sync::Arc};

use chrono::Utc;
use serde_json::{Map, Value, json};

use crate::{
    CortexError, Result,
    domain::{AnalyzedSymbol, Document, GraphNode, GraphNodeType, SourceSegment, SymbolKind},
    storage::SqliteStorage,
};

const FILE_ANALYZER_ID: &str = "graph-core";
const FILE_ANALYZER_VERSION: &str = "1";
const FILE_STRUCTURE_VERSION: &str = "graph-core:file:v1";

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRegistryUpdate {
    pub nodes: Vec<GraphNode>,
    pub removed: usize,
}

impl SymbolRegistryUpdate {
    pub fn file_node(&self) -> Option<&GraphNode> {
        self.nodes
            .iter()
            .find(|node| node.node_type == GraphNodeType::File)
    }

    pub fn symbol_nodes(&self) -> impl Iterator<Item = &GraphNode> {
        self.nodes
            .iter()
            .filter(|node| node.node_type != GraphNodeType::File)
    }
}

#[derive(Debug, Clone)]
pub struct SymbolRegistry {
    storage: Arc<SqliteStorage>,
}

impl SymbolRegistry {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub fn file_stable_key(relative_path: &str) -> String {
        format!("file:{}", normalized_path(relative_path))
    }

    pub fn symbol_stable_key(analyzer_id: &str, analyzer_stable_key: &str) -> String {
        format!("symbol:{analyzer_id}:{analyzer_stable_key}")
    }

    pub async fn reconcile_document(
        &self,
        document: &Document,
        structure_version: &str,
        symbols: &[AnalyzedSymbol],
    ) -> Result<SymbolRegistryUpdate> {
        if document.analyzer_id.trim().is_empty() || structure_version.trim().is_empty() {
            return Err(CortexError::Analysis(
                "symbol registration requires analyzer and structure identities".into(),
            ));
        }

        let previous = self
            .storage
            .graph_nodes_for_document(&document.workspace_id, &document.id)
            .await?;
        let nodes = self.nodes_for_document(document, structure_version, symbols)?;
        let next_keys: HashSet<_> = nodes.iter().map(|node| node.stable_key.as_str()).collect();
        let removed = previous
            .iter()
            .filter(|node| !next_keys.contains(node.stable_key.as_str()))
            .count();
        let nodes = self
            .storage
            .reconcile_graph_nodes_for_document(&document.workspace_id, &document.id, &nodes)
            .await?;
        Ok(SymbolRegistryUpdate { nodes, removed })
    }

    pub(crate) fn nodes_for_document(
        &self,
        document: &Document,
        structure_version: &str,
        symbols: &[AnalyzedSymbol],
    ) -> Result<Vec<GraphNode>> {
        let path = normalized_path(&document.relative_path);
        if path.is_empty() {
            return Err(CortexError::Analysis(
                "symbol registration requires a non-empty document path".into(),
            ));
        }
        let end_byte = u64::try_from(document.size_bytes).map_err(|_| {
            CortexError::Analysis("document size cannot be negative during registration".into())
        })?;
        let now = Utc::now();
        let mut file = GraphNode::new(
            &document.workspace_id,
            GraphNodeType::File,
            Self::file_stable_key(&path),
            path.rsplit('/').next().unwrap_or(path.as_str()),
        );
        file.language = Some(document.language.clone());
        file.qualified_name = Some(path.clone());
        file.document_id = Some(document.id.clone());
        file.source_segment = Some(SourceSegment::new(&path, 0, end_byte));
        file.analyzer_id = FILE_ANALYZER_ID.into();
        file.analyzer_version = FILE_ANALYZER_VERSION.into();
        file.structure_version = FILE_STRUCTURE_VERSION.into();
        file.content_revision = document.content_revision;
        file.metadata = json!({ "logical_kind": "file" });
        file.created_at = now;
        file.updated_at = now;

        let mut nodes = Vec::with_capacity(symbols.len() + 1);
        nodes.push(file);
        for symbol in symbols {
            let analyzer_key = symbol.stable_key.trim();
            if analyzer_key.is_empty() {
                return Err(CortexError::Analysis(format!(
                    "analyzer {} emitted an empty symbol stable key",
                    document.analyzer_id
                )));
            }
            if symbol.name.trim().is_empty() {
                return Err(CortexError::Analysis(format!(
                    "analyzer {} emitted an unnamed symbol {analyzer_key}",
                    document.analyzer_id
                )));
            }
            if symbol.end_byte < symbol.start_byte {
                return Err(CortexError::Analysis(format!(
                    "analyzer {} emitted an invalid source range for {analyzer_key}",
                    document.analyzer_id
                )));
            }

            let mut node = GraphNode::new(
                &document.workspace_id,
                node_type(&symbol.symbol_kind, &symbol.metadata),
                Self::symbol_stable_key(&document.analyzer_id, analyzer_key),
                &symbol.name,
            );
            node.language = Some(document.language.clone());
            node.qualified_name = symbol.qualified_name.clone();
            node.document_id = Some(document.id.clone());
            node.source_segment = Some(SourceSegment::new(
                &path,
                u64::try_from(symbol.start_byte).map_err(|_| {
                    CortexError::Analysis("symbol start offset exceeds graph range".into())
                })?,
                u64::try_from(symbol.end_byte).map_err(|_| {
                    CortexError::Analysis("symbol end offset exceeds graph range".into())
                })?,
            ));
            node.analyzer_id = document.analyzer_id.clone();
            node.analyzer_version = document.analyzer_version.clone();
            node.structure_version = structure_version.into();
            node.content_revision = document.content_revision;
            node.metadata = symbol_metadata(symbol, &document.analyzer_id);
            node.created_at = now;
            node.updated_at = now;
            nodes.push(node);
        }
        Ok(nodes)
    }
}

fn normalized_path(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn node_type(kind: &SymbolKind, metadata: &Value) -> GraphNodeType {
    if metadata
        .get("is_test")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return GraphNodeType::Test;
    }
    match kind {
        SymbolKind::Function => GraphNodeType::Function,
        SymbolKind::Method => GraphNodeType::Method,
        SymbolKind::Class => GraphNodeType::Class,
        SymbolKind::Struct => GraphNodeType::Struct,
        SymbolKind::Enum => GraphNodeType::Enum,
        SymbolKind::Trait => GraphNodeType::Trait,
        SymbolKind::Interface => GraphNodeType::Interface,
        SymbolKind::Module => GraphNodeType::Module,
        SymbolKind::Namespace => GraphNodeType::Namespace,
        SymbolKind::Type => GraphNodeType::Type,
        SymbolKind::Constant => GraphNodeType::Constant,
        SymbolKind::Variable => GraphNodeType::Other("variable".into()),
        SymbolKind::Implementation => GraphNodeType::Other("implementation".into()),
        SymbolKind::Other(value) => GraphNodeType::Other(value.clone()),
    }
}

fn symbol_metadata(symbol: &AnalyzedSymbol, analyzer_id: &str) -> Value {
    let mut metadata = match &symbol.metadata {
        Value::Object(values) => values.clone(),
        value => {
            let mut values = Map::new();
            values.insert("analyzer_metadata".into(), value.clone());
            values
        }
    };
    metadata.insert(
        "logical_stable_key".into(),
        Value::String(symbol.stable_key.clone()),
    );
    metadata.insert(
        "symbol_kind".into(),
        Value::String(symbol.symbol_kind.storage_name()),
    );
    metadata.insert(
        "start_line".into(),
        Value::from(u64::try_from(symbol.start_line).unwrap_or(u64::MAX)),
    );
    metadata.insert(
        "end_line".into(),
        Value::from(u64::try_from(symbol.end_line).unwrap_or(u64::MAX)),
    );
    if let Some(parent_key) = &symbol.parent_key {
        metadata.insert(
            "parent_stable_key".into(),
            Value::String(SymbolRegistry::symbol_stable_key(analyzer_id, parent_key)),
        );
    }
    Value::Object(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_keys_use_reserved_namespaces_and_normalized_paths() {
        assert_eq!(
            SymbolRegistry::file_stable_key(".\\src\\main.rs"),
            "file:src/main.rs"
        );
        assert_eq!(
            SymbolRegistry::symbol_stable_key("rust", "module::run"),
            "symbol:rust:module::run"
        );
    }
}
