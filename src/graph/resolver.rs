use std::{collections::HashSet, sync::Arc};

use crate::{
    CortexError, Result,
    domain::{
        GraphNode, GraphNodeType, RelationshipTarget, ResolutionAlias, ResolutionBasis,
        ResolutionOutcome, ResolutionRequest,
    },
    storage::SqliteStorage,
};

use super::SymbolRegistry;

#[derive(Debug, Clone)]
pub struct SymbolResolver {
    storage: Arc<SqliteStorage>,
}

impl SymbolResolver {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub async fn resolve(&self, request: &ResolutionRequest) -> Result<ResolutionOutcome> {
        let nodes = self.storage.graph_nodes(&request.workspace_id).await?;
        Self::resolve_against_nodes(request, &nodes)
    }

    pub(crate) fn resolve_against_nodes(
        request: &ResolutionRequest,
        nodes: &[GraphNode],
    ) -> Result<ResolutionOutcome> {
        let source_node = request
            .source_node_id
            .as_deref()
            .map(|node_id| {
                nodes.iter().find(|node| node.id == node_id).ok_or_else(|| {
                    CortexError::NotFound(format!(
                        "source graph node {node_id} in workspace {}",
                        request.workspace_id
                    ))
                })
            })
            .transpose()?;
        let language = request
            .language
            .as_deref()
            .or_else(|| source_node.and_then(|node| node.language.as_deref()));

        match &request.target {
            RelationshipTarget::LocalStableKey(target) => Ok(Self::resolve_local_key(
                request,
                source_node,
                language,
                target,
                nodes,
            )),
            RelationshipTarget::QualifiedSymbol(target) => Ok(Self::resolve_qualified(
                request,
                source_node,
                language,
                target,
                nodes,
            )),
            RelationshipTarget::ModulePath(target) => {
                Ok(Self::resolve_module(request, language, target, nodes))
            }
            RelationshipTarget::External(target) => Ok(nonempty_outcome(target, true)),
            RelationshipTarget::Unresolved(target) => Ok(nonempty_outcome(target, false)),
        }
    }

    fn resolve_local_key(
        request: &ResolutionRequest,
        source_node: Option<&GraphNode>,
        language: Option<&str>,
        target: &str,
        nodes: &[GraphNode],
    ) -> ResolutionOutcome {
        let target = target.trim();
        if target.is_empty() {
            return unresolved(target);
        }

        if let Some(node) = nodes
            .iter()
            .find(|node| node.stable_key == target)
            .filter(|node| language_matches(node, language))
            .cloned()
        {
            return resolved(node, ResolutionBasis::LocalStableKey);
        }
        if let Some(source_node) = source_node {
            let stable_key = SymbolRegistry::symbol_stable_key(&source_node.analyzer_id, target);
            if let Some(node) = nodes
                .iter()
                .find(|node| node.stable_key == stable_key)
                .filter(|node| language_matches(node, language))
                .cloned()
            {
                return resolved(node, ResolutionBasis::LocalStableKey);
            }
        }

        let mut candidates: Vec<_> = nodes
            .iter()
            .filter(|node| language_matches(node, language))
            .filter(|node| {
                node.metadata
                    .get("logical_stable_key")
                    .and_then(serde_json::Value::as_str)
                    == Some(target)
            })
            .cloned()
            .collect();
        if let Some(document_id) = source_document_id(request, source_node) {
            let local: Vec<_> = candidates
                .iter()
                .filter(|node| node.document_id.as_deref() == Some(document_id))
                .cloned()
                .collect();
            if !local.is_empty() {
                candidates = local;
            }
        }
        finish(candidates, ResolutionBasis::LocalStableKey, target)
    }

    fn resolve_qualified(
        request: &ResolutionRequest,
        source_node: Option<&GraphNode>,
        language: Option<&str>,
        target: &str,
        nodes: &[GraphNode],
    ) -> ResolutionOutcome {
        let target = target.trim();
        if target.is_empty() {
            return unresolved(target);
        }

        let (expanded, used_alias) = expand_aliases(target, &request.aliases);
        if used_alias {
            let candidates = qualified_candidates(nodes, language, &expanded);
            if !candidates.is_empty() {
                return finish(candidates, ResolutionBasis::Alias, target);
            }
        }

        if is_simple_name(target)
            && let Some(container_target) = source_node
                .and_then(|node| node.qualified_name.as_deref())
                .and_then(|qualified| sibling_qualified_name(qualified, target))
        {
            let candidates = nodes_by_qualified_name(nodes, &container_target, language);
            if !candidates.is_empty() {
                return finish(candidates, ResolutionBasis::EnclosingContainer, target);
            }
        }

        let candidates = nodes_by_qualified_name(nodes, target, language);
        if !candidates.is_empty() {
            return finish(candidates, ResolutionBasis::QualifiedName, target);
        }

        if is_simple_name(target) {
            let named = nodes_by_name(nodes, target, language);
            if let Some(document_id) = source_document_id(request, source_node) {
                let local: Vec<_> = named
                    .iter()
                    .filter(|node| node.document_id.as_deref() == Some(document_id))
                    .cloned()
                    .collect();
                if !local.is_empty() {
                    return finish(local, ResolutionBasis::SourceDocument, target);
                }
            }
            if !named.is_empty() {
                return finish(named, ResolutionBasis::WorkspaceName, target);
            }
        }
        unresolved(target)
    }

    fn resolve_module(
        request: &ResolutionRequest,
        language: Option<&str>,
        target: &str,
        nodes: &[GraphNode],
    ) -> ResolutionOutcome {
        let target = target.trim();
        if target.is_empty() {
            return unresolved(target);
        }
        let (expanded, used_alias) = expand_aliases(target, &request.aliases);
        let mut candidates = qualified_candidates(nodes, language, &expanded);
        candidates.retain(module_like);
        for candidate_target in &expanded {
            let file_key = SymbolRegistry::file_stable_key(candidate_target);
            if let Some(node) = nodes
                .iter()
                .find(|node| node.stable_key == file_key)
                .filter(|node| language_matches(node, language))
                .cloned()
            {
                candidates.push(node);
            }
        }
        if candidates.is_empty() && is_simple_name(target) {
            candidates = nodes_by_name(nodes, target, language)
                .into_iter()
                .filter(module_like)
                .collect();
        }
        let mut external_targets: Vec<_> = expanded
            .iter()
            .filter(|candidate| {
                request
                    .external_module_roots
                    .iter()
                    .any(|root| module_belongs_to(candidate, root))
            })
            .cloned()
            .collect();
        external_targets.sort();
        external_targets.dedup();

        if used_alias && !candidates.is_empty() && !external_targets.is_empty()
            || external_targets.len() > 1
        {
            return ResolutionOutcome::Ambiguous {
                candidates: deduplicate(candidates),
                external_targets,
                basis: ResolutionBasis::Alias,
            };
        }
        if !candidates.is_empty() {
            return finish(
                candidates,
                if used_alias {
                    ResolutionBasis::Alias
                } else {
                    ResolutionBasis::Module
                },
                target,
            );
        }
        if let Some(external_target) = external_targets.pop() {
            return ResolutionOutcome::External {
                target: external_target,
            };
        }
        unresolved(target)
    }
}

fn nodes_by_qualified_name(
    nodes: &[GraphNode],
    qualified_name: &str,
    language: Option<&str>,
) -> Vec<GraphNode> {
    nodes
        .iter()
        .filter(|node| node.qualified_name.as_deref() == Some(qualified_name))
        .filter(|node| language_matches(node, language))
        .cloned()
        .collect()
}

fn nodes_by_name(nodes: &[GraphNode], name: &str, language: Option<&str>) -> Vec<GraphNode> {
    nodes
        .iter()
        .filter(|node| node.name == name)
        .filter(|node| language_matches(node, language))
        .cloned()
        .collect()
}

fn qualified_candidates(
    nodes: &[GraphNode],
    language: Option<&str>,
    qualified_names: &[String],
) -> Vec<GraphNode> {
    deduplicate(
        nodes
            .iter()
            .filter(|node| {
                node.qualified_name
                    .as_ref()
                    .is_some_and(|name| qualified_names.contains(name))
            })
            .filter(|node| language_matches(node, language))
            .cloned()
            .collect(),
    )
}

fn source_document_id<'a>(
    request: &'a ResolutionRequest,
    source_node: Option<&'a GraphNode>,
) -> Option<&'a str> {
    request
        .source_document_id
        .as_deref()
        .or_else(|| source_node.and_then(|node| node.document_id.as_deref()))
}

fn language_matches(node: &GraphNode, language: Option<&str>) -> bool {
    language.is_none_or(|language| node.language.as_deref() == Some(language))
}

fn resolved(node: GraphNode, basis: ResolutionBasis) -> ResolutionOutcome {
    ResolutionOutcome::Resolved {
        node: Box::new(node),
        basis,
    }
}

fn unresolved(target: &str) -> ResolutionOutcome {
    ResolutionOutcome::Unresolved {
        target: target.into(),
    }
}

fn nonempty_outcome(target: &str, external: bool) -> ResolutionOutcome {
    let target = target.trim();
    if target.is_empty() || !external {
        unresolved(target)
    } else {
        ResolutionOutcome::External {
            target: target.into(),
        }
    }
}

fn finish(candidates: Vec<GraphNode>, basis: ResolutionBasis, target: &str) -> ResolutionOutcome {
    let mut candidates = deduplicate(candidates);
    match candidates.len() {
        0 => unresolved(target),
        1 => resolved(candidates.pop().expect("one candidate"), basis),
        _ => ResolutionOutcome::Ambiguous {
            candidates,
            external_targets: Vec::new(),
            basis,
        },
    }
}

fn deduplicate(mut candidates: Vec<GraphNode>) -> Vec<GraphNode> {
    candidates.sort_by(|left, right| {
        left.stable_key
            .cmp(&right.stable_key)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut ids = HashSet::with_capacity(candidates.len());
    candidates.retain(|node| ids.insert(node.id.clone()));
    candidates
}

fn expand_aliases(target: &str, aliases: &[ResolutionAlias]) -> (Vec<String>, bool) {
    let mut expanded = Vec::new();
    for alias in aliases {
        let alias_name = alias.alias.trim();
        let alias_target = alias.target.trim();
        if alias_name.is_empty() || alias_target.is_empty() {
            continue;
        }
        if target == alias_name {
            expanded.push(alias_target.into());
            continue;
        }
        if let Some(suffix) = target.strip_prefix(alias_name)
            && starts_with_separator(suffix)
        {
            expanded.push(format!("{alias_target}{suffix}"));
        }
    }
    let used_alias = !expanded.is_empty();
    if !used_alias {
        expanded.push(target.into());
    }
    expanded.sort();
    expanded.dedup();
    (expanded, used_alias)
}

fn starts_with_separator(value: &str) -> bool {
    value.starts_with("::") || value.starts_with('.') || value.starts_with('/')
}

fn is_simple_name(target: &str) -> bool {
    !target.contains("::") && !target.contains('.') && !target.contains(['/', '\\'])
}

fn sibling_qualified_name(source: &str, target: &str) -> Option<String> {
    let separators = [(source.rfind("::"), "::"), (source.rfind('.'), ".")];
    separators
        .into_iter()
        .filter_map(|(position, separator)| position.map(|position| (position, separator)))
        .max_by_key(|(position, _)| *position)
        .map(|(position, separator)| format!("{}{separator}{target}", &source[..position]))
}

fn module_like(node: &GraphNode) -> bool {
    matches!(
        node.node_type,
        GraphNodeType::Module
            | GraphNodeType::Namespace
            | GraphNodeType::Package
            | GraphNodeType::File
    )
}

fn module_belongs_to(target: &str, root: &str) -> bool {
    let root = root.trim();
    target == root || target.strip_prefix(root).is_some_and(starts_with_separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_expansion_preserves_all_ambiguous_imports() {
        let aliases = vec![
            ResolutionAlias::new("api", "alpha::api"),
            ResolutionAlias::new("api", "beta::api"),
        ];
        assert_eq!(
            expand_aliases("api::run", &aliases),
            (
                vec!["alpha::api::run".into(), "beta::api::run".into()],
                true
            )
        );
    }

    #[test]
    fn external_root_matching_respects_component_boundaries() {
        assert!(module_belongs_to("serde::de", "serde"));
        assert!(!module_belongs_to("serde_json", "serde"));
    }
}
