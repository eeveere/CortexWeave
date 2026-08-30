use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRepairMode {
    IfNeeded,
    Force,
}

impl GraphRepairMode {
    pub fn storage_name(self) -> &'static str {
        match self {
            Self::IfNeeded => "if_needed",
            Self::Force => "force",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "force" => Self::Force,
            _ => Self::IfNeeded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRepairState {
    Active,
    Failed,
    Interrupted,
    Completed,
}

impl GraphRepairState {
    pub fn storage_name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Completed => "completed",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            "failed" => Self::Failed,
            "completed" => Self::Completed,
            _ => Self::Interrupted,
        }
    }

    pub fn blocks_structural_reads(self) -> bool {
        !matches!(self, Self::Completed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRepairReason {
    Current,
    ForceRequested,
    LegacyGraphMissing,
    MissingDocumentAnalysis,
    MissingProjection,
    ProjectionMismatch,
    ContentRevisionMismatch,
    AnalyzerMismatch,
    StructureVersionMismatch,
    PriorFailure,
    InterruptedRepair,
    SourceDrift,
    InProgressElsewhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphRepairDisposition {
    Current,
    Repair,
    RequiresSourceReconciliation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRepairDocumentPlan {
    pub document_id: String,
    pub relative_path: String,
    pub disposition: GraphRepairDisposition,
    pub reason: GraphRepairReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRepairPlan {
    pub workspace_id: String,
    pub mode: GraphRepairMode,
    pub reason: GraphRepairReason,
    pub revision_before: i64,
    pub graph_revision_before: i64,
    pub documents: Vec<GraphRepairDocumentPlan>,
}

impl GraphRepairPlan {
    pub fn documents_considered(&self) -> usize {
        self.documents.len()
    }

    pub fn documents_requiring_repair(&self) -> usize {
        self.documents
            .iter()
            .filter(|document| document.disposition == GraphRepairDisposition::Repair)
            .count()
    }

    pub fn documents_requiring_source_reconciliation(&self) -> usize {
        self.documents
            .iter()
            .filter(|document| {
                document.disposition == GraphRepairDisposition::RequiresSourceReconciliation
            })
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRepairGeneration {
    pub workspace_id: String,
    pub generation_id: String,
    pub mode: GraphRepairMode,
    pub target_content_revision: i64,
    pub state: GraphRepairState,
    pub started_at: DateTime<Utc>,
    pub lease_expires_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub documents_considered: usize,
    pub documents_repaired: usize,
    pub documents_failed: usize,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphProjectionManifest {
    pub document_id: String,
    pub workspace_id: String,
    pub content_revision: i64,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub structure_version: String,
    pub node_count: usize,
    pub fact_count: usize,
    pub edge_count: usize,
    pub unresolved_count: usize,
    pub projected_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphRepairOutcome {
    pub workspace_id: String,
    pub mode: GraphRepairMode,
    pub reason: GraphRepairReason,
    pub revision_before: i64,
    pub revision_after: i64,
    pub graph_revision_before: i64,
    pub graph_revision_after: i64,
    pub documents_considered: usize,
    pub documents_analyzed: usize,
    pub documents_unchanged: usize,
    pub documents_failed: usize,
    pub nodes_projected: usize,
    pub edges_projected: usize,
    pub resolved_relationships: usize,
    pub unresolved_relationships: usize,
    pub embeddings_computed: usize,
    pub source_revision_changed: bool,
    pub final_graph_state: GraphRepairState,
    pub already_completed_elsewhere: bool,
    pub generation_id: Option<String>,
    pub error: Option<String>,
}
