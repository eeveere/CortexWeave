use std::{
    collections::{HashSet, VecDeque},
    path::Path,
    sync::Arc,
};

use crate::{
    CortexError, Result,
    domain::{
        GraphEdge, GraphEdgeType, GraphNode, GraphNodeType, GraphState, ImpactItem, ImpactReport,
        MAX_STRUCTURAL_DEPTH, MAX_STRUCTURAL_EDGES, MAX_STRUCTURAL_NODES, StructuralDirection,
        StructuralPath, StructuralReadOptions, StructuralResult, WorkspaceGraphRevision,
    },
    parsing::AnalyzerRegistry,
    storage::{CodeCandidate, SqliteStorage},
};

#[derive(Debug, Clone)]
struct TraversalState {
    node_id: String,
    node_ids: Vec<String>,
    edges: Vec<GraphEdge>,
    confidence: f32,
    depth: usize,
}

pub struct StructuralService {
    storage: Arc<SqliteStorage>,
    analyzers: Arc<AnalyzerRegistry>,
}

impl StructuralService {
    pub fn new(storage: Arc<SqliteStorage>, analyzers: Arc<AnalyzerRegistry>) -> Self {
        Self { storage, analyzers }
    }

    pub async fn graph_snapshot(
        &self,
        workspace_id: &str,
        allow_stale: bool,
    ) -> Result<WorkspaceGraphRevision> {
        let mut snapshot = self
            .storage
            .workspace_graph_revision(workspace_id)
            .await?
            .ok_or_else(|| {
                CortexError::NotFound(format!("workspace graph revision {workspace_id}"))
            })?;
        if self
            .storage
            .workspace_graph_repair(workspace_id)
            .await?
            .is_some_and(|repair| repair.state.blocks_structural_reads())
        {
            return Err(CortexError::Analysis(format!(
                "structural graph for workspace {workspace_id} is unavailable while graph repair is active, failed, or interrupted"
            )));
        }
        if snapshot.is_current() && !self.analysis_versions_are_current(workspace_id).await? {
            snapshot.graph_state = GraphState::Stale;
        }
        if !allow_stale && !snapshot.is_current() {
            return Err(CortexError::Analysis(format!(
                "structural graph for workspace {workspace_id} is {}; reindex the workspace or explicitly allow stale structural evidence",
                snapshot.graph_state.storage_name()
            )));
        }
        Ok(snapshot)
    }

    pub async fn find_symbol(
        &self,
        workspace_id: &str,
        symbol_or_path: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        validate_options(options)?;
        let snapshot = self
            .graph_snapshot(workspace_id, options.allow_stale)
            .await?;
        let seeds = self
            .storage
            .graph_nodes_matching(workspace_id, symbol_or_path, options.max_nodes + 1)
            .await?;
        let truncated = seeds.len() > options.max_nodes;
        let seeds = seeds.into_iter().take(options.max_nodes).collect();
        self.ensure_snapshot_unchanged(workspace_id, &snapshot, options.allow_stale)
            .await?;
        Ok(StructuralResult {
            snapshot,
            limits: options.clone(),
            seeds,
            nodes: Vec::new(),
            edges: Vec::new(),
            paths: Vec::new(),
            truncated,
        })
    }

    pub async fn seed_query(
        &self,
        workspace_id: &str,
        query: &str,
        chunk_ids: &[String],
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        validate_options(options)?;
        let snapshot = self
            .graph_snapshot(workspace_id, options.allow_stale)
            .await?;
        let mut seeds = Vec::new();
        let mut seen = HashSet::new();
        let mut truncated = false;
        for token in structural_query_tokens(query) {
            let matches = self
                .storage
                .graph_nodes_matching(workspace_id, &token, options.max_nodes + 1)
                .await?;
            if matches.len() > options.max_nodes {
                truncated = true;
            }
            for node in matches {
                if seen.insert(node.id.clone()) {
                    seeds.push(node);
                    if seeds.len() == options.max_nodes {
                        truncated = true;
                        break;
                    }
                }
            }
            if seeds.len() == options.max_nodes {
                break;
            }
        }
        if seeds.is_empty() {
            for chunk_id in chunk_ids {
                if seeds.len() == options.max_nodes {
                    truncated = true;
                    break;
                }
                for node in self
                    .storage
                    .graph_nodes_for_chunk(workspace_id, chunk_id, options.max_nodes - seeds.len())
                    .await?
                {
                    if seen.insert(node.id.clone()) {
                        seeds.push(node);
                    }
                }
            }
        }
        seeds.sort_by(|left, right| {
            node_seed_order(left)
                .cmp(&node_seed_order(right))
                .then_with(|| left.stable_key.cmp(&right.stable_key))
        });
        seeds.truncate(options.max_nodes);
        self.ensure_snapshot_unchanged(workspace_id, &snapshot, options.allow_stale)
            .await?;
        Ok(StructuralResult {
            snapshot,
            limits: options.clone(),
            seeds,
            nodes: Vec::new(),
            edges: Vec::new(),
            paths: Vec::new(),
            truncated,
        })
    }

    pub async fn neighbors(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        let mut options = options.clone();
        options.max_depth = 1;
        self.traverse(
            workspace_id,
            &[node_id.to_owned()],
            StructuralDirection::Both,
            &[],
            &options,
        )
        .await
    }

    pub async fn callers(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Incoming,
            &[GraphEdgeType::Calls],
            options,
        )
        .await
    }

    pub async fn callees(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Outgoing,
            &[GraphEdgeType::Calls],
            options,
        )
        .await
    }

    pub async fn references(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Incoming,
            &[GraphEdgeType::References],
            options,
        )
        .await
    }

    pub async fn implementations(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Incoming,
            &[
                GraphEdgeType::Implements,
                GraphEdgeType::Extends,
                GraphEdgeType::Overrides,
            ],
            options,
        )
        .await
    }

    pub async fn tests(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Incoming,
            &[GraphEdgeType::Tests],
            options,
        )
        .await
    }

    pub async fn dependencies(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Outgoing,
            &[GraphEdgeType::Imports, GraphEdgeType::DependsOn],
            options,
        )
        .await
    }

    pub async fn dependents(
        &self,
        workspace_id: &str,
        node_id: &str,
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        self.one_hop(
            workspace_id,
            node_id,
            StructuralDirection::Incoming,
            &[GraphEdgeType::Imports, GraphEdgeType::DependsOn],
            options,
        )
        .await
    }

    pub async fn traverse(
        &self,
        workspace_id: &str,
        seed_node_ids: &[String],
        direction: StructuralDirection,
        edge_types: &[GraphEdgeType],
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        validate_options(options)?;
        let snapshot = self
            .graph_snapshot(workspace_id, options.allow_stale)
            .await?;
        let seeds = self
            .load_seed_nodes(workspace_id, seed_node_ids, options.max_nodes)
            .await?;
        let result = self
            .traverse_with_snapshot(
                workspace_id,
                snapshot,
                seeds,
                direction,
                edge_types,
                options,
            )
            .await?;
        self.ensure_snapshot_unchanged(workspace_id, &result.snapshot, options.allow_stale)
            .await?;
        Ok(result)
    }

    pub async fn impact_from_symbol(
        &self,
        workspace_id: &str,
        symbol: &str,
        options: &StructuralReadOptions,
    ) -> Result<ImpactReport> {
        let found = self.find_symbol(workspace_id, symbol, options).await?;
        self.impact_from_seed_nodes(workspace_id, found.snapshot, found.seeds, options)
            .await
    }

    pub async fn impact_from_path(
        &self,
        workspace_id: &str,
        path: &str,
        options: &StructuralReadOptions,
    ) -> Result<ImpactReport> {
        let found = self
            .find_symbol(workspace_id, &normalize_path(path), options)
            .await?;
        let file_seeds: Vec<_> = found
            .seeds
            .into_iter()
            .filter(|node| node.node_type == GraphNodeType::File)
            .collect();
        self.impact_from_seed_nodes(workspace_id, found.snapshot, file_seeds, options)
            .await
    }

    pub async fn impact_from_nodes(
        &self,
        workspace_id: &str,
        seed_node_ids: &[String],
        options: &StructuralReadOptions,
    ) -> Result<ImpactReport> {
        validate_options(options)?;
        let snapshot = self
            .graph_snapshot(workspace_id, options.allow_stale)
            .await?;
        let seeds = self
            .load_seed_nodes(workspace_id, seed_node_ids, options.max_nodes)
            .await?;
        self.impact_from_seed_nodes(workspace_id, snapshot, seeds, options)
            .await
    }

    pub(crate) async fn code_candidates_for_node(
        &self,
        workspace_id: &str,
        node_id: &str,
        limit: usize,
    ) -> Result<Vec<CodeCandidate>> {
        self.storage
            .code_candidates_for_graph_node(workspace_id, node_id, limit)
            .await
    }

    async fn one_hop(
        &self,
        workspace_id: &str,
        node_id: &str,
        direction: StructuralDirection,
        edge_types: &[GraphEdgeType],
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        let mut options = options.clone();
        options.max_depth = 1;
        self.traverse(
            workspace_id,
            &[node_id.to_owned()],
            direction,
            edge_types,
            &options,
        )
        .await
    }

    async fn traverse_with_snapshot(
        &self,
        workspace_id: &str,
        snapshot: WorkspaceGraphRevision,
        seeds: Vec<GraphNode>,
        direction: StructuralDirection,
        edge_types: &[GraphEdgeType],
        options: &StructuralReadOptions,
    ) -> Result<StructuralResult> {
        let mut queue = VecDeque::new();
        let mut seen: HashSet<String> = seeds.iter().map(|node| node.id.clone()).collect();
        for seed in &seeds {
            queue.push_back(TraversalState {
                node_id: seed.id.clone(),
                node_ids: vec![seed.id.clone()],
                edges: Vec::new(),
                confidence: 1.0,
                depth: 0,
            });
        }
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut edge_ids = HashSet::new();
        let mut paths = Vec::new();
        let mut truncated = false;

        while let Some(state) = queue.pop_front() {
            if state.depth >= options.max_depth {
                continue;
            }
            let remaining_edges = options.max_edges.saturating_sub(edges.len());
            if remaining_edges == 0 || nodes.len() == options.max_nodes {
                truncated = true;
                break;
            }
            let mut adjacent = self
                .adjacent_edges(
                    workspace_id,
                    &state.node_id,
                    direction,
                    edge_types,
                    remaining_edges + 1,
                )
                .await?;
            if adjacent.len() > remaining_edges {
                adjacent.truncate(remaining_edges);
                truncated = true;
            }
            let adjacent_len = adjacent.len();
            for (edge_index, edge) in adjacent.into_iter().enumerate() {
                let next_id = adjacent_node_id(&edge, &state.node_id).ok_or_else(|| {
                    CortexError::Analysis("structural edge does not touch traversal node".into())
                })?;
                if edge_ids.insert(edge.id.clone()) {
                    edges.push(edge.clone());
                }
                if !seen.insert(next_id.to_owned()) {
                    continue;
                }
                let Some(node) = self.storage.graph_node_by_id(workspace_id, next_id).await? else {
                    continue;
                };
                let mut node_ids = state.node_ids.clone();
                node_ids.push(node.id.clone());
                let mut path_edges = state.edges.clone();
                path_edges.push(edge.clone());
                let confidence = (state.confidence * edge.confidence).clamp(0.0, 1.0);
                paths.push(StructuralPath {
                    node_ids: node_ids.clone(),
                    edges: path_edges.clone(),
                    confidence,
                });
                nodes.push(node.clone());
                if nodes.len() == options.max_nodes {
                    truncated = edge_index + 1 < adjacent_len || !queue.is_empty();
                    break;
                }
                queue.push_back(TraversalState {
                    node_id: node.id,
                    node_ids,
                    edges: path_edges,
                    confidence,
                    depth: state.depth + 1,
                });
            }
        }
        Ok(StructuralResult {
            snapshot,
            limits: options.clone(),
            seeds,
            nodes,
            edges,
            paths,
            truncated,
        })
    }

    async fn impact_from_seed_nodes(
        &self,
        workspace_id: &str,
        snapshot: WorkspaceGraphRevision,
        seeds: Vec<GraphNode>,
        options: &StructuralReadOptions,
    ) -> Result<ImpactReport> {
        validate_options(options)?;
        let reverse_types = [
            GraphEdgeType::Calls,
            GraphEdgeType::References,
            GraphEdgeType::Implements,
            GraphEdgeType::Extends,
            GraphEdgeType::UsesType,
            GraphEdgeType::Constructs,
            GraphEdgeType::Overrides,
            GraphEdgeType::Tests,
            GraphEdgeType::Imports,
            GraphEdgeType::DependsOn,
        ];
        let mut queue = VecDeque::new();
        let mut seen: HashSet<String> = seeds.iter().map(|node| node.id.clone()).collect();
        for seed in &seeds {
            queue.push_back((
                seed.clone(),
                StructuralPath {
                    node_ids: vec![seed.id.clone()],
                    edges: Vec::new(),
                    confidence: 1.0,
                },
                0_usize,
            ));
        }
        let mut impacts = Vec::new();
        let mut traversed_edges = 0_usize;
        let mut truncated = false;
        while let Some((current, path, depth)) = queue.pop_front() {
            if depth >= options.max_depth {
                continue;
            }
            let remaining = options.max_edges.saturating_sub(traversed_edges);
            if remaining == 0 || impacts.len() == options.max_nodes {
                truncated = true;
                break;
            }
            let mut adjacent = self
                .storage
                .graph_edges_for_node(
                    workspace_id,
                    &current.id,
                    true,
                    &reverse_types,
                    remaining + 1,
                )
                .await?;
            if depth == 0 && current.node_type == GraphNodeType::File {
                adjacent.extend(
                    self.storage
                        .graph_edges_for_node(
                            workspace_id,
                            &current.id,
                            false,
                            &[GraphEdgeType::Contains],
                            remaining + 1,
                        )
                        .await?,
                );
            }
            adjacent.sort_by(|left, right| {
                left.edge_type
                    .storage_name()
                    .cmp(&right.edge_type.storage_name())
                    .then_with(|| left.relationship_key.cmp(&right.relationship_key))
            });
            adjacent.dedup_by(|left, right| left.id == right.id);
            if adjacent.len() > remaining {
                adjacent.truncate(remaining);
                truncated = true;
            }
            let adjacent_len = adjacent.len();
            for (edge_index, edge) in adjacent.into_iter().enumerate() {
                traversed_edges += 1;
                let next_id = adjacent_node_id(&edge, &current.id).ok_or_else(|| {
                    CortexError::Analysis("impact edge does not touch traversal node".into())
                })?;
                if !seen.insert(next_id.to_owned()) {
                    continue;
                }
                let Some(node) = self.storage.graph_node_by_id(workspace_id, next_id).await? else {
                    continue;
                };
                let mut next_path = path.clone();
                next_path.node_ids.push(node.id.clone());
                next_path.edges.push(edge.clone());
                next_path.confidence = (next_path.confidence * edge.confidence).clamp(0.0, 1.0);
                impacts.push(ImpactItem {
                    node: node.clone(),
                    confidence: next_path.confidence,
                    path: next_path.clone(),
                });
                queue.push_back((node, next_path, depth + 1));
                if impacts.len() == options.max_nodes {
                    truncated = edge_index + 1 < adjacent_len || !queue.is_empty();
                    break;
                }
            }
        }
        impacts.sort_by(|left, right| {
            left.path
                .distance()
                .cmp(&right.path.distance())
                .then_with(|| right.confidence.total_cmp(&left.confidence))
                .then_with(|| left.node.stable_key.cmp(&right.node.stable_key))
        });
        let report = ImpactReport {
            snapshot,
            limits: options.clone(),
            seeds,
            impacts,
            truncated,
        };
        self.ensure_snapshot_unchanged(workspace_id, &report.snapshot, options.allow_stale)
            .await?;
        Ok(report)
    }

    async fn adjacent_edges(
        &self,
        workspace_id: &str,
        node_id: &str,
        direction: StructuralDirection,
        edge_types: &[GraphEdgeType],
        limit: usize,
    ) -> Result<Vec<GraphEdge>> {
        let mut edges = Vec::new();
        if matches!(
            direction,
            StructuralDirection::Outgoing | StructuralDirection::Both
        ) {
            edges.extend(
                self.storage
                    .graph_edges_for_node(workspace_id, node_id, false, edge_types, limit)
                    .await?,
            );
        }
        if matches!(
            direction,
            StructuralDirection::Incoming | StructuralDirection::Both
        ) {
            edges.extend(
                self.storage
                    .graph_edges_for_node(workspace_id, node_id, true, edge_types, limit)
                    .await?,
            );
        }
        edges.sort_by(|left, right| {
            left.edge_type
                .storage_name()
                .cmp(&right.edge_type.storage_name())
                .then_with(|| left.relationship_key.cmp(&right.relationship_key))
                .then_with(|| left.id.cmp(&right.id))
        });
        edges.dedup_by(|left, right| left.id == right.id);
        edges.truncate(limit);
        Ok(edges)
    }

    async fn load_seed_nodes(
        &self,
        workspace_id: &str,
        node_ids: &[String],
        max_nodes: usize,
    ) -> Result<Vec<GraphNode>> {
        let mut seeds = Vec::new();
        let mut seen = HashSet::new();
        for node_id in node_ids {
            if !seen.insert(node_id) {
                continue;
            }
            if seen.len() > max_nodes {
                return Err(CortexError::Analysis(format!(
                    "structural seed count exceeds the effective node bound of {max_nodes}"
                )));
            }
            let node = self
                .storage
                .graph_node_by_id(workspace_id, node_id)
                .await?
                .ok_or_else(|| {
                    CortexError::NotFound(format!(
                        "graph node {node_id} in workspace {workspace_id}"
                    ))
                })?;
            seeds.push(node);
        }
        seeds.sort_by(|left, right| left.stable_key.cmp(&right.stable_key));
        Ok(seeds)
    }

    async fn analysis_versions_are_current(&self, workspace_id: &str) -> Result<bool> {
        for document in self.storage.list_documents(workspace_id).await? {
            let analyzer = self.analyzers.for_path(Path::new(&document.relative_path));
            let Some(state) = self.storage.graph_analysis_state(&document.id).await? else {
                return Ok(false);
            };
            if state.content_revision != document.content_revision
                || state.analyzer_id != analyzer.analyzer_id()
                || state.analyzer_version != analyzer.analyzer_version()
                || state.structure_version != analyzer.structure_version()
                || state.last_error.is_some()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn ensure_snapshot_unchanged(
        &self,
        workspace_id: &str,
        before: &WorkspaceGraphRevision,
        allow_stale: bool,
    ) -> Result<()> {
        let after = self.graph_snapshot(workspace_id, allow_stale).await?;
        if &after != before {
            return Err(CortexError::Analysis(format!(
                "structural graph for workspace {workspace_id} changed during the read; retry against one graph snapshot"
            )));
        }
        Ok(())
    }
}

fn validate_options(options: &StructuralReadOptions) -> Result<()> {
    if options.max_nodes == 0
        || options.max_nodes > MAX_STRUCTURAL_NODES
        || options.max_edges == 0
        || options.max_edges > MAX_STRUCTURAL_EDGES
        || options.max_depth == 0
        || options.max_depth > MAX_STRUCTURAL_DEPTH
    {
        return Err(CortexError::Analysis(format!(
            "structural bounds require 1..={MAX_STRUCTURAL_NODES} nodes, 1..={MAX_STRUCTURAL_EDGES} edges, and 1..={MAX_STRUCTURAL_DEPTH} depth"
        )));
    }
    Ok(())
}

fn adjacent_node_id<'a>(edge: &'a GraphEdge, current_node_id: &str) -> Option<&'a str> {
    if edge.from_node == current_node_id {
        Some(&edge.to_node)
    } else if edge.to_node == current_node_id {
        Some(&edge.from_node)
    } else {
        None
    }
}

fn structural_query_tokens(query: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "a",
        "an",
        "and",
        "are",
        "break",
        "by",
        "calls",
        "change",
        "changes",
        "could",
        "dependencies",
        "dependents",
        "does",
        "for",
        "how",
        "if",
        "implemented",
        "implements",
        "in",
        "is",
        "of",
        "on",
        "references",
        "tests",
        "the",
        "this",
        "uses",
        "what",
        "where",
        "which",
        "work",
    ];
    let mut tokens: Vec<_> = query
        .split(|character: char| {
            !character.is_alphanumeric() && !matches!(character, '_' | ':' | '.' | '/' | '\\')
        })
        .map(str::trim)
        .filter(|token| token.len() > 1)
        .filter(|token| !STOP_WORDS.contains(&token.to_ascii_lowercase().as_str()))
        .map(normalize_path)
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn normalize_path(value: &str) -> String {
    let mut segments = Vec::new();
    let normalized = value.trim().replace('\\', "/");
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

fn node_seed_order(node: &GraphNode) -> (bool, bool) {
    (
        node.node_type == GraphNodeType::File,
        node.node_type == GraphNodeType::UnknownSymbol,
    )
}
