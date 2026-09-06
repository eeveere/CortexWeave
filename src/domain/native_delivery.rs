//! Retry-safe native ingress. Receipts describe the original commit, not current lifecycle state.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{CortexEvent, Episode, EpisodeStartRequest, Session, Task};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeDeliveryRequest {
    pub request_key: String,
    pub operation: NativeOperation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum NativeOperation {
    StartSession {
        workspace_id: String,
        metadata: Value,
    },
    StartTask {
        workspace_id: String,
        session_id: Option<String>,
        title: String,
        details: Value,
    },
    StartEpisode {
        request: EpisodeStartRequest,
    },
    RecordEvent {
        event: CortexEvent,
    },
}

impl NativeOperation {
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::StartSession { workspace_id, .. } | Self::StartTask { workspace_id, .. } => {
                workspace_id
            }
            Self::StartEpisode { request } => &request.workspace_id,
            Self::RecordEvent { event } => &event.workspace_id,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::StartSession { .. } => "start_session",
            Self::StartTask { .. } => "start_task",
            Self::StartEpisode { .. } => "start_episode",
            Self::RecordEvent { .. } => "record_event",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NativeDeliveryReceipt {
    pub request_key: String,
    pub record: NativeRecord,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "record", rename_all = "snake_case")]
pub enum NativeRecord {
    Session(Session),
    Task(Task),
    Episode(Episode),
    Event(CortexEvent),
}
