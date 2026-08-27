use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_CONTEXT_TOKEN_BUDGET: usize = 6_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSourceType {
    Code,
    Document,
    Memory,
    Event,
    TaskState,
    SessionState,
    Other(String),
}

impl ContextSourceType {
    pub fn storage_name(&self) -> String {
        match self {
            Self::Code => "code".into(),
            Self::Document => "document".into(),
            Self::Memory => "memory".into(),
            Self::Event => "event".into(),
            Self::TaskState => "task_state".into(),
            Self::SessionState => "session_state".into(),
            Self::Other(value) => format!("other:{value}"),
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "code" => Self::Code,
            "document" => Self::Document,
            "memory" => Self::Memory,
            "event" => Self::Event,
            "task_state" => Self::TaskState,
            "session_state" => Self::SessionState,
            other => Self::Other(other.strip_prefix("other:").unwrap_or(other).to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFreshness {
    Current,
    Historical,
    Superseded,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextSelectionReason {
    DirectSemanticMatch,
    DirectLexicalMatch,
    ActiveTaskReference,
    ActiveWorkingSet,
    RecentDecision,
    RecentFailure,
    NeighborOfRelevantSymbol,
    RelatedFile,
    ExplicitPathScope,
    ResumeState,
    Pinned,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextRequest {
    pub workspace_id: String,
    pub query: Option<String>,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub token_budget: usize,
    pub include_code: bool,
    pub include_documents: bool,
    pub include_memories: bool,
    pub include_events: bool,
    pub path_scope: Vec<String>,
    pub language_scope: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalSessionScope {
    #[default]
    Any,
    Current,
    Previous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecentWindow {
    Hours(u64),
    Days(u64),
}

impl RecentWindow {
    pub fn hours(self) -> u64 {
        match self {
            Self::Hours(hours) => hours,
            Self::Days(days) => days.saturating_mul(24),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TemporalFilter {
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub modified_after: Option<DateTime<Utc>>,
    pub modified_before: Option<DateTime<Utc>>,
    pub recent_within: Option<RecentWindow>,
    pub session_scope: TemporalSessionScope,
    pub include_superseded: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalQuery {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub source_types: Vec<ContextSourceType>,
    pub filter: TemporalFilter,
    pub limit: usize,
}

impl TemporalQuery {
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            session_id: None,
            source_types: Vec::new(),
            filter: TemporalFilter::default(),
            limit: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemporalContextItem {
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub content: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub language: Option<String>,
    pub created_at: DateTime<Utc>,
    pub modified_at: Option<DateTime<Utc>>,
    pub freshness: ContextFreshness,
    pub recency_score: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct TemporalBounds {
    pub created_after: Option<DateTime<Utc>>,
    pub created_before: Option<DateTime<Utc>>,
    pub modified_after: Option<DateTime<Utc>>,
    pub modified_before: Option<DateTime<Utc>>,
    pub activity_after: Option<DateTime<Utc>>,
    pub activity_before: Option<DateTime<Utc>>,
    pub scoped_session_id: Option<String>,
    pub include_superseded: bool,
}

impl ContextRequest {
    pub fn new(workspace_id: impl Into<String>) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            query: None,
            session_id: None,
            task_id: None,
            token_budget: DEFAULT_CONTEXT_TOKEN_BUDGET,
            include_code: true,
            include_documents: true,
            include_memories: true,
            include_events: true,
            path_scope: Vec::new(),
            language_scope: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextScores {
    pub semantic: Option<f32>,
    pub lexical: Option<f32>,
    pub recency: f32,
    pub working_set: f32,
    pub task: f32,
    pub provenance: f32,
    pub final_score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextItem {
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub content: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub language: Option<String>,
    pub freshness: ContextFreshness,
    pub scores: ContextScores,
    pub reasons: Vec<ContextSelectionReason>,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextCandidate {
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub content: String,
    pub path: Option<String>,
    pub symbol: Option<String>,
    pub language: Option<String>,
    pub freshness: ContextFreshness,
    pub scores: ContextScores,
    pub reasons: Vec<ContextSelectionReason>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextCandidatePool {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub candidates: Vec<ContextCandidate>,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPacket {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub summary: Option<String>,
    pub items: Vec<ContextItem>,
    pub token_budget: usize,
    pub estimated_tokens: usize,
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkingSetEntry {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub activation_score: f32,
    pub last_activated_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkingSetSnapshot {
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub entries: Vec<WorkingSetEntry>,
    pub pins: Vec<ContextPin>,
    pub generated_at: DateTime<Utc>,
}

impl WorkingSetEntry {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        source_id: impl Into<String>,
        source_type: ContextSourceType,
        activation_score: f32,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            task_id: None,
            source_id: source_id.into(),
            source_type,
            activation_score,
            last_activated_at: now,
            created_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContextPin {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub source_id: String,
    pub source_type: ContextSourceType,
    pub created_at: DateTime<Utc>,
}

impl ContextPin {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        source_id: impl Into<String>,
        source_type: ContextSourceType,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            task_id: None,
            source_id: source_id.into(),
            source_type,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub content: String,
    pub objective: Option<String>,
    pub completed: Vec<String>,
    pub decision_ids: Vec<String>,
    pub open_problems: Vec<String>,
    pub related_paths: Vec<String>,
    pub related_symbols: Vec<String>,
    pub next_action: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Checkpoint {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            task_id: None,
            content: content.into(),
            objective: None,
            completed: Vec::new(),
            decision_ids: Vec::new(),
            open_problems: Vec::new(),
            related_paths: Vec::new(),
            related_symbols: Vec::new(),
            next_action: None,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySupersession {
    pub workspace_id: String,
    pub superseded_memory_id: String,
    pub superseding_memory_id: String,
    pub created_at: DateTime<Utc>,
}

impl MemorySupersession {
    pub fn new(
        workspace_id: impl Into<String>,
        superseded_memory_id: impl Into<String>,
        superseding_memory_id: impl Into<String>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            superseded_memory_id: superseded_memory_id.into(),
            superseding_memory_id: superseding_memory_id.into(),
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_request_defaults_enable_v0_2_sources() {
        let request = ContextRequest::new("workspace");

        assert_eq!(request.token_budget, DEFAULT_CONTEXT_TOKEN_BUDGET);
        assert!(request.include_code);
        assert!(request.include_documents);
        assert!(request.include_memories);
        assert!(request.include_events);
    }

    #[test]
    fn source_types_round_trip_storage_names() {
        for source_type in [
            ContextSourceType::Code,
            ContextSourceType::Document,
            ContextSourceType::Memory,
            ContextSourceType::Event,
            ContextSourceType::TaskState,
            ContextSourceType::SessionState,
            ContextSourceType::Other("future_source".into()),
        ] {
            assert_eq!(
                ContextSourceType::from_storage(&source_type.storage_name()),
                source_type
            );
        }
    }
}
