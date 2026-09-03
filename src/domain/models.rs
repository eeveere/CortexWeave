use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::SymbolKind;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub root_path: String,
    pub name: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Workspace {
    pub fn new(root_path: impl Into<String>, name: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            root_path: root_path.into(),
            name: name.into(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub workspace_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub metadata: Value,
}

impl Session {
    pub fn new(workspace_id: impl Into<String>, metadata: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            started_at: Utc::now(),
            ended_at: None,
            metadata,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Active,
    Completed,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub title: String,
    pub status: TaskStatus,
    pub details: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl Task {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: Option<String>,
        title: impl Into<String>,
        details: Value,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id,
            title: title.into(),
            status: TaskStatus::Pending,
            details,
            created_at: now,
            updated_at: now,
            completed_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub workspace_id: String,
    pub relative_path: String,
    pub language: String,
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub segmentation_id: String,
    #[serde(default)]
    pub content_revision: i64,
    pub content_hash: String,
    pub size_bytes: i64,
    pub modified_at_ns: Option<i64>,
    pub indexed_at: DateTime<Utc>,
}

impl Document {
    pub fn new(workspace_id: impl Into<String>, relative_path: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            relative_path: relative_path.into(),
            language: "text".into(),
            analyzer_id: "generic".into(),
            analyzer_version: "1".into(),
            segmentation_id: "legacy-v0.1".into(),
            content_revision: 0,
            content_hash: String::new(),
            size_bytes: 0,
            modified_at_ns: None,
            indexed_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredChunk {
    pub id: String,
    pub document_id: String,
    pub stable_key: String,
    pub language: String,
    pub symbol: Option<String>,
    pub qualified_symbol: Option<String>,
    pub symbol_kind: Option<SymbolKind>,
    pub start_byte: i64,
    pub end_byte: i64,
    pub start_line: i64,
    pub end_line: i64,
    pub content: String,
    pub content_hash: String,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl StoredChunk {
    pub fn new(
        document_id: impl Into<String>,
        stable_key: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let now = Utc::now();
        let content = content.into();
        Self {
            id: Uuid::new_v4().to_string(),
            document_id: document_id.into(),
            stable_key: stable_key.into(),
            language: "text".into(),
            symbol: None,
            qualified_symbol: None,
            symbol_kind: None,
            start_byte: 0,
            end_byte: content.len() as i64,
            start_line: 1,
            end_line: 1,
            content_hash: blake3::hash(content.as_bytes()).to_hex().to_string(),
            content,
            metadata: json!({}),
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingRecord {
    pub chunk_id: String,
    pub model: String,
    pub dimension: usize,
    pub vector: Vec<f32>,
    pub created_at: DateTime<Utc>,
}

impl EmbeddingRecord {
    pub fn new(chunk_id: impl Into<String>, model: impl Into<String>, vector: Vec<f32>) -> Self {
        Self {
            chunk_id: chunk_id.into(),
            model: model.into(),
            dimension: vector.len(),
            vector,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Decision,
    Observation,
    Failure,
    Solution,
    Todo,
    Note,
    Checkpoint,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Observation => "observation",
            Self::Failure => "failure",
            Self::Solution => "solution",
            Self::Todo => "todo",
            Self::Note => "note",
            Self::Checkpoint => "checkpoint",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "decision" => Self::Decision,
            "observation" => Self::Observation,
            "failure" => Self::Failure,
            "solution" => Self::Solution,
            "todo" => Self::Todo,
            "checkpoint" => Self::Checkpoint,
            _ => Self::Note,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOrigin {
    #[default]
    HumanAuthorized,
    Imported,
}

impl MemoryOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HumanAuthorized => "human_authorized",
            Self::Imported => "imported",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "imported" => Self::Imported,
            _ => Self::HumanAuthorized,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryTrust {
    #[default]
    Trusted,
    Unreviewed,
    Rejected,
}

impl MemoryTrust {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::Unreviewed => "unreviewed",
            Self::Rejected => "rejected",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "unreviewed" => Self::Unreviewed,
            "rejected" => Self::Rejected,
            _ => Self::Trusted,
        }
    }

    pub fn is_context_eligible(self) -> bool {
        self == Self::Trusted
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSegment {
    pub source: String,
    pub start_byte: u64,
    pub end_byte: u64,
}

impl SourceSegment {
    pub fn new(source: impl Into<String>, start_byte: u64, end_byte: u64) -> Self {
        Self {
            source: source.into(),
            start_byte,
            end_byte,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryClaim {
    pub key: String,
    pub value: Value,
}

impl MemoryClaim {
    pub fn new(key: impl Into<String>, value: Value) -> Self {
        Self {
            key: key.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryRecord {
    pub id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub kind: MemoryKind,
    pub content: String,
    pub related_paths: Vec<String>,
    pub metadata: Value,
    #[serde(default)]
    pub origin: MemoryOrigin,
    #[serde(default)]
    pub trust: MemoryTrust,
    #[serde(default)]
    pub source_segments: Vec<SourceSegment>,
    #[serde(default)]
    pub claim: Option<MemoryClaim>,
    pub created_at: DateTime<Utc>,
}

impl MemoryRecord {
    pub fn new(
        workspace_id: impl Into<String>,
        kind: MemoryKind,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id: None,
            task_id: None,
            kind,
            content: content.into(),
            related_paths: Vec::new(),
            metadata: json!({}),
            origin: MemoryOrigin::HumanAuthorized,
            trust: MemoryTrust::Trusted,
            source_segments: Vec::new(),
            claim: None,
            created_at: Utc::now(),
        }
    }

    pub fn imported(
        workspace_id: impl Into<String>,
        kind: MemoryKind,
        content: impl Into<String>,
        source_segments: Vec<SourceSegment>,
    ) -> Self {
        let mut memory = Self::new(workspace_id, kind, content);
        memory.origin = MemoryOrigin::Imported;
        memory.trust = MemoryTrust::Unreviewed;
        memory.source_segments = source_segments;
        memory
    }

    pub fn metadata_for_storage(&self) -> Value {
        let mut metadata = self.metadata.clone();
        if !metadata.is_object() {
            metadata = json!({ "value": metadata });
        }
        if !self.related_paths.is_empty() {
            metadata["related_paths"] = json!(self.related_paths);
        }
        metadata
    }

    pub fn related_paths_from_metadata(metadata: &Value) -> Vec<String> {
        metadata
            .get("related_paths")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryTrustReview {
    pub id: String,
    pub workspace_id: String,
    pub memory_id: String,
    pub previous_trust: MemoryTrust,
    pub new_trust: MemoryTrust,
    pub reviewed_by: String,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}

impl MemoryTrustReview {
    pub fn new(
        workspace_id: impl Into<String>,
        memory_id: impl Into<String>,
        previous_trust: MemoryTrust,
        new_trust: MemoryTrust,
        reviewed_by: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            memory_id: memory_id.into(),
            previous_trust,
            new_trust,
            reviewed_by: reviewed_by.into(),
            reason: reason.into(),
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    FileCreated,
    FileModified,
    FileRemoved,
    FileRenamed,
    MemoryRecorded,
    SessionStarted,
    SessionEnded,
    TaskStarted,
    TaskUpdated,
    TaskCompleted,
    ExternalToolStarted,
    ExternalToolFinished,
    CompilerResult,
    TestResult,
    UserAcceptance,
    ContextHydrationOverride,
    Other(String),
}

impl EventType {
    pub fn storage_name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .unwrap_or_else(|| match self {
                Self::Other(value) => value.clone(),
                _ => "other".into(),
            })
    }

    pub fn from_storage(value: &str) -> Self {
        serde_json::from_value(Value::String(value.to_owned()))
            .unwrap_or_else(|_| Self::Other(value.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CortexEvent {
    pub id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub event_type: EventType,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

impl CortexEvent {
    pub fn new(workspace_id: impl Into<String>, event_type: EventType, payload: Value) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id: None,
            task_id: None,
            event_type,
            payload,
            created_at: Utc::now(),
        }
    }
}
