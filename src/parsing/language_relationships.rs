use std::{collections::HashMap, path::Path};

use serde_json::json;
use tree_sitter::Node;

use crate::domain::{AnalyzedSymbol, GraphEdgeType, RelationshipTarget, SymbolKind};

use super::tree_sitter_support::{RelationshipDraft, file_node_key};

#[derive(Debug, Clone, Copy)]
pub(crate) enum RelationshipLanguage {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    CSharp,
    Go,
}

pub(crate) fn extract_relationships(
    language: RelationshipLanguage,
    root: Node<'_>,
    source: &str,
    normalized_path: &str,
    symbols: &mut Vec<AnalyzedSymbol>,
) -> Vec<RelationshipDraft> {
    annotate_tests(language, root, source, normalized_path, symbols);
    let file_key = file_node_key(normalized_path);
    let mut drafts = export_drafts(language, source, &file_key, symbols);
    visit(root, &mut |node| match node.kind() {
        "use_declaration" if matches!(language, RelationshipLanguage::Rust) => {
            if let Some(target) = rust_use_target(node_text(node, source)) {
                add_module_dependency(
                    &mut drafts,
                    language,
                    normalized_path,
                    &file_key,
                    target.clone(),
                    node,
                    source,
                );
                if node_text(node, source).trim_start().starts_with("pub use ") {
                    add_export_module(&mut drafts, &file_key, target, node, source);
                }
            }
        }
        "import_statement" if matches!(language, RelationshipLanguage::Python) => {
            for target in python_import_targets(node_text(node, source)) {
                add_module_dependency(
                    &mut drafts,
                    language,
                    normalized_path,
                    &file_key,
                    target,
                    node,
                    source,
                );
            }
        }
        "import_from_statement" if matches!(language, RelationshipLanguage::Python) => {
            if let Some(target) = node
                .child_by_field_name("module_name")
                .map(|module| node_text(module, source).trim().to_owned())
                .filter(|target| !target.is_empty())
            {
                add_module_dependency(
                    &mut drafts,
                    language,
                    normalized_path,
                    &file_key,
                    target,
                    node,
                    source,
                );
            }
        }
        "import_statement"
            if matches!(
                language,
                RelationshipLanguage::TypeScript | RelationshipLanguage::JavaScript
            ) =>
        {
            if let Some(target) = node
                .child_by_field_name("source")
                .map(|source_node| unquote(node_text(source_node, source)))
                .filter(|target| !target.is_empty())
            {
                add_module_dependency(
                    &mut drafts,
                    language,
                    normalized_path,
                    &file_key,
                    target,
                    node,
                    source,
                );
            }
        }
        "using_directive" if matches!(language, RelationshipLanguage::CSharp) => {
            if let Some(target) = csharp_using_target(node_text(node, source)) {
                add_module_dependency(
                    &mut drafts,
                    language,
                    normalized_path,
                    &file_key,
                    target,
                    node,
                    source,
                );
            }
        }
        "import_spec" if matches!(language, RelationshipLanguage::Go) => {
            let target = go_import_target(node_text(node, source));
            if !target.is_empty() {
                add_module_dependency(
                    &mut drafts,
                    language,
                    normalized_path,
                    &file_key,
                    target,
                    node,
                    source,
                );
            }
        }
        "call_expression" if !matches!(language, RelationshipLanguage::CSharp) => {
            add_direct_call(&mut drafts, node, source, symbols);
        }
        "invocation_expression" if matches!(language, RelationshipLanguage::CSharp) => {
            add_direct_call(&mut drafts, node, source, symbols);
        }
        "impl_item" if matches!(language, RelationshipLanguage::Rust) => {
            add_rust_implementation(&mut drafts, node, source, symbols);
        }
        "class_definition" if matches!(language, RelationshipLanguage::Python) => {
            add_python_inheritance(&mut drafts, node, source, symbols);
        }
        "extends_clause"
            if matches!(
                language,
                RelationshipLanguage::TypeScript | RelationshipLanguage::JavaScript
            ) =>
        {
            add_heritage(&mut drafts, node, source, symbols, GraphEdgeType::Extends);
        }
        "implements_clause" if matches!(language, RelationshipLanguage::TypeScript) => {
            add_heritage(
                &mut drafts,
                node,
                source,
                symbols,
                GraphEdgeType::Implements,
            );
        }
        "base_list" if matches!(language, RelationshipLanguage::CSharp) => {
            add_csharp_base_types(&mut drafts, node, source, symbols);
        }
        _ => {}
    });
    drafts
}

fn export_drafts(
    language: RelationshipLanguage,
    source: &str,
    file_key: &str,
    symbols: &[AnalyzedSymbol],
) -> Vec<RelationshipDraft> {
    if !matches!(
        language,
        RelationshipLanguage::Rust
            | RelationshipLanguage::TypeScript
            | RelationshipLanguage::JavaScript
            | RelationshipLanguage::Go
    ) {
        return Vec::new();
    }
    symbols
        .iter()
        .filter(|symbol| is_exported(language, source, symbol))
        .map(|symbol| {
            RelationshipDraft::new(
                file_key,
                RelationshipTarget::LocalStableKey(symbol.stable_key.clone()),
                GraphEdgeType::Exports,
                1.0,
                symbol.start_byte,
                symbol.end_byte,
                source,
            )
        })
        .collect()
}

fn is_exported(language: RelationshipLanguage, source: &str, symbol: &AnalyzedSymbol) -> bool {
    let declaration = source
        .get(symbol.start_byte..symbol.end_byte)
        .unwrap_or_default()
        .trim_start();
    match language {
        RelationshipLanguage::Rust => {
            declaration.starts_with("pub ") || declaration.starts_with("pub(")
        }
        RelationshipLanguage::TypeScript | RelationshipLanguage::JavaScript => {
            declaration.starts_with("export ") || declaration.starts_with("export default ")
        }
        RelationshipLanguage::Go => symbol.name.chars().next().is_some_and(char::is_uppercase),
        _ => false,
    }
}

fn add_module_dependency(
    drafts: &mut Vec<RelationshipDraft>,
    language: RelationshipLanguage,
    normalized_path: &str,
    file_key: &str,
    target: String,
    node: Node<'_>,
    source: &str,
) {
    let metadata = module_resolution_metadata(language, normalized_path, &target);
    for relationship in [GraphEdgeType::Imports, GraphEdgeType::DependsOn] {
        let mut draft = RelationshipDraft::new(
            file_key,
            RelationshipTarget::ModulePath(target.clone()),
            relationship,
            1.0,
            node.start_byte(),
            node.end_byte(),
            source,
        );
        draft.metadata = metadata.clone();
        drafts.push(draft);
    }
}

fn module_resolution_metadata(
    language: RelationshipLanguage,
    normalized_path: &str,
    target: &str,
) -> serde_json::Value {
    let candidates = module_candidates(language, normalized_path, target);
    let external_module_roots = explicit_external_module_roots(language, target);
    json!({
        "resolution_aliases": candidates.into_iter().map(|candidate| json!({
            "alias": target,
            "target": candidate,
        })).collect::<Vec<_>>(),
        "external_module_roots": external_module_roots,
    })
}

fn module_candidates(
    language: RelationshipLanguage,
    normalized_path: &str,
    target: &str,
) -> Vec<String> {
    let target = target.trim();
    let candidates = match language {
        RelationshipLanguage::TypeScript => relative_module_candidates(
            normalized_path,
            target,
            &["ts", "tsx", "d.ts", "js", "jsx", "mjs", "cjs"],
        ),
        RelationshipLanguage::JavaScript => relative_module_candidates(
            normalized_path,
            target,
            &["js", "jsx", "mjs", "cjs", "ts", "tsx", "d.ts"],
        ),
        RelationshipLanguage::Python => python_module_candidates(normalized_path, target),
        RelationshipLanguage::Rust => rust_module_candidates(normalized_path, target),
        RelationshipLanguage::Go => relative_module_candidates(normalized_path, target, &["go"]),
        RelationshipLanguage::CSharp => Vec::new(),
    };
    let mut candidates: Vec<_> = candidates
        .into_iter()
        .map(|candidate| normalize_module_path(&candidate))
        .filter(|candidate| !candidate.is_empty())
        .collect();
    candidates.sort();
    candidates.dedup();
    candidates
}

fn explicit_external_module_roots(language: RelationshipLanguage, target: &str) -> Vec<String> {
    let target = target.trim();
    let root = match language {
        RelationshipLanguage::Rust => target
            .split("::")
            .next()
            .filter(|root| matches!(*root, "std" | "core" | "alloc")),
        RelationshipLanguage::TypeScript | RelationshipLanguage::JavaScript => {
            target.strip_prefix("node:")
        }
        _ => None,
    };
    root.map(|root| {
        if target.starts_with("node:") {
            format!("node:{root}")
        } else {
            root.into()
        }
    })
    .into_iter()
    .collect()
}

fn relative_module_candidates(source_path: &str, target: &str, extensions: &[&str]) -> Vec<String> {
    if !target.starts_with('.') {
        return Vec::new();
    }
    let directory = source_directory(source_path);
    let base = normalize_module_path(&format!("{directory}/{target}"));
    if let Some((without_extension, extension)) = base.rsplit_once('.') {
        let mut candidates = vec![base.clone()];
        if matches!(extension, "js" | "jsx" | "mjs" | "cjs") {
            candidates.extend(file_candidates(without_extension, extensions));
        }
        return candidates;
    }
    file_candidates(&base, extensions)
}

fn python_module_candidates(source_path: &str, target: &str) -> Vec<String> {
    if target.is_empty() {
        return Vec::new();
    }
    let levels = target
        .chars()
        .take_while(|character| *character == '.')
        .count();
    let remainder = &target[levels..];
    let base = if levels == 0 {
        remainder.replace('.', "/")
    } else {
        let mut directory = source_directory(source_path);
        for _ in 1..levels {
            directory = parent_directory(&directory);
        }
        if remainder.is_empty() {
            directory
        } else {
            format!("{directory}/{}", remainder.replace('.', "/"))
        }
    };
    file_candidates(&normalize_module_path(&base), &["py"])
}

fn rust_module_candidates(source_path: &str, target: &str) -> Vec<String> {
    let segments: Vec<_> = target
        .split("::")
        .filter(|segment| !segment.is_empty())
        .collect();
    let Some((head, tail)) = segments.split_first() else {
        return Vec::new();
    };
    let base = match *head {
        "crate" => crate_source_root(source_path),
        "self" => source_directory(source_path),
        "super" => parent_directory(&source_directory(source_path)),
        _ => return Vec::new(),
    };
    let mut candidates = Vec::new();
    for length in 1..=tail.len() {
        let module = tail[..length].join("/");
        candidates.extend(file_candidates(&format!("{base}/{module}"), &["rs"]));
    }
    candidates
}

fn file_candidates(base: &str, extensions: &[&str]) -> Vec<String> {
    let base = normalize_module_path(base);
    if base.is_empty() {
        return Vec::new();
    }
    if Path::new(&base).extension().is_some() {
        return vec![base];
    }
    extensions
        .iter()
        .flat_map(|extension| {
            [
                format!("{base}.{extension}"),
                format!("{base}/index.{extension}"),
            ]
        })
        .collect()
}

fn source_directory(path: &str) -> String {
    parent_directory(&normalize_module_path(path))
}

fn parent_directory(path: &str) -> String {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
        .into()
}

fn crate_source_root(path: &str) -> String {
    let normalized = normalize_module_path(path);
    let segments: Vec<_> = normalized.split('/').collect();
    segments
        .iter()
        .rposition(|segment| *segment == "src")
        .map(|index| segments[..=index].join("/"))
        .unwrap_or_else(|| source_directory(path))
}

fn normalize_module_path(path: &str) -> String {
    let mut segments = Vec::new();
    let normalized = path.replace('\\', "/");
    for segment in normalized.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|last| *last != "..") {
                    segments.pop();
                } else {
                    segments.push(segment);
                }
            }
            _ => segments.push(segment),
        }
    }
    segments.join("/")
}

fn add_export_module(
    drafts: &mut Vec<RelationshipDraft>,
    file_key: &str,
    target: String,
    node: Node<'_>,
    source: &str,
) {
    drafts.push(RelationshipDraft::new(
        file_key,
        RelationshipTarget::ModulePath(target),
        GraphEdgeType::Exports,
        1.0,
        node.start_byte(),
        node.end_byte(),
        source,
    ));
}

fn add_direct_call(
    drafts: &mut Vec<RelationshipDraft>,
    node: Node<'_>,
    source: &str,
    symbols: &[AnalyzedSymbol],
) {
    let Some(callee) = node
        .child_by_field_name("function")
        .or_else(|| node.child_by_field_name("expression"))
    else {
        return;
    };
    let target = node_text(callee, source).trim();
    if !is_identifier(target) || matches!(target, "test" | "it" | "describe") {
        return;
    }
    let Some(owner) = owner_symbol(symbols, node.start_byte()) else {
        return;
    };
    let target = RelationshipTarget::QualifiedSymbol(target.into());
    for relationship in [GraphEdgeType::Calls, GraphEdgeType::References] {
        drafts.push(RelationshipDraft::new(
            &owner.stable_key,
            target.clone(),
            relationship,
            0.9,
            node.start_byte(),
            node.end_byte(),
            source,
        ));
    }
    if is_test(owner) {
        let mut draft = RelationshipDraft::new(
            &owner.stable_key,
            target,
            GraphEdgeType::Tests,
            0.9,
            node.start_byte(),
            node.end_byte(),
            source,
        );
        draft.metadata = json!({
            "test_relationship": "direct_call_association",
            "test_certainty": "likely",
        });
        drafts.push(draft);
    }
}

fn add_rust_implementation(
    drafts: &mut Vec<RelationshipDraft>,
    node: Node<'_>,
    source: &str,
    symbols: &[AnalyzedSymbol],
) {
    let Some(trait_node) = node.child_by_field_name("trait") else {
        return;
    };
    let Some(type_node) = node.child_by_field_name("type") else {
        return;
    };
    let Some(type_name) = type_reference(node_text(type_node, source)) else {
        return;
    };
    let Some(trait_name) = type_reference(node_text(trait_node, source)) else {
        return;
    };
    let Some(owner) = symbols.iter().find(|symbol| symbol.name == type_name) else {
        return;
    };
    for relationship in [GraphEdgeType::Implements, GraphEdgeType::UsesType] {
        drafts.push(RelationshipDraft::new(
            &owner.stable_key,
            RelationshipTarget::QualifiedSymbol(trait_name.clone()),
            relationship,
            1.0,
            node.start_byte(),
            node.end_byte(),
            source,
        ));
    }
}

fn add_python_inheritance(
    drafts: &mut Vec<RelationshipDraft>,
    node: Node<'_>,
    source: &str,
    symbols: &[AnalyzedSymbol],
) {
    let Some(owner) = owner_symbol(symbols, node.start_byte()) else {
        return;
    };
    let Some(superclasses) = node.child_by_field_name("superclasses") else {
        return;
    };
    for target in immediate_type_references(superclasses, source) {
        for relationship in [GraphEdgeType::Extends, GraphEdgeType::UsesType] {
            drafts.push(RelationshipDraft::new(
                &owner.stable_key,
                RelationshipTarget::QualifiedSymbol(target.clone()),
                relationship,
                1.0,
                superclasses.start_byte(),
                superclasses.end_byte(),
                source,
            ));
        }
    }
}

fn add_heritage(
    drafts: &mut Vec<RelationshipDraft>,
    node: Node<'_>,
    source: &str,
    symbols: &[AnalyzedSymbol],
    relationship: GraphEdgeType,
) {
    let Some(owner) = owner_symbol(symbols, node.start_byte()) else {
        return;
    };
    for target in immediate_type_references(node, source) {
        drafts.push(RelationshipDraft::new(
            &owner.stable_key,
            RelationshipTarget::QualifiedSymbol(target.clone()),
            relationship.clone(),
            1.0,
            node.start_byte(),
            node.end_byte(),
            source,
        ));
        drafts.push(RelationshipDraft::new(
            &owner.stable_key,
            RelationshipTarget::QualifiedSymbol(target),
            GraphEdgeType::UsesType,
            1.0,
            node.start_byte(),
            node.end_byte(),
            source,
        ));
    }
}

fn add_csharp_base_types(
    drafts: &mut Vec<RelationshipDraft>,
    node: Node<'_>,
    source: &str,
    symbols: &[AnalyzedSymbol],
) {
    let Some(owner) = owner_symbol(symbols, node.start_byte()) else {
        return;
    };
    for target in immediate_type_references(node, source) {
        drafts.push(RelationshipDraft::new(
            &owner.stable_key,
            RelationshipTarget::QualifiedSymbol(target),
            GraphEdgeType::UsesType,
            0.9,
            node.start_byte(),
            node.end_byte(),
            source,
        ));
    }
}

fn annotate_tests(
    language: RelationshipLanguage,
    root: Node<'_>,
    source: &str,
    normalized_path: &str,
    symbols: &mut Vec<AnalyzedSymbol>,
) {
    let csharp_test_declarations = matches!(language, RelationshipLanguage::CSharp)
        .then(|| csharp_test_declarations(root, source))
        .unwrap_or_default();
    for symbol in symbols.iter_mut() {
        let declaration = source
            .get(symbol.start_byte..symbol.end_byte)
            .unwrap_or_default();
        let is_test = match language {
            RelationshipLanguage::Rust => declaration.contains("#[test]"),
            RelationshipLanguage::Python => symbol.name.starts_with("test_"),
            RelationshipLanguage::CSharp => csharp_test_declarations.contains(&symbol.start_byte),
            RelationshipLanguage::Go => {
                symbol.symbol_kind == SymbolKind::Function
                    && symbol.name.starts_with("Test")
                    && symbol.name.chars().nth(4).is_some_and(char::is_uppercase)
            }
            RelationshipLanguage::TypeScript | RelationshipLanguage::JavaScript => false,
        };
        if is_test {
            ensure_test_metadata(symbol);
        }
    }
    if matches!(
        language,
        RelationshipLanguage::TypeScript | RelationshipLanguage::JavaScript
    ) {
        add_javascript_test_symbols(root, source, normalized_path, symbols);
    }
}

fn csharp_test_declarations(root: Node<'_>, source: &str) -> std::collections::HashSet<usize> {
    let mut declarations = std::collections::HashSet::new();
    visit(root, &mut |node| {
        if !matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration" | "local_function_statement"
        ) {
            return;
        }
        let has_attribute = csharp_attributes(node, source)
            || node
                .prev_named_sibling()
                .filter(|sibling| sibling.kind() == "attribute_list")
                .is_some_and(|attributes| csharp_attributes(attributes, source));
        if has_attribute {
            declarations.insert(node.start_byte());
        }
    });
    declarations
}

fn csharp_attributes(node: Node<'_>, source: &str) -> bool {
    if node.kind() == "attribute_list" && csharp_attribute_text(node_text(node, source)) {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|child| child.kind() == "attribute_list")
        .any(|attributes| csharp_attribute_text(node_text(attributes, source)))
}

fn csharp_attribute_text(text: &str) -> bool {
    ["Fact", "Theory", "Test"].iter().any(|name| {
        text.split(|character: char| !character.is_alphanumeric() && character != '_')
            .any(|token| token == *name)
    })
}

fn ensure_test_metadata(symbol: &mut AnalyzedSymbol) {
    if !symbol.metadata.is_object() {
        symbol.metadata = json!({ "analyzer_metadata": symbol.metadata });
    }
    symbol.metadata["is_test"] = json!(true);
}

fn add_javascript_test_symbols(
    root: Node<'_>,
    source: &str,
    normalized_path: &str,
    symbols: &mut Vec<AnalyzedSymbol>,
) {
    let mut titles = HashMap::<String, usize>::new();
    visit(root, &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let framework = node_text(function, source).trim();
        if !matches!(framework, "test" | "it") {
            return;
        }
        let Some(arguments) = node.child_by_field_name("arguments") else {
            return;
        };
        let Some(title_node) = first_named_child(arguments) else {
            return;
        };
        let title = unquote(node_text(title_node, source));
        if title.is_empty() {
            return;
        }
        let ordinal = titles.entry(title.clone()).or_default();
        let stable_key = format!(
            "{normalized_path}::test:{}:{ordinal}",
            stable_fragment(&title)
        );
        *ordinal += 1;
        symbols.push(AnalyzedSymbol {
            stable_key,
            name: title.clone(),
            qualified_name: Some(title),
            symbol_kind: SymbolKind::Function,
            parent_key: None,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            start_line: super::tree_sitter_support::line_number(source, node.start_byte()),
            end_line: super::tree_sitter_support::line_number(source, node.end_byte()),
            metadata: json!({ "is_test": true, "test_framework": framework }),
        });
    });
}

fn rust_use_target(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix("pub ").unwrap_or(value.trim());
    let value = value
        .strip_prefix("use ")?
        .trim()
        .trim_end_matches(';')
        .trim();
    let value = value
        .split("::{")
        .next()
        .unwrap_or(value)
        .trim_end_matches("::*");
    (!value.is_empty()).then(|| value.into())
}

fn python_import_targets(value: &str) -> Vec<String> {
    value
        .trim()
        .strip_prefix("import ")
        .unwrap_or_default()
        .split(',')
        .filter_map(|part| part.split_whitespace().next())
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn csharp_using_target(value: &str) -> Option<String> {
    let value = value.trim().trim_end_matches(';').trim();
    let value = value.strip_prefix("global ").unwrap_or(value);
    let value = value.strip_prefix("using ")?.trim();
    let value = value.split('=').next_back().unwrap_or(value).trim();
    (!value.is_empty() && !value.starts_with("static ")).then(|| value.into())
}

fn go_import_target(value: &str) -> String {
    let value = value.trim();
    let Some(start) = value.find('"') else {
        return String::new();
    };
    let Some(end) = value[start + 1..].find('"') else {
        return String::new();
    };
    value[start + 1..start + 1 + end].into()
}

fn immediate_type_references(node: Node<'_>, source: &str) -> Vec<String> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter_map(|child| type_reference(node_text(child, source)))
        .collect()
}

fn type_reference(value: &str) -> Option<String> {
    let value = value.trim();
    let value = value.split('<').next().unwrap_or(value).trim();
    let value = value
        .trim_matches(|character: char| character == '(' || character == ')' || character == '?');
    (!value.is_empty()
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | ':')
        }))
    .then(|| value.into())
}

fn owner_symbol(symbols: &[AnalyzedSymbol], byte: usize) -> Option<&AnalyzedSymbol> {
    symbols
        .iter()
        .filter(|symbol| symbol.start_byte <= byte && symbol.end_byte >= byte)
        .min_by_key(|symbol| symbol.end_byte.saturating_sub(symbol.start_byte))
}

fn is_test(symbol: &AnalyzedSymbol) -> bool {
    symbol
        .metadata
        .get("is_test")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn is_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(character) if character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn stable_fragment(value: &str) -> String {
    let normalized: String = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect();
    let prefix = normalized.trim_matches('_');
    let prefix = if prefix.is_empty() { "test" } else { prefix };
    format!(
        "{}-{}",
        prefix,
        &blake3::hash(value.as_bytes()).to_hex()[..10]
    )
}

fn unquote(value: &str) -> String {
    value
        .trim()
        .trim_matches(|character| matches!(character, '\'' | '"' | '`'))
        .to_owned()
}

fn node_text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    source.get(node.byte_range()).unwrap_or_default()
}

fn first_named_child(node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor).next()
}

fn visit(node: Node<'_>, callback: &mut impl FnMut(Node<'_>)) {
    callback(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        visit(child, callback);
    }
}
