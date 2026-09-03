use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CortexError, Result};

use super::ExperienceRecord;

pub const FAILURE_TO_VERIFICATION_EXTRACTOR_ID: &str = "cortexweave.failure_to_verification";
pub const FAILURE_TO_VERIFICATION_EXTRACTOR_VERSION: &str = "1";

/// The only caller supplied authority for consolidation.  The episode version
/// makes a preview a proposal over one immutable membership frontier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidationRequest {
    pub workspace_id: String,
    pub episode_id: String,
    pub expected_episode_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidationAcceptanceRequest {
    pub request: ConsolidationRequest,
    pub expected_fingerprint: String,
    pub expected_proposal_hash: String,
}

/// The immutable member identity that an acceptance transaction must re-read.
/// It contains no display timestamps or mutable derived state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsolidationInputIdentity {
    pub workspace_id: String,
    pub episode_id: String,
    pub members: Vec<ConsolidationInputMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ConsolidationInputMember {
    pub ordinal: u64,
    pub event_id: String,
    pub event_type: String,
    pub payload_hash: String,
}

pub(crate) fn canonical_json(value: &Value) -> Result<String> {
    fn write(value: &Value, out: &mut String) -> Result<()> {
        match value {
            Value::Null => out.push_str("null"),
            Value::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Value::Number(v) => out.push_str(&v.to_string()),
            Value::String(v) => out.push_str(
                &serde_json::to_string(v).map_err(|e| CortexError::Analysis(e.to_string()))?,
            ),
            Value::Array(v) => {
                out.push('[');
                for (index, item) in v.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    write(item, out)?;
                }
                out.push(']');
            }
            Value::Object(v) => {
                out.push('{');
                let mut keys: Vec<_> = v.keys().collect();
                keys.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
                for (index, key) in keys.into_iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(key)
                            .map_err(|e| CortexError::Analysis(e.to_string()))?,
                    );
                    out.push(':');
                    write(&v[key], out)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }

    let mut out = String::new();
    write(value, &mut out)?;
    Ok(out)
}

pub(crate) fn canonical_event_payload_hash(value: &Value) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"cortexweave.event-payload.v1\0");
    hasher.update(canonical_json(value)?.as_bytes());
    Ok(hasher.finalize().to_hex().to_string())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsolidationNoResultReason {
    EpisodeNotClosed,
    EpisodeIneligible,
    StaleEpisodeVersion,
    EpisodeEmpty,
    EvidenceLimitExceeded,
    UnsupportedPayloadContract,
    InvalidPayloadContract,
    NoSupportedFailure,
    AmbiguousInitialFailure,
    NoAttemptEvidence,
    NoVerificationEvidence,
    AmbiguousVerificationScope,
    ConflictingResults,
    EmptyVerificationRun,
    FingerprintMismatch,
    ProposalHashMismatch,
    ReviewRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidationDiagnostic {
    pub code: String,
    pub message: String,
    pub membership_ordinal: Option<u64>,
    pub event_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalDisposition {
    Automatic,
    ReviewRequired { reasons: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperienceProposal {
    pub record: ExperienceRecord,
    pub fingerprint: String,
    pub proposal_hash: String,
    pub diagnostics: Vec<ConsolidationDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ConsolidationPreview {
    Proposal {
        proposal: Box<ExperienceProposal>,
        disposition: ProposalDisposition,
    },
    NoResult {
        reason: ConsolidationNoResultReason,
        diagnostics: Vec<ConsolidationDiagnostic>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ConsolidationAcceptance {
    Accepted {
        record: Box<ExperienceRecord>,
    },
    NoResult {
        reason: ConsolidationNoResultReason,
        diagnostics: Vec<ConsolidationDiagnostic>,
    },
}
