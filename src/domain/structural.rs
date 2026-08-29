use serde::{Deserialize, Serialize};

use super::{GraphEdge, GraphNode, WorkspaceGraphRevision};

pub const MAX_STRUCTURAL_NODES: usize = 1_000;
pub const MAX_STRUCTURAL_EDGES: usize = 5_000;
pub const MAX_STRUCTURAL_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralDirection {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructuralReadOptions {
    pub allow_stale: bool,
    pub max_nodes: usize,
    pub max_edges: usize,
    pub max_depth: usize,
}

impl Default for StructuralReadOptions {
    fn default() -> Self {
        Self {
            allow_stale: false,
            max_nodes: 50,
            max_edges: 100,
            max_depth: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralPath {
    pub node_ids: Vec<String>,
    pub edges: Vec<GraphEdge>,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralEvidence {
    pub seed_node_id: String,
    pub node_id: String,
    pub path: StructuralPath,
    pub snapshot: WorkspaceGraphRevision,
    pub limits: StructuralReadOptions,
    pub truncated: bool,
}

impl StructuralPath {
    pub fn distance(&self) -> usize {
        self.edges.len()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuralResult {
    pub snapshot: WorkspaceGraphRevision,
    pub limits: StructuralReadOptions,
    pub seeds: Vec<GraphNode>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub paths: Vec<StructuralPath>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactItem {
    pub node: GraphNode,
    pub path: StructuralPath,
    pub confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImpactReport {
    pub snapshot: WorkspaceGraphRevision,
    pub limits: StructuralReadOptions,
    pub seeds: Vec<GraphNode>,
    pub impacts: Vec<ImpactItem>,
    pub truncated: bool,
}
