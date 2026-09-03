use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const MAX_EPISODE_EVENTS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeType {
    Implementation,
    Debugging,
    Verification,
    Investigation,
    Refactor,
    Configuration,
    DependencyChange,
    ArchitectureDecision,
    Documentation,
    Other,
}

impl EpisodeType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Implementation => "implementation",
            Self::Debugging => "debugging",
            Self::Verification => "verification",
            Self::Investigation => "investigation",
            Self::Refactor => "refactor",
            Self::Configuration => "configuration",
            Self::DependencyChange => "dependency_change",
            Self::ArchitectureDecision => "architecture_decision",
            Self::Documentation => "documentation",
            Self::Other => "other",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "implementation" => Self::Implementation,
            "debugging" => Self::Debugging,
            "verification" => Self::Verification,
            "investigation" => Self::Investigation,
            "refactor" => Self::Refactor,
            "configuration" => Self::Configuration,
            "dependency_change" => Self::DependencyChange,
            "architecture_decision" => Self::ArchitectureDecision,
            "documentation" => Self::Documentation,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeStatus {
    Open,
    Closed,
    Abandoned,
    Invalid,
}

impl EpisodeStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::Abandoned => "abandoned",
            Self::Invalid => "invalid",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "closed" => Self::Closed,
            "abandoned" => Self::Abandoned,
            "invalid" => Self::Invalid,
            _ => Self::Open,
        }
    }

    pub const fn is_terminal(self) -> bool {
        !matches!(self, Self::Open)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EpisodeCreator {
    User,
    NativeHarness,
}

impl EpisodeCreator {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::NativeHarness => "native_harness",
        }
    }

    pub fn from_storage(value: &str) -> Self {
        match value {
            "native_harness" => Self::NativeHarness,
            _ => Self::User,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub episode_type: EpisodeType,
    pub status: EpisodeStatus,
    pub title: Option<String>,
    pub created_by: EpisodeCreator,
    pub version: u64,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl Episode {
    pub fn new(
        workspace_id: impl Into<String>,
        session_id: impl Into<String>,
        task_id: Option<String>,
        episode_type: EpisodeType,
        title: Option<String>,
        created_by: EpisodeCreator,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            task_id,
            episode_type,
            status: EpisodeStatus::Open,
            title,
            created_by,
            version: 0,
            started_at: now,
            ended_at: None,
            created_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeEvent {
    pub workspace_id: String,
    pub episode_id: String,
    pub event_id: String,
    pub ordinal: u64,
    pub associated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeStartRequest {
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub episode_type: EpisodeType,
    pub title: Option<String>,
    pub created_by: EpisodeCreator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeEventAssociationRequest {
    pub workspace_id: String,
    pub episode_id: String,
    pub expected_version: u64,
    pub request_key: String,
    pub event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeTerminalRequest {
    pub workspace_id: String,
    pub episode_id: String,
    pub expected_version: u64,
    pub request_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpisodeListRequest {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub limit: usize,
}
