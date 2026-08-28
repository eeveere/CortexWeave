use serde::{Deserialize, Serialize};

use crate::{
    domain::{
        ContextPacket, ContextScores, ContextSourceType, Session, SourceSegment, Task, Workspace,
    },
    retrieval::RetrievalResult,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessContextRequest {
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: String,
    pub query: String,
    pub token_budget: usize,
}

impl HarnessContextRequest {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        task_id: impl Into<String>,
        query: impl Into<String>,
        token_budget: usize,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            task_id: task_id.into(),
            query: query.into(),
            token_budget,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessSelectedSource {
    pub workspace_id: String,
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub path: Option<String>,
    pub symbol: Option<String>,
    #[serde(default)]
    pub source_segments: Vec<SourceSegment>,
    pub scores: ContextScores,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessContext {
    pub workspace: Workspace,
    pub session: Session,
    pub task: Task,
    pub packet: ContextPacket,
    pub selected_sources: Vec<HarnessSelectedSource>,
}

impl HarnessContext {
    pub fn evaluate_with(
        &self,
        policy: &impl HarnessContextPolicy,
    ) -> ContextSufficiencyAssessment {
        policy.evaluate(self)
    }
}

pub trait HarnessContextPolicy {
    fn evaluate(&self, context: &HarnessContext) -> ContextSufficiencyAssessment;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSufficiencyAssessment {
    pub sufficient: bool,
    pub selected_chunk_ids_to_hydrate: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessHydrationRequest {
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: String,
    pub selected_sources: Vec<HarnessSelectedSource>,
    pub chunk_ids: Vec<String>,
    pub override_reason: Option<String>,
}

impl HarnessHydrationRequest {
    pub fn from_context(context: &HarnessContext, chunk_ids: Vec<String>) -> Self {
        Self {
            workspace_id: context.workspace.id.clone(),
            session_id: context.session.id.clone(),
            task_id: context.task.id.clone(),
            selected_sources: context.selected_sources.clone(),
            chunk_ids,
            override_reason: None,
        }
    }

    pub fn with_override_reason(mut self, reason: impl Into<String>) -> Self {
        self.override_reason = Some(reason.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HydrationAuthorization {
    PacketSelection,
    HarnessOverride {
        reason: String,
        audit_event_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "scores", rename_all = "snake_case")]
pub enum HydrationScoreProvenance {
    PacketSelection(ContextScores),
    OutOfPacketNotScored,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HydratedContextSource {
    pub source: RetrievalResult,
    pub authorization: HydrationAuthorization,
    pub score_provenance: HydrationScoreProvenance,
}
