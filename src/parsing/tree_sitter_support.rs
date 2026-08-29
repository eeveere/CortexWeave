use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
};

use serde_json::json;
use tree_sitter::{Language, Node, Parser};

use crate::{
    CortexError, Result,
    domain::{
        AnalysisResult, AnalyzedChunk, AnalyzedRelationship, AnalyzedSymbol, GraphEdgeType,
        RelationshipTarget, SymbolKind,
    },
};

pub(crate) struct SymbolDescriptor {
    pub name: String,
    pub qualified_name: Option<String>,
    pub identity_name: Option<String>,
    pub kind: SymbolKind,
    pub key_kind: &'static str,
    pub is_container: bool,
}

pub(crate) trait StructureSpec {
    fn language(&self, path: &Path) -> Language;
    fn language_id(&self) -> &'static str;
    fn classify(&self, node: Node<'_>, source: &str) -> Option<SymbolDescriptor>;
    fn relationship_drafts(
        &self,
        _root: Node<'_>,
        _source: &str,
        _normalized_path: &str,
        _symbols: &mut Vec<AnalyzedSymbol>,
    ) -> Vec<RelationshipDraft> {
        Vec::new()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RelationshipDraft {
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

impl RelationshipDraft {
    pub(crate) fn new(
        from_key: impl Into<String>,
        target: RelationshipTarget,
        relationship: GraphEdgeType,
        confidence: f32,
        start_byte: usize,
        end_byte: usize,
        source: &str,
    ) -> Self {
        Self {
            from_key: from_key.into(),
            target,
            relationship,
            confidence,
            start_byte: Some(start_byte),
            end_byte: Some(end_byte),
            start_line: Some(line_number(source, start_byte)),
            end_line: Some(line_number(source, end_byte)),
            metadata: json!({}),
        }
    }
}

pub(crate) fn analyze<S: StructureSpec>(
    spec: &S,
    path: &Path,
    source: &str,
) -> Result<AnalysisResult> {
    if source.is_empty() {
        return Ok(AnalysisResult::default());
    }
    let mut parser = Parser::new();
    parser
        .set_language(&spec.language(path))
        .map_err(|error| CortexError::Analysis(error.to_string()))?;
    let tree = parser
        .parse(source, None)
        .ok_or_else(|| CortexError::Analysis("tree-sitter returned no syntax tree".into()))?;
    let normalized_path = path.to_string_lossy().replace('\\', "/");
    let mut chunks = Vec::new();
    walk(
        spec,
        tree.root_node(),
        source,
        &normalized_path,
        &mut Vec::new(),
        &mut chunks,
    );

    if chunks.is_empty() {
        let end = source.len();
        chunks.push(AnalyzedChunk {
            stable_key: format!("{normalized_path}::file"),
            language: spec.language_id().into(),
            symbol: None,
            qualified_symbol: None,
            symbol_kind: None,
            start_byte: 0,
            end_byte: end,
            start_line: 1,
            end_line: line_number(source, end),
            content: source.to_owned(),
            metadata: structural_metadata("source_file", false, "", None, None, None, 0),
        });
    }
    disambiguate_duplicate_keys(&mut chunks);
    assign_structural_ordinals(&mut chunks);
    let mut result = AnalysisResult::structured_chunks(chunks);
    let mut drafts = spec.relationship_drafts(
        tree.root_node(),
        source,
        &normalized_path,
        &mut result.symbols,
    );
    drafts.extend(containment_drafts(
        &result.symbols,
        &normalized_path,
        source,
    ));
    result.relationships = finalize_relationship_drafts(drafts);
    Ok(result)
}

pub(crate) fn file_node_key(normalized_path: &str) -> String {
    format!("file:{normalized_path}")
}

pub(crate) fn finalize_relationship_drafts(
    mut drafts: Vec<RelationshipDraft>,
) -> Vec<AnalyzedRelationship> {
    drafts.sort_by_key(relationship_sort_key);
    let mut ordinals = BTreeMap::<String, usize>::new();
    drafts
        .into_iter()
        .map(|draft| {
            let identity = format!(
                "{}|{}|{}|{}",
                draft.from_key,
                draft.relationship.storage_name(),
                draft.target.kind().storage_name(),
                draft.target.value(),
            );
            let ordinal = ordinals.entry(identity.clone()).or_default();
            let relationship_key = format!(
                "relationship:{}:{}",
                blake3::hash(identity.as_bytes()).to_hex(),
                *ordinal
            );
            *ordinal += 1;
            AnalyzedRelationship {
                relationship_key,
                from_key: draft.from_key,
                target: draft.target,
                relationship: draft.relationship,
                confidence: draft.confidence,
                start_byte: draft.start_byte,
                end_byte: draft.end_byte,
                start_line: draft.start_line,
                end_line: draft.end_line,
                metadata: draft.metadata,
            }
        })
        .collect()
}

fn relationship_sort_key(draft: &RelationshipDraft) -> (String, String, String, String, usize) {
    (
        draft.from_key.clone(),
        draft.relationship.storage_name(),
        draft.target.kind().storage_name(),
        draft.target.value().to_owned(),
        draft.start_byte.unwrap_or_default(),
    )
}

fn containment_drafts(
    symbols: &[AnalyzedSymbol],
    normalized_path: &str,
    source: &str,
) -> Vec<RelationshipDraft> {
    let file_key = file_node_key(normalized_path);
    let mut drafts = Vec::with_capacity(symbols.len() * 2);
    for symbol in symbols {
        let parent_key = symbol.parent_key.as_deref().unwrap_or(&file_key);
        drafts.push(RelationshipDraft::new(
            parent_key,
            RelationshipTarget::LocalStableKey(symbol.stable_key.clone()),
            GraphEdgeType::Contains,
            1.0,
            symbol.start_byte,
            symbol.end_byte,
            source,
        ));
        drafts.push(RelationshipDraft::new(
            &symbol.stable_key,
            RelationshipTarget::LocalStableKey(file_key.clone()),
            GraphEdgeType::DeclaredIn,
            1.0,
            symbol.start_byte,
            symbol.end_byte,
            source,
        ));
    }
    drafts
}

fn walk<S: StructureSpec>(
    spec: &S,
    node: Node<'_>,
    source: &str,
    path: &str,
    containers: &mut Vec<Container>,
    chunks: &mut Vec<AnalyzedChunk>,
) {
    let descriptor = spec.classify(node, source).map(|mut descriptor| {
        if descriptor.kind == SymbolKind::Function
            && containers.last().is_some_and(|container| {
                matches!(
                    container.kind,
                    SymbolKind::Class
                        | SymbolKind::Struct
                        | SymbolKind::Trait
                        | SymbolKind::Interface
                        | SymbolKind::Implementation
                )
            })
        {
            descriptor.kind = SymbolKind::Method;
            descriptor.key_kind = "method";
        }
        descriptor
    });

    let pushed = if let Some(descriptor) = descriptor {
        let mut segments: Vec<String> = containers
            .iter()
            .map(|container| format!("{}:{}", container.key_kind, container.name))
            .collect();
        let identity_name = descriptor
            .identity_name
            .as_deref()
            .unwrap_or(&descriptor.name);
        segments.push(format!("{}:{identity_name}", descriptor.key_kind));
        let stable_key = format!("{path}::{}", segments.join("::"));
        let qualified_symbol = containers
            .iter()
            .map(|container| container.name.as_str())
            .chain(std::iter::once(
                descriptor
                    .qualified_name
                    .as_deref()
                    .unwrap_or(&descriptor.name),
            ))
            .collect::<Vec<_>>()
            .join(".");
        let content_node = wrapped_node(node);
        let content_start = attribute_start(node).min(content_node.start_byte());
        let content_end = content_node.end_byte();
        let content = source
            .get(content_start..content_end)
            .unwrap_or_default()
            .to_owned();
        let identity_signature = ["type_parameters", "parameters"]
            .into_iter()
            .filter_map(|field| node.child_by_field_name(field))
            .filter_map(|parameters| source.get(parameters.byte_range()))
            .map(normalize_identity_text)
            .collect::<Vec<_>>()
            .join("|");
        let chunk_index = chunks.len();
        chunks.push(AnalyzedChunk {
            stable_key,
            language: spec.language_id().into(),
            symbol: Some(descriptor.name.clone()),
            qualified_symbol: Some(qualified_symbol),
            symbol_kind: Some(descriptor.kind.clone()),
            start_byte: content_start,
            end_byte: content_end,
            start_line: line_number(source, content_start),
            end_line: line_number(source, content_end),
            content,
            metadata: structural_metadata(
                node.kind(),
                node.has_error(),
                &identity_signature,
                containers
                    .last()
                    .map(|container| container.stable_key.as_str()),
                containers.last().map(|container| container.chunk_index),
                containers.last().map(|container| container.name.as_str()),
                containers.len(),
            ),
        });
        if descriptor.is_container {
            containers.push(Container {
                stable_key: chunks
                    .last()
                    .expect("chunk was just inserted")
                    .stable_key
                    .clone(),
                chunk_index,
                key_kind: descriptor.key_kind.into(),
                name: descriptor.identity_name.unwrap_or(descriptor.name),
                kind: descriptor.kind,
            });
            true
        } else {
            false
        }
    } else {
        false
    };

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(spec, child, source, path, containers, chunks);
    }
    if pushed {
        containers.pop();
    }
}

#[derive(Debug, Clone)]
struct Container {
    stable_key: String,
    chunk_index: usize,
    key_kind: String,
    name: String,
    kind: SymbolKind,
}

fn structural_metadata(
    node_kind: &str,
    has_error: bool,
    identity_signature: &str,
    parent_stable_key: Option<&str>,
    parent_chunk_index: Option<usize>,
    container_symbol: Option<&str>,
    structural_depth: usize,
) -> serde_json::Value {
    json!({
        "node_kind": node_kind,
        "has_error": has_error,
        "identity_signature": identity_signature,
        "parent_stable_key": parent_stable_key,
        "_parent_chunk_index": parent_chunk_index,
        "container_symbol": container_symbol,
        "structural_depth": structural_depth,
        "ordinal_in_container": null,
    })
}

fn wrapped_node(mut node: Node<'_>) -> Node<'_> {
    while let Some(parent) = node.parent() {
        if matches!(parent.kind(), "decorated_definition" | "export_statement") {
            node = parent;
        } else {
            break;
        }
    }
    node
}

fn attribute_start(node: Node<'_>) -> usize {
    let mut start = node.start_byte();
    let mut sibling = node.prev_sibling();
    while let Some(previous) = sibling {
        if previous.kind() != "attribute_item" {
            break;
        }
        start = previous.start_byte();
        sibling = previous.prev_sibling();
    }
    start
}

pub(crate) fn field_text(node: Node<'_>, field: &str, source: &str) -> Option<String> {
    node.child_by_field_name(field)
        .and_then(|child| source.get(child.byte_range()))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_identity_text(text: &str) -> String {
    text.chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

pub(crate) fn line_number(source: &str, byte: usize) -> usize {
    source[..byte.min(source.len())]
        .bytes()
        .filter(|value| *value == b'\n')
        .count()
        + 1
}

fn disambiguate_duplicate_keys(chunks: &mut [AnalyzedChunk]) {
    let mut counts = HashMap::new();
    for chunk in chunks.iter() {
        *counts.entry(chunk.stable_key.clone()).or_insert(0_usize) += 1;
    }
    let mut exact_counts: HashMap<String, usize> = HashMap::new();
    for chunk in chunks.iter_mut() {
        let original_key = chunk.stable_key.clone();
        if counts.get(&original_key).copied().unwrap_or_default() < 2 {
            continue;
        }
        let signature = chunk
            .metadata
            .get("identity_signature")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let digest = blake3::hash(signature.as_bytes()).to_hex()[..10].to_owned();
        let candidate = format!("{original_key}#{digest}");
        let ordinal = exact_counts.entry(candidate.clone()).or_default();
        chunk.stable_key = if *ordinal == 0 {
            candidate
        } else {
            format!("{candidate}~{ordinal}")
        };
        *ordinal += 1;
    }
    let stable_keys: Vec<_> = chunks
        .iter()
        .map(|chunk| chunk.stable_key.clone())
        .collect();
    for chunk in chunks {
        let Some(metadata) = chunk.metadata.as_object_mut() else {
            continue;
        };
        let parent_index = metadata
            .remove("_parent_chunk_index")
            .and_then(|value| value.as_u64())
            .and_then(|value| usize::try_from(value).ok());
        let parent_key = parent_index.and_then(|index| stable_keys.get(index));
        metadata.insert(
            "parent_stable_key".into(),
            parent_key.map_or(serde_json::Value::Null, |value| json!(value)),
        );
    }
}

fn assign_structural_ordinals(chunks: &mut [AnalyzedChunk]) {
    let mut ordinals: HashMap<Option<String>, usize> = HashMap::new();
    for chunk in chunks {
        let parent_key = chunk
            .metadata
            .get("parent_stable_key")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned);
        let ordinal = ordinals.entry(parent_key).or_default();
        if let Some(metadata) = chunk.metadata.as_object_mut() {
            metadata.insert("ordinal_in_container".into(), (*ordinal).into());
        }
        *ordinal += 1;
    }
}
