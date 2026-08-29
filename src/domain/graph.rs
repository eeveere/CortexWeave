use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::RelationshipTarget;
use super::SourceSegment;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeType {
    Workspace,
    Module,
    Namespace,
    Package,
    File,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    Function,
    Method,
    Constructor,
    Type,
    Constant,
    Test,
    UnknownSymbol,
    Other(String),
}

impl GraphNodeType {
    pub fn storage_name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| match self {
                Self::Other(value) => format!("other:{value}"),
                _ => "other".into(),
            })
    }

    pub fn from_storage(value: &str) -> Self {
        serde_json::from_value(Value::String(value.to_owned())).unwrap_or_else(|_| {
            Self::Other(value.strip_prefix("other:").unwrap_or(value).to_owned())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphEdgeType {
    Contains,
    Imports,
    Exports,
    Calls,
    References,
    Implements,
    Extends,
    UsesType,
    Constructs,
    Overrides,
    Tests,
    DeclaredIn,
    DependsOn,
    Other(String),
}

impl GraphEdgeType {
    pub fn storage_name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| match self {
                Self::Other(value) => format!("other:{value}"),
                _ => "other".into(),
            })
    }

    pub fn from_storage(value: &str) -> Self {
        serde_json::from_value(Value::String(value.to_owned())).unwrap_or_else(|_| {
            Self::Other(value.strip_prefix("other:").unwrap_or(value).to_owned())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationshipTargetKind {
    LocalStableKey,
    QualifiedSymbol,
    ModulePath,
    External,
    Unresolved,
    Other(String),
}

impl RelationshipTargetKind {
    pub fn storage_name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| match self {
                Self::Other(value) => format!("other:{value}"),
                _ => "other".into(),
            })
    }

    pub fn from_storage(value: &str) -> Self {
        serde_json::from_value(Value::String(value.to_owned())).unwrap_or_else(|_| {
            Self::Other(value.strip_prefix("other:").unwrap_or(value).to_owned())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphState {
    Current,
    Updating,
    Stale,
    Error,
}

impl GraphState {
    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Updating => "updating",
            Self::Stale => "stale",
            Self::Error => "error",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "current" => Self::Current,
            "updating" => Self::Updating,
            "error" => Self::Error,
            _ => Self::Stale,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceGraphRevision {
    pub workspace_id: String,
    pub content_revision: i64,
    pub graph_content_revision: i64,
    pub graph_schema_version: i64,
    pub graph_state: GraphState,
    pub graph_update_started_at: Option<DateTime<Utc>>,
    pub failed_graph_target_revision: Option<i64>,
    pub last_graph_error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl WorkspaceGraphRevision {
    pub fn is_current(&self) -> bool {
        self.graph_state == GraphState::Current
            && self.content_revision == self.graph_content_revision
            && self.last_graph_error.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub workspace_id: String,
    pub node_type: GraphNodeType,
    pub stable_key: String,
    pub language: Option<String>,
    pub name: String,
    pub qualified_name: Option<String>,
    pub document_id: Option<String>,
    pub chunk_id: Option<String>,
    pub source_segment: Option<SourceSegment>,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
    pub content_revision: i64,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl GraphNode {
    pub fn new(
        workspace_id: impl Into<String>,
        node_type: GraphNodeType,
        stable_key: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            node_type,
            stable_key: stable_key.into(),
            language: None,
            name: name.into(),
            qualified_name: None,
            document_id: None,
            chunk_id: None,
            source_segment: None,
            analyzer_id: "graph-core".into(),
            analyzer_version: "1".into(),
            structure_version: "graph-core:v1".into(),
            content_revision: 0,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub workspace_id: String,
    pub relationship_key: String,
    pub relationship_fact_id: Option<String>,
    pub from_node: String,
    pub to_node: String,
    pub edge_type: GraphEdgeType,
    pub confidence: f32,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
    pub source_document_id: Option<String>,
    pub source_segment: Option<SourceSegment>,
    pub content_revision: i64,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

impl GraphEdge {
    pub fn new(
        workspace_id: impl Into<String>,
        relationship_key: impl Into<String>,
        from_node: impl Into<String>,
        to_node: impl Into<String>,
        edge_type: GraphEdgeType,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            relationship_key: relationship_key.into(),
            relationship_fact_id: None,
            from_node: from_node.into(),
            to_node: to_node.into(),
            edge_type,
            confidence: 1.0,
            analyzer_id: "graph-core".into(),
            analyzer_version: "1".into(),
            structure_version: "graph-core:v1".into(),
            source_document_id: None,
            source_segment: None,
            content_revision: 0,
            metadata: serde_json::json!({}),
            created_at: Utc::now(),
        }
    }
}

/// A source-owned relationship assertion. It remains durable while a resolver
/// changes, removes, or recreates its edge projection.
pub type GraphRelationshipFact = UnresolvedRelationship;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnresolvedRelationship {
    pub id: String,
    pub workspace_id: String,
    pub source_document_id: String,
    pub relationship_key: String,
    pub from_node: Option<String>,
    pub from_stable_key: String,
    pub edge_type: GraphEdgeType,
    pub target_kind: RelationshipTargetKind,
    pub target_value: String,
    pub confidence: f32,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
    pub source_segment: Option<SourceSegment>,
    pub content_revision: i64,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl UnresolvedRelationship {
    pub fn new(
        workspace_id: impl Into<String>,
        source_document_id: impl Into<String>,
        relationship_key: impl Into<String>,
        from_stable_key: impl Into<String>,
        edge_type: GraphEdgeType,
        target_kind: RelationshipTargetKind,
        target_value: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            source_document_id: source_document_id.into(),
            relationship_key: relationship_key.into(),
            from_node: None,
            from_stable_key: from_stable_key.into(),
            edge_type,
            target_kind,
            target_value: target_value.into(),
            confidence: 1.0,
            analyzer_id: "graph-core".into(),
            analyzer_version: "1".into(),
            structure_version: "graph-core:v1".into(),
            source_segment: None,
            content_revision: 0,
            metadata: serde_json::json!({}),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphAnalysisState {
    pub document_id: String,
    pub workspace_id: String,
    pub content_revision: i64,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
    pub last_error: Option<String>,
    pub analyzed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphAnalysisExpectation {
    pub document_id: String,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionAlias {
    pub alias: String,
    pub target: String,
}

impl ResolutionAlias {
    pub fn new(alias: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            alias: alias.into(),
            target: target.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionRequest {
    pub workspace_id: String,
    pub target: RelationshipTarget,
    pub source_document_id: Option<String>,
    pub source_node_id: Option<String>,
    pub language: Option<String>,
    #[serde(default)]
    pub aliases: Vec<ResolutionAlias>,
    #[serde(default)]
    pub external_module_roots: Vec<String>,
}

impl ResolutionRequest {
    pub fn new(workspace_id: impl Into<String>, target: RelationshipTarget) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            target,
            source_document_id: None,
            source_node_id: None,
            language: None,
            aliases: Vec::new(),
            external_module_roots: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionBasis {
    LocalStableKey,
    Alias,
    EnclosingContainer,
    QualifiedName,
    SourceDocument,
    WorkspaceName,
    Module,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ResolutionOutcome {
    Resolved {
        node: Box<GraphNode>,
        basis: ResolutionBasis,
    },
    Ambiguous {
        candidates: Vec<GraphNode>,
        #[serde(default)]
        external_targets: Vec<String>,
        basis: ResolutionBasis,
    },
    External {
        target: String,
    },
    Unresolved {
        target: String,
    },
}

impl ResolutionOutcome {
    pub fn resolved_node(&self) -> Option<&GraphNode> {
        match self {
            Self::Resolved { node, .. } => Some(node.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_storage_names_round_trip() {
        for node_type in [
            GraphNodeType::Function,
            GraphNodeType::Other("future".into()),
        ] {
            assert_eq!(
                GraphNodeType::from_storage(&node_type.storage_name()),
                node_type
            );
        }
        for edge_type in [GraphEdgeType::Calls, GraphEdgeType::Other("future".into())] {
            assert_eq!(
                GraphEdgeType::from_storage(&edge_type.storage_name()),
                edge_type
            );
        }
    }
}
