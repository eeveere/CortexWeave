use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Workspace;

pub const WORKSPACE_DEREGISTRATION_PLAN_TTL_SECONDS: i64 = 300;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeregistrationCounts {
    pub documents: u64,
    pub chunks: u64,
    pub embeddings: u64,
    pub graph_nodes: u64,
    pub graph_edges: u64,
    pub memories: u64,
    pub sessions: u64,
    pub tasks: u64,
    pub events: u64,
    pub episodes: u64,
    pub experiences: u64,
    pub assessments: u64,
    pub working_set_entries: u64,
    pub checkpoints: u64,
    pub test_evidence_receipts: u64,
    pub native_delivery_receipts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeregistrationState {
    pub active_sessions: u64,
    pub open_tasks: u64,
    pub open_episodes: u64,
    pub graph_state: String,
    pub graph_content_revision: u64,
    pub content_revision: u64,
    pub graph_repair_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceDeregistrationPreview {
    pub plan_id: String,
    pub workspace: Workspace,
    pub counts: WorkspaceDeregistrationCounts,
    pub state: WorkspaceDeregistrationState,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeregistrationRequest {
    pub workspace_id: String,
    pub plan_id: String,
    pub request_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDeregistrationReceipt {
    pub workspace_id: String,
    pub plan_id: String,
    pub request_key: String,
    pub counts: WorkspaceDeregistrationCounts,
    pub completed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceDeregistrationStaleReason {
    PlanNotFound,
    PlanExpired,
    WorkspaceNotFound,
    WorkspaceChanged,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkspaceDeregistrationOutcome {
    Deregistered {
        receipt: WorkspaceDeregistrationReceipt,
        replayed: bool,
    },
    StalePreview {
        reason: WorkspaceDeregistrationStaleReason,
    },
}
