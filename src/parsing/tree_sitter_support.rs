use std::{collections::HashMap, path::Path};

use serde_json::json;
use tree_sitter::{Language, Node, Parser};

use crate::{
    CortexError, Result,
    domain::{AnalyzedChunk, SymbolKind},
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
}

pub(crate) fn analyze<S: StructureSpec>(
    spec: &S,
    path: &Path,
    source: &str,
) -> Result<Vec<AnalyzedChunk>> {
    if source.is_empty() {
        return Ok(Vec::new());
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
            metadata: json!({"node_kind": "source_file", "structural_fallback": true}),
        });
    }
    disambiguate_duplicate_keys(&mut chunks);
    Ok(chunks)
}

fn walk<S: StructureSpec>(
    spec: &S,
    node: Node<'_>,
    source: &str,
    path: &str,
    containers: &mut Vec<(String, String, SymbolKind)>,
    chunks: &mut Vec<AnalyzedChunk>,
) {
    let descriptor = spec.classify(node, source).map(|mut descriptor| {
        if descriptor.kind == SymbolKind::Function
            && containers.last().is_some_and(|(_, _, kind)| {
                matches!(
                    kind,
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
            .map(|(key_kind, name, _)| format!("{key_kind}:{name}"))
            .collect();
        let identity_name = descriptor
            .identity_name
            .as_deref()
            .unwrap_or(&descriptor.name);
        segments.push(format!("{}:{identity_name}", descriptor.key_kind));
        let stable_key = format!("{path}::{}", segments.join("::"));
        let qualified_symbol = containers
            .iter()
            .map(|(_, name, _)| name.as_str())
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
            metadata: json!({
                "node_kind": node.kind(),
                "has_error": node.has_error(),
                "identity_signature": identity_signature,
            }),
        });
        if descriptor.is_container {
            containers.push((
                descriptor.key_kind.into(),
                descriptor.identity_name.unwrap_or(descriptor.name),
                descriptor.kind,
            ));
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

fn line_number(source: &str, byte: usize) -> usize {
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
        if counts.get(&chunk.stable_key).copied().unwrap_or_default() < 2 {
            continue;
        }
        let signature = chunk
            .metadata
            .get("identity_signature")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let digest = blake3::hash(signature.as_bytes()).to_hex()[..10].to_owned();
        let candidate = format!("{}#{digest}", chunk.stable_key);
        let ordinal = exact_counts.entry(candidate.clone()).or_default();
        chunk.stable_key = if *ordinal == 0 {
            candidate
        } else {
            format!("{candidate}~{ordinal}")
        };
        *ordinal += 1;
    }
}
