use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Duration, Utc};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    CortexError, Result,
    domain::{
        AnalysisResult, AnalyzedRelationship, Document, GraphAnalysisExpectation,
        GraphAnalysisState, GraphEdge, GraphNode, GraphNodeType, GraphRelationshipFact,
        RelationshipTarget, RelationshipTargetKind, ResolutionAlias, ResolutionBasis,
        ResolutionOutcome, ResolutionRequest, SourceSegment, UnresolvedRelationship,
    },
    storage::{
        GraphReconciliationBatch, GraphReconciliationStatus, GraphRelationshipIdentity,
        SqliteStorage, UnresolvedGraphProjection,
    },
};

use super::{SymbolRegistry, SymbolResolver};

const MAX_RECONCILIATION_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphReconcileStatus {
    Current,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphReconcileOutcome {
    pub status: GraphReconcileStatus,
    pub affected_relationships: usize,
    pub resolved_relationships: usize,
    pub unresolved_relationships: usize,
}

#[derive(Debug, Clone)]
pub struct GraphIndexer {
    storage: Arc<SqliteStorage>,
    registry: SymbolRegistry,
}

impl GraphIndexer {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self {
            registry: SymbolRegistry::new(Arc::clone(&storage)),
            storage,
        }
    }

    pub async fn reconcile_document(
        &self,
        document: &Document,
        structure_version: &str,
        analysis: &AnalysisResult,
        expected_analysis: &[GraphAnalysisExpectation],
    ) -> Result<GraphReconcileOutcome> {
        let mut graph_snapshot = self
            .storage
            .workspace_graph_revision(&document.workspace_id)
            .await?
            .ok_or_else(|| {
                CortexError::NotFound(format!(
                    "workspace graph revision {}",
                    document.workspace_id
                ))
            })?;
        let mut target_revision = graph_snapshot.content_revision;

        for _ in 0..MAX_RECONCILIATION_ATTEMPTS {
            let prepared = self
                .prepare_document_batch(
                    document,
                    structure_version,
                    analysis,
                    expected_analysis,
                    target_revision,
                )
                .await;
            let mut batch = match prepared {
                Ok(batch) => batch,
                Err(error) => {
                    let _ = self
                        .storage
                        .mark_graph_error_if_snapshot(
                            &document.workspace_id,
                            target_revision,
                            graph_snapshot.updated_at,
                            &error.to_string(),
                            next_update_time(graph_snapshot.updated_at),
                        )
                        .await;
                    return Err(error);
                }
            };
            let update_started_at = next_update_time(graph_snapshot.updated_at);
            batch.update_started_at = Some(update_started_at);
            if !self
                .storage
                .mark_graph_updating_if_current(
                    &document.workspace_id,
                    target_revision,
                    graph_snapshot.updated_at,
                    update_started_at,
                )
                .await?
            {
                graph_snapshot = self
                    .storage
                    .workspace_graph_revision(&document.workspace_id)
                    .await?
                    .ok_or_else(|| {
                        CortexError::NotFound(format!(
                            "workspace graph revision {}",
                            document.workspace_id
                        ))
                    })?;
                target_revision = graph_snapshot.content_revision;
                continue;
            }
            match self
                .storage
                .apply_graph_reconciliation(&batch, next_update_time(update_started_at))
                .await
            {
                Ok(GraphReconciliationStatus::Current) => {
                    return Ok(batch_outcome(&batch, GraphReconcileStatus::Current));
                }
                Ok(GraphReconciliationStatus::Stale) => {
                    return Ok(batch_outcome(&batch, GraphReconcileStatus::Stale));
                }
                Ok(GraphReconciliationStatus::Superseded) => {
                    let _ = self
                        .storage
                        .clear_graph_update_if_owned(
                            &document.workspace_id,
                            target_revision,
                            update_started_at,
                            next_update_time(update_started_at),
                        )
                        .await;
                    graph_snapshot = self
                        .storage
                        .workspace_graph_revision(&document.workspace_id)
                        .await?
                        .ok_or_else(|| {
                            CortexError::NotFound(format!(
                                "workspace graph revision {}",
                                document.workspace_id
                            ))
                        })?;
                    target_revision = graph_snapshot.content_revision;
                }
                Err(error) => {
                    let _ = self
                        .storage
                        .mark_graph_error_for_update(
                            &document.workspace_id,
                            target_revision,
                            update_started_at,
                            &error.to_string(),
                            next_update_time(update_started_at),
                        )
                        .await;
                    return Err(error);
                }
            }
        }
        Err(CortexError::Analysis(format!(
            "graph reconciliation for {} was repeatedly superseded",
            document.relative_path
        )))
    }

    pub async fn delete_document(
        &self,
        workspace_id: &str,
        relative_path: &str,
        expected_analysis: &[GraphAnalysisExpectation],
    ) -> Result<bool> {
        for _ in 0..MAX_RECONCILIATION_ATTEMPTS {
            let Some(document) = self
                .storage
                .find_document(workspace_id, relative_path)
                .await?
            else {
                return Ok(false);
            };
            let revision = self
                .storage
                .workspace_graph_revision(workspace_id)
                .await?
                .ok_or_else(|| {
                    CortexError::NotFound(format!("workspace graph revision {workspace_id}"))
                })?;
            let target_revision = revision.content_revision.checked_add(1).ok_or_else(|| {
                CortexError::Analysis("workspace content revision overflow".into())
            })?;
            let batch = self
                .prepare_deletion_batch(
                    &document,
                    expected_analysis,
                    target_revision,
                    revision.updated_at,
                )
                .await?;
            match self
                .storage
                .apply_graph_reconciliation(&batch, next_update_time(revision.updated_at))
                .await?
            {
                GraphReconciliationStatus::Current | GraphReconciliationStatus::Stale => {
                    return Ok(true);
                }
                GraphReconciliationStatus::Superseded => continue,
            }
        }
        Err(CortexError::Analysis(format!(
            "graph deletion for {relative_path} was repeatedly superseded"
        )))
    }

    async fn prepare_document_batch(
        &self,
        document: &Document,
        structure_version: &str,
        analysis: &AnalysisResult,
        expected_analysis: &[GraphAnalysisExpectation],
        target_revision: i64,
    ) -> Result<GraphReconciliationBatch> {
        if document.analyzer_id.trim().is_empty() || structure_version.trim().is_empty() {
            return Err(CortexError::Analysis(
                "graph indexing requires analyzer and structure identities".into(),
            ));
        }
        let old_nodes = self.storage.graph_nodes(&document.workspace_id).await?;
        let old_facts = self
            .storage
            .graph_relationship_facts(&document.workspace_id)
            .await?;
        let old_edges = self.storage.graph_edges(&document.workspace_id).await?;
        let old_unresolved = self
            .storage
            .unresolved_relationships(&document.workspace_id)
            .await?;
        let old_document_nodes: Vec<_> = old_nodes
            .iter()
            .filter(|node| node.document_id.as_deref() == Some(&document.id))
            .cloned()
            .collect();
        let mut nodes =
            self.registry
                .nodes_for_document(document, structure_version, &analysis.symbols)?;
        preserve_node_identity(&mut nodes, &old_document_nodes);
        let mut desired_nodes: Vec<_> = old_nodes
            .iter()
            .filter(|node| node.document_id.as_deref() != Some(&document.id))
            .cloned()
            .collect();
        desired_nodes.extend(nodes.iter().cloned());

        let existing_source_facts: Vec<_> = old_facts
            .iter()
            .filter(|fact| fact.source_document_id == document.id)
            .cloned()
            .collect();
        let mut facts =
            relationships_to_facts(document, structure_version, &analysis.relationships, &nodes)?;
        preserve_fact_identity(&mut facts, &existing_source_facts);
        let mut desired_facts: Vec<_> = old_facts
            .iter()
            .filter(|fact| fact.source_document_id != document.id)
            .cloned()
            .collect();
        desired_facts.extend(facts.iter().cloned());

        let changed_terms = changed_resolution_terms(&old_document_nodes, &nodes);
        let removed_node_ids: HashSet<_> = old_document_nodes
            .iter()
            .filter(|old| !nodes.iter().any(|node| node.id == old.id))
            .map(|node| node.id.as_str())
            .collect();
        let affected = affected_relationships(
            &document.id,
            &old_facts,
            &desired_facts,
            &old_edges,
            &changed_terms,
            &removed_node_ids,
        );
        let (edges, unresolved) = project_relationships(
            &document.workspace_id,
            &desired_nodes,
            &desired_facts,
            &affected,
            &old_edges,
            &old_unresolved,
        )?;
        Ok(GraphReconciliationBatch {
            workspace_id: document.workspace_id.clone(),
            target_content_revision: target_revision,
            expected_graph_updated_at: None,
            update_started_at: None,
            delete_relative_path: None,
            source_document_id: Some(document.id.clone()),
            nodes,
            facts,
            analysis_state: Some(GraphAnalysisState {
                document_id: document.id.clone(),
                workspace_id: document.workspace_id.clone(),
                content_revision: document.content_revision,
                analyzer_id: document.analyzer_id.clone(),
                analyzer_version: document.analyzer_version.clone(),
                structure_version: structure_version.into(),
                last_error: None,
                analyzed_at: Utc::now(),
            }),
            expected_analysis: expected_analysis.to_vec(),
            affected_relationships: affected,
            edges,
            unresolved,
        })
    }

    async fn prepare_deletion_batch(
        &self,
        document: &Document,
        expected_analysis: &[GraphAnalysisExpectation],
        target_revision: i64,
        expected_graph_updated_at: DateTime<Utc>,
    ) -> Result<GraphReconciliationBatch> {
        let old_nodes = self.storage.graph_nodes(&document.workspace_id).await?;
        let old_facts = self
            .storage
            .graph_relationship_facts(&document.workspace_id)
            .await?;
        let old_edges = self.storage.graph_edges(&document.workspace_id).await?;
        let old_unresolved = self
            .storage
            .unresolved_relationships(&document.workspace_id)
            .await?;
        let deleted_nodes: Vec<_> = old_nodes
            .iter()
            .filter(|node| node.document_id.as_deref() == Some(&document.id))
            .cloned()
            .collect();
        let desired_nodes: Vec<_> = old_nodes
            .iter()
            .filter(|node| node.document_id.as_deref() != Some(&document.id))
            .cloned()
            .collect();
        let desired_facts: Vec<_> = old_facts
            .iter()
            .filter(|fact| fact.source_document_id != document.id)
            .cloned()
            .collect();
        let changed_terms = changed_resolution_terms(&deleted_nodes, &[]);
        let removed_node_ids: HashSet<_> =
            deleted_nodes.iter().map(|node| node.id.as_str()).collect();
        let affected = affected_relationships(
            &document.id,
            &old_facts,
            &desired_facts,
            &old_edges,
            &changed_terms,
            &removed_node_ids,
        );
        let (edges, unresolved) = project_relationships(
            &document.workspace_id,
            &desired_nodes,
            &desired_facts,
            &affected,
            &old_edges,
            &old_unresolved,
        )?;
        Ok(GraphReconciliationBatch {
            workspace_id: document.workspace_id.clone(),
            target_content_revision: target_revision,
            expected_graph_updated_at: Some(expected_graph_updated_at),
            update_started_at: None,
            delete_relative_path: Some(document.relative_path.clone()),
            source_document_id: None,
            nodes: Vec::new(),
            facts: Vec::new(),
            analysis_state: None,
            expected_analysis: expected_analysis.to_vec(),
            affected_relationships: affected,
            edges,
            unresolved,
        })
    }
}

fn next_update_time(after: DateTime<Utc>) -> DateTime<Utc> {
    let now = Utc::now();
    if now > after {
        now
    } else {
        after + Duration::nanoseconds(1)
    }
}

fn batch_outcome(
    batch: &GraphReconciliationBatch,
    status: GraphReconcileStatus,
) -> GraphReconcileOutcome {
    GraphReconcileOutcome {
        status,
        affected_relationships: batch.affected_relationships.len(),
        resolved_relationships: batch.edges.len(),
        unresolved_relationships: batch.unresolved.len(),
    }
}

fn preserve_node_identity(nodes: &mut [GraphNode], previous: &[GraphNode]) {
    let by_key: HashMap<_, _> = previous
        .iter()
        .map(|node| (node.stable_key.as_str(), node))
        .collect();
    for node in nodes {
        if let Some(old) = by_key.get(node.stable_key.as_str()) {
            node.id = old.id.clone();
            node.created_at = old.created_at;
        }
    }
}

fn preserve_fact_identity(facts: &mut [GraphRelationshipFact], previous: &[GraphRelationshipFact]) {
    let by_key: HashMap<_, _> = previous
        .iter()
        .map(|fact| (fact.relationship_key.as_str(), fact))
        .collect();
    for fact in facts {
        if let Some(old) = by_key.get(fact.relationship_key.as_str()) {
            fact.id = old.id.clone();
            fact.created_at = old.created_at;
        }
    }
}

fn relationships_to_facts(
    document: &Document,
    structure_version: &str,
    relationships: &[AnalyzedRelationship],
    nodes: &[GraphNode],
) -> Result<Vec<GraphRelationshipFact>> {
    let now = Utc::now();
    let mut keys = HashSet::new();
    let mut facts = Vec::with_capacity(relationships.len());
    for relationship in relationships {
        if relationship.relationship_key.trim().is_empty()
            || !keys.insert(relationship.relationship_key.as_str())
        {
            return Err(CortexError::Analysis(
                "analyzer emitted an empty or duplicate relationship key".into(),
            ));
        }
        if !relationship.confidence.is_finite() || !(0.0..=1.0).contains(&relationship.confidence) {
            return Err(CortexError::Analysis(format!(
                "relationship {} has invalid confidence",
                relationship.relationship_key
            )));
        }
        let from_node = source_node(nodes, &document.analyzer_id, &relationship.from_key)
            .ok_or_else(|| {
                CortexError::Analysis(format!(
                    "relationship {} refers to unknown source {}",
                    relationship.relationship_key, relationship.from_key
                ))
            })?;
        let mut fact = UnresolvedRelationship::new(
            &document.workspace_id,
            &document.id,
            &relationship.relationship_key,
            &from_node.stable_key,
            relationship.relationship.clone(),
            relationship.target.kind(),
            relationship.target.value(),
        );
        fact.from_node = Some(from_node.id.clone());
        fact.confidence = relationship.confidence;
        fact.analyzer_id = document.analyzer_id.clone();
        fact.analyzer_version = document.analyzer_version.clone();
        fact.structure_version = structure_version.into();
        fact.source_segment = relationship_source_segment(document, relationship)?;
        fact.content_revision = document.content_revision;
        fact.metadata = relationship_metadata(relationship);
        fact.created_at = now;
        fact.updated_at = now;
        facts.push(fact);
    }
    Ok(facts)
}

fn source_node<'a>(
    nodes: &'a [GraphNode],
    analyzer_id: &str,
    from_key: &str,
) -> Option<&'a GraphNode> {
    nodes
        .iter()
        .find(|node| node.stable_key == from_key)
        .or_else(|| {
            let stable_key = SymbolRegistry::symbol_stable_key(analyzer_id, from_key);
            nodes.iter().find(|node| node.stable_key == stable_key)
        })
        .or_else(|| {
            nodes.iter().find(|node| {
                node.metadata
                    .get("logical_stable_key")
                    .and_then(Value::as_str)
                    == Some(from_key)
            })
        })
}

fn relationship_source_segment(
    document: &Document,
    relationship: &AnalyzedRelationship,
) -> Result<Option<SourceSegment>> {
    match (relationship.start_byte, relationship.end_byte) {
        (Some(start), Some(end)) if end >= start => Ok(Some(SourceSegment::new(
            &document.relative_path,
            u64::try_from(start).map_err(|_| {
                CortexError::Analysis("relationship start offset exceeds graph range".into())
            })?,
            u64::try_from(end).map_err(|_| {
                CortexError::Analysis("relationship end offset exceeds graph range".into())
            })?,
        ))),
        (None, None) => Ok(None),
        _ => Err(CortexError::Analysis(
            "relationship source range must be complete and ordered".into(),
        )),
    }
}

fn relationship_metadata(relationship: &AnalyzedRelationship) -> Value {
    let mut metadata = object_metadata(&relationship.metadata);
    if let Some(line) = relationship.start_line {
        metadata.insert("start_line".into(), json!(line));
    }
    if let Some(line) = relationship.end_line {
        metadata.insert("end_line".into(), json!(line));
    }
    Value::Object(metadata)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ResolutionTerm {
    kind: RelationshipTargetKind,
    value: String,
}

fn changed_resolution_terms(old: &[GraphNode], new: &[GraphNode]) -> HashSet<ResolutionTerm> {
    let old_surface = resolution_surface(old);
    let new_surface = resolution_surface(new);
    old_surface
        .keys()
        .chain(new_surface.keys())
        .filter(|term| old_surface.get(*term) != new_surface.get(*term))
        .cloned()
        .collect()
}

fn resolution_surface(nodes: &[GraphNode]) -> HashMap<ResolutionTerm, Vec<String>> {
    let mut surface = HashMap::<ResolutionTerm, Vec<String>>::new();
    for node in nodes {
        for term in node_resolution_terms(node) {
            surface.entry(term).or_default().push(format!(
                "{}|{}|{}",
                node.stable_key,
                node.node_type.storage_name(),
                node.language.as_deref().unwrap_or_default()
            ));
        }
    }
    for candidates in surface.values_mut() {
        candidates.sort();
        candidates.dedup();
    }
    surface
}

fn node_resolution_terms(node: &GraphNode) -> HashSet<ResolutionTerm> {
    let mut terms = HashSet::new();
    add_term(
        &mut terms,
        RelationshipTargetKind::LocalStableKey,
        &node.stable_key,
    );
    if let Some(logical_key) = node
        .metadata
        .get("logical_stable_key")
        .and_then(Value::as_str)
    {
        add_term(
            &mut terms,
            RelationshipTargetKind::LocalStableKey,
            logical_key,
        );
    }
    add_term(
        &mut terms,
        RelationshipTargetKind::QualifiedSymbol,
        &node.name,
    );
    if let Some(qualified_name) = &node.qualified_name {
        add_term(
            &mut terms,
            RelationshipTargetKind::QualifiedSymbol,
            qualified_name,
        );
    }
    if module_like(node) {
        add_term(&mut terms, RelationshipTargetKind::ModulePath, &node.name);
        if let Some(qualified_name) = &node.qualified_name {
            add_term(
                &mut terms,
                RelationshipTargetKind::ModulePath,
                qualified_name,
            );
        }
        if let Some(path) = node.stable_key.strip_prefix("file:") {
            add_term(&mut terms, RelationshipTargetKind::ModulePath, path);
        }
    }
    terms
}

fn add_term(terms: &mut HashSet<ResolutionTerm>, kind: RelationshipTargetKind, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        terms.insert(ResolutionTerm {
            kind,
            value: value.into(),
        });
    }
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

fn affected_relationships(
    source_document_id: &str,
    old_facts: &[GraphRelationshipFact],
    desired_facts: &[GraphRelationshipFact],
    old_edges: &[GraphEdge],
    changed_terms: &HashSet<ResolutionTerm>,
    removed_node_ids: &HashSet<&str>,
) -> Vec<GraphRelationshipIdentity> {
    let mut affected = HashSet::<(String, String)>::new();
    for fact in old_facts.iter().chain(desired_facts) {
        let term = ResolutionTerm {
            kind: fact.target_kind.clone(),
            value: fact.target_value.trim().into(),
        };
        if fact.source_document_id == source_document_id || changed_terms.contains(&term) {
            affected.insert((
                fact.source_document_id.clone(),
                fact.relationship_key.clone(),
            ));
        }
    }
    for edge in old_edges {
        if removed_node_ids.contains(edge.to_node.as_str())
            && let Some(document_id) = &edge.source_document_id
        {
            affected.insert((document_id.clone(), edge.relationship_key.clone()));
        }
    }
    let mut affected: Vec<_> = affected
        .into_iter()
        .map(
            |(source_document_id, relationship_key)| GraphRelationshipIdentity {
                source_document_id,
                relationship_key,
            },
        )
        .collect();
    affected.sort_by(|left, right| {
        left.source_document_id
            .cmp(&right.source_document_id)
            .then_with(|| left.relationship_key.cmp(&right.relationship_key))
    });
    affected
}

fn project_relationships(
    workspace_id: &str,
    nodes: &[GraphNode],
    facts: &[GraphRelationshipFact],
    affected: &[GraphRelationshipIdentity],
    old_edges: &[GraphEdge],
    old_unresolved: &[UnresolvedRelationship],
) -> Result<(Vec<GraphEdge>, Vec<UnresolvedGraphProjection>)> {
    let affected: HashSet<_> = affected
        .iter()
        .map(|identity| {
            (
                identity.source_document_id.as_str(),
                identity.relationship_key.as_str(),
            )
        })
        .collect();
    let old_edges: HashMap<_, _> = old_edges
        .iter()
        .filter_map(|edge| {
            edge.source_document_id
                .as_deref()
                .map(|document_id| ((document_id, edge.relationship_key.as_str()), edge))
        })
        .collect();
    let old_unresolved: HashMap<_, _> = old_unresolved
        .iter()
        .map(|relationship| {
            (
                (
                    relationship.source_document_id.as_str(),
                    relationship.relationship_key.as_str(),
                ),
                relationship,
            )
        })
        .collect();
    let mut edges = Vec::new();
    let mut unresolved = Vec::new();
    for fact in facts {
        let identity = (
            fact.source_document_id.as_str(),
            fact.relationship_key.as_str(),
        );
        if !affected.contains(&identity) {
            continue;
        }
        let request = resolution_request(workspace_id, fact, nodes);
        let outcome = SymbolResolver::resolve_against_nodes(&request, nodes)?;
        match outcome {
            ResolutionOutcome::Resolved { node, basis } => {
                let from_node = fact.from_node.as_ref().ok_or_else(|| {
                    CortexError::Analysis(format!(
                        "relationship {} has no source node",
                        fact.relationship_key
                    ))
                })?;
                let mut edge = GraphEdge::new(
                    workspace_id,
                    &fact.relationship_key,
                    from_node,
                    &node.id,
                    fact.edge_type.clone(),
                );
                edge.relationship_fact_id = Some(fact.id.clone());
                edge.confidence = fact.confidence;
                edge.analyzer_id = fact.analyzer_id.clone();
                edge.analyzer_version = fact.analyzer_version.clone();
                edge.structure_version = fact.structure_version.clone();
                edge.source_document_id = Some(fact.source_document_id.clone());
                edge.source_segment = fact.source_segment.clone();
                edge.content_revision = fact.content_revision;
                edge.metadata = projection_metadata(&fact.metadata, "resolved", Some(basis), &[]);
                if let Some(old) = old_edges.get(&identity)
                    && old.from_node == edge.from_node
                    && old.to_node == edge.to_node
                    && old.edge_type == edge.edge_type
                {
                    edge.id = old.id.clone();
                    edge.created_at = old.created_at;
                }
                edges.push(edge);
            }
            outcome => {
                let (state, candidate_nodes, external_targets) = match &outcome {
                    ResolutionOutcome::Ambiguous {
                        candidates,
                        external_targets,
                        ..
                    } => ("ambiguous", candidates.clone(), external_targets.clone()),
                    ResolutionOutcome::External { target } => {
                        ("external", Vec::new(), vec![target.clone()])
                    }
                    ResolutionOutcome::Unresolved { .. } => ("unresolved", Vec::new(), Vec::new()),
                    ResolutionOutcome::Resolved { .. } => unreachable!(),
                };
                let basis = match outcome {
                    ResolutionOutcome::Ambiguous { basis, .. } => Some(basis),
                    _ => None,
                };
                let now = Utc::now();
                let mut relationship = fact.clone();
                relationship.id = Uuid::new_v4().to_string();
                relationship.created_at = now;
                relationship.updated_at = now;
                relationship.metadata =
                    projection_metadata(&fact.metadata, state, basis, &external_targets);
                if let Some(old) = old_unresolved.get(&identity) {
                    relationship.id = old.id.clone();
                    relationship.created_at = old.created_at;
                }
                unresolved.push(UnresolvedGraphProjection {
                    relationship,
                    candidate_node_ids: candidate_nodes
                        .into_iter()
                        .map(|candidate| candidate.id)
                        .collect(),
                });
            }
        }
    }
    Ok((edges, unresolved))
}

fn resolution_request(
    workspace_id: &str,
    fact: &GraphRelationshipFact,
    nodes: &[GraphNode],
) -> ResolutionRequest {
    let target = match &fact.target_kind {
        RelationshipTargetKind::LocalStableKey => {
            RelationshipTarget::LocalStableKey(fact.target_value.clone())
        }
        RelationshipTargetKind::QualifiedSymbol => {
            RelationshipTarget::QualifiedSymbol(fact.target_value.clone())
        }
        RelationshipTargetKind::ModulePath => {
            RelationshipTarget::ModulePath(fact.target_value.clone())
        }
        RelationshipTargetKind::External => RelationshipTarget::External(fact.target_value.clone()),
        RelationshipTargetKind::Unresolved | RelationshipTargetKind::Other(_) => {
            RelationshipTarget::Unresolved(fact.target_value.clone())
        }
    };
    let language = fact.from_node.as_deref().and_then(|source_id| {
        nodes
            .iter()
            .find(|node| node.id == source_id)
            .and_then(|node| node.language.clone())
    });
    let mut request = ResolutionRequest::new(workspace_id, target);
    request.source_document_id = Some(fact.source_document_id.clone());
    request.source_node_id = fact.from_node.clone();
    request.language = language;
    request.aliases = resolution_aliases(&fact.metadata);
    request.external_module_roots = string_array(&fact.metadata, "external_module_roots");
    request
}

fn resolution_aliases(metadata: &Value) -> Vec<ResolutionAlias> {
    metadata
        .get("resolution_aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(ResolutionAlias::new(
                value.get("alias")?.as_str()?,
                value.get("target")?.as_str()?,
            ))
        })
        .collect()
}

fn string_array(metadata: &Value, key: &str) -> Vec<String> {
    metadata
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn projection_metadata(
    metadata: &Value,
    state: &str,
    basis: Option<ResolutionBasis>,
    external_targets: &[String],
) -> Value {
    let mut metadata = object_metadata(metadata);
    metadata.insert("resolution_state".into(), json!(state));
    if let Some(basis) = basis {
        metadata.insert("resolution_basis".into(), json!(basis));
    }
    if !external_targets.is_empty() {
        metadata.insert("external_targets".into(), json!(external_targets));
    }
    Value::Object(metadata)
}

fn object_metadata(value: &Value) -> Map<String, Value> {
    match value {
        Value::Object(values) => values.clone(),
        value => {
            let mut values = Map::new();
            values.insert("analyzer_metadata".into(), value.clone());
            values
        }
    }
}
