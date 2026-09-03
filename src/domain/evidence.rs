use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Maximum serialized JSON bytes accepted by a v1 evidence decoder.
pub const MAX_EVIDENCE_PAYLOAD_BYTES: usize = 64 * 1024;
pub const MAX_EVIDENCE_TEXT_BYTES: usize = 4 * 1024;
pub const MAX_EVIDENCE_IDENTIFIER_BYTES: usize = 256;
pub const MAX_RUST_DIAGNOSTICS: usize = 64;
pub const MAX_TEST_FAILURES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceContract {
    ExternalToolCompletion,
    RustCompilerResult,
    CargoTestResult,
    GenericVerifierResult,
    SourceChangeObservation,
    UserAcceptance,
}

impl EvidenceContract {
    pub const fn id(self) -> &'static str {
        match self {
            Self::ExternalToolCompletion => "cortexweave.external_tool_completion",
            Self::RustCompilerResult => "cortexweave.rust_compiler_result",
            Self::CargoTestResult => "cortexweave.cargo_test_result",
            Self::GenericVerifierResult => "cortexweave.generic_verifier_result",
            Self::SourceChangeObservation => "cortexweave.source_change_observation",
            Self::UserAcceptance => "cortexweave.user_acceptance",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        match value {
            "cortexweave.external_tool_completion" => Some(Self::ExternalToolCompletion),
            "cortexweave.rust_compiler_result" => Some(Self::RustCompilerResult),
            "cortexweave.cargo_test_result" => Some(Self::CargoTestResult),
            "cortexweave.generic_verifier_result" => Some(Self::GenericVerifierResult),
            "cortexweave.source_change_observation" => Some(Self::SourceChangeObservation),
            "cortexweave.user_acceptance" => Some(Self::UserAcceptance),
            _ => None,
        }
    }
}

/// Facts copied from the stored event envelope. They scope the report but do
/// not authenticate the producer or prove the payload's claims.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEnvelope {
    pub event_id: String,
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub occurred_at: DateTime<Utc>,
    pub contract: EvidenceContract,
    pub version: u16,
}

/// A decoded producer report. Except for mechanically derived fields such as
/// `process_outcome`, observation values remain producer assertions. Decoding
/// proves shape and internal consistency only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecodedEvidence {
    pub envelope: EvidenceEnvelope,
    pub observation: EvidenceObservation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum EvidenceObservation {
    ExternalToolCompletion(ToolCompletionEvidence),
    RustCompilerResult(RustCompilerResultEvidence),
    CargoTestResult(CargoTestResultEvidence),
    GenericVerifierResult(GenericVerifierResultEvidence),
    SourceChange(SourceChangeEvidence),
    UserAcceptance(UserAcceptanceEvidence),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCompletionEvidence {
    /// Stable producer-selected tool identity, not an arbitrary command line.
    pub tool: String,
    /// Stable producer-selected operation identity, not an arbitrary command line.
    pub operation: String,
    /// Direct process observation. It does not by itself prove task success.
    pub exit_code: i64,
    pub error_class: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessOutcome {
    ExitedZero,
    ExitedNonzero,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustDiagnosticEvidence {
    pub level: DiagnosticLevel,
    /// A producer-supplied diagnostic code such as `E0308` when available.
    pub code: Option<String>,
    pub message: String,
    /// Producer-reported structured type classes. They are paired so failure
    /// normalization never has to scrape type identity from diagnostic prose.
    pub expected_type: Option<String>,
    pub actual_type: Option<String>,
    /// Workspace-relative only. It is not resolved to a symbol by this decoder.
    pub path: Option<String>,
    pub start_line: Option<u64>,
    pub start_column: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Error,
    Warning,
    Note,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RustCompilerResultEvidence {
    /// Producer-declared verification scope. Later verifier rules decide
    /// whether it is compatible with an episode's subject.
    pub subject: EvidenceSubject,
    /// Derived only from `exit_code`; never a task outcome.
    pub process_outcome: ProcessOutcome,
    pub exit_code: i64,
    pub diagnostics: Vec<RustDiagnosticEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestFailureEvidence {
    pub test_name: Option<String>,
    pub assertion_class: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CargoTestResultEvidence {
    /// Producer-declared verification scope.
    pub subject: EvidenceSubject,
    /// Derived only from `exit_code`; never a task outcome.
    pub process_outcome: ProcessOutcome,
    pub exit_code: i64,
    /// The producer's bounded count of tests actually executed.  A successful
    /// command with zero tests is not verification evidence.
    pub executed_test_count: u64,
    pub failures: Vec<TestFailureEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationResult {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSubjectKind {
    Workspace,
    Package,
    Target,
    Test,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSubject {
    /// Subject kind and value are producer assertions until a registered
    /// verifier rule validates them against the invocation and episode.
    pub kind: EvidenceSubjectKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenericVerifierResultEvidence {
    pub verifier_id: String,
    pub verifier_version: String,
    pub tool: String,
    pub operation: String,
    /// A producer assertion. A later registered verifier rule decides whether
    /// this report is eligible verification evidence.
    pub reported_result: VerificationResult,
    /// Derived only from `exit_code`; never a task outcome.
    pub process_outcome: ProcessOutcome,
    pub exit_code: i64,
    pub subject: EvidenceSubject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceChangeKind {
    Created,
    Modified,
    Removed,
    Renamed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChangeEvidence {
    pub change: SourceChangeKind,
    pub path: String,
    pub previous_path: Option<String>,
    /// Producer-reported revisions. The decoder checks range only; storage
    /// reconciliation must establish whether they identify durable state.
    pub reported_workspace_content_revision: i64,
    pub reported_document_content_revision: i64,
    /// A BLAKE3 digest of observed source bytes. Removal explicitly records None.
    pub reported_content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAcceptanceEvidence {
    /// Producer-reported actor label. Authentication belongs to the ingress
    /// adapter or future acceptance policy, not this decoder.
    pub reported_actor: String,
    pub subject: EvidenceSubject,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvidenceDecodeResult {
    Decoded { evidence: Box<DecodedEvidence> },
    Unsupported { reason: UnsupportedEvidenceReason },
    Invalid { issue: EvidenceDecodeIssue },
}

impl EvidenceDecodeResult {
    pub fn decoded(&self) -> Option<&DecodedEvidence> {
        match self {
            Self::Decoded { evidence } => Some(evidence),
            Self::Unsupported { .. } | Self::Invalid { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UnsupportedEvidenceReason {
    LegacyUnversionedPayload,
    UnknownContract {
        contract: String,
    },
    UnsupportedVersion {
        contract: EvidenceContract,
        version: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceDecodeIssue {
    pub code: String,
    pub field: Option<String>,
    pub message: String,
}

impl EvidenceDecodeIssue {
    pub fn new(
        code: impl Into<String>,
        field: Option<impl Into<String>>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            field: field.map(Into::into),
            message: message.into(),
        }
    }
}
