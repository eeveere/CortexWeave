use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    CortexEvent, EvidenceDecodeResult, FailureNormalizationResult, ProcessOutcome, VerifierRule,
};

pub const MAX_TEST_CASES: usize = 256;
pub const MAX_TEST_CHILD_OBSERVATIONS: usize = 256;
pub const MAX_TEST_NAMESPACE_COMPONENTS: usize = 16;
pub const MAX_TEST_RUN_ERRORS: usize = 64;
pub const MAX_TEST_VERIFICATION_INPUTS: usize = 32;
pub const MAX_TEST_ARTIFACTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionedTestComponent {
    pub id: String,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestSelectionKind {
    File,
    Module,
    Class,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestCaseStatus {
    Passed,
    AssertionFailed,
    Errored,
    Skipped,
    Todo,
    Pending,
    ExpectedFailed,
    UnexpectedSuccessful,
}

impl TestCaseStatus {
    pub const fn was_executed(self) -> bool {
        !matches!(self, Self::Skipped | Self::Todo | Self::Pending)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCaseIdentity {
    /// Ordered suite or module/class components. Components are retained as an
    /// array so identity never depends on an ambiguous display-name delimiter.
    pub namespace: Vec<String>,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCaseObservation {
    pub identity: TestCaseIdentity,
    pub status: TestCaseStatus,
    pub error_class: Option<String>,
    pub retry_configured: bool,
    pub retry_count: u32,
    pub repeat_configured: bool,
    pub repeat_count: u32,
    pub flaky: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestChildObservation {
    pub parent: TestCaseIdentity,
    /// A producer-local discriminator used only to keep raw child observations
    /// distinct. The initial profiles never use it as stable failure identity.
    pub ordinal: u32,
    pub description: Option<String>,
    pub status: TestCaseStatus,
    pub error_class: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestRunErrorPhase {
    Discovery,
    Import,
    Setup,
    Teardown,
    Cleanup,
    Runtime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRunError {
    pub phase: TestRunErrorPhase,
    pub error_class: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestExecutionOrigin {
    Current,
    Cached,
    Replayed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestSnapshotMode {
    NotApplicable,
    Disabled,
    CreateMissing,
    UpdateAll,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRunSettings {
    pub focused_only: bool,
    pub name_filter: bool,
    pub sharded: bool,
    pub bail: bool,
    pub watch: bool,
    pub snapshot_mode: TestSnapshotMode,
    pub pass_with_no_tests: bool,
    pub execution_origin: TestExecutionOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationInputKind {
    TestFile,
    Configuration,
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum VerificationInputState {
    Present {
        before_digest: String,
        after_digest: String,
    },
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestVerificationInput {
    pub kind: VerificationInputKind,
    pub path: String,
    pub observed: VerificationInputState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestArtifactEvidence {
    pub kind: String,
    pub digest: String,
    pub reference: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentTestCounts {
    pub discovered: u64,
    pub executed: u64,
    pub passed: u64,
    pub assertion_failed: u64,
    pub errored: u64,
    pub skipped: u64,
    pub todo: u64,
    pub pending: u64,
    pub expected_failed: u64,
    pub unexpected_successful: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildTestCounts {
    pub observed: u64,
    pub passed: u64,
    pub assertion_failed: u64,
    pub errored: u64,
    pub skipped: u64,
    pub todo: u64,
    pub pending: u64,
    pub expected_failed: u64,
    pub unexpected_successful: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRunCounts {
    pub parents: ParentTestCounts,
    pub children: ChildTestCounts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRunSelection {
    /// Normalized workspace-relative roots; `.` denotes the workspace root.
    pub project_root: String,
    pub import_root: String,
    pub test_file: String,
    pub kind: TestSelectionKind,
    pub value: String,
    pub settings_digest: String,
    pub inventory_complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRunResultEvidence {
    pub producer: VersionedTestComponent,
    pub runner: VersionedTestComponent,
    pub runtime: VersionedTestComponent,
    pub profile: VersionedTestComponent,
    pub run_id: String,
    pub operation: String,
    pub completed: bool,
    pub process_outcome: Option<ProcessOutcome>,
    pub exit_code: Option<i64>,
    pub language: Option<String>,
    pub environment_fingerprint: String,
    pub selection: TestRunSelection,
    pub cases: Vec<TestCaseObservation>,
    pub children: Vec<TestChildObservation>,
    pub counts: TestRunCounts,
    pub errors: Vec<TestRunError>,
    pub settings: TestRunSettings,
    pub verification_inputs: Vec<TestVerificationInput>,
    pub artifacts: Vec<TestArtifactEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestEvidenceProfile {
    pub id: String,
    pub version: String,
    pub producer: VersionedTestComponent,
    pub runner: VersionedTestComponent,
    pub runtime: VersionedTestComponent,
    pub operation: String,
    pub selection_kinds: BTreeSet<TestSelectionKind>,
    pub supported_languages: BTreeSet<String>,
}

impl TestEvidenceProfile {
    pub fn vitest_4_1_10_node_24_19_0() -> Self {
        Self {
            id: "cortexweave.vitest.file".into(),
            version: "1".into(),
            producer: VersionedTestComponent {
                id: "cortexweave.vitest_capture".into(),
                version: "1".into(),
            },
            runner: VersionedTestComponent {
                id: "vitest".into(),
                version: "4.1.10".into(),
            },
            runtime: VersionedTestComponent {
                id: "node".into(),
                version: "24.19.0".into(),
            },
            operation: "test".into(),
            selection_kinds: [TestSelectionKind::File].into_iter().collect(),
            supported_languages: ["javascript".into(), "typescript".into()]
                .into_iter()
                .collect(),
        }
    }

    pub fn unittest_python_3_14_7() -> Self {
        Self {
            id: "cortexweave.unittest.module_or_class".into(),
            version: "1".into(),
            producer: VersionedTestComponent {
                id: "cortexweave.unittest_capture".into(),
                version: "1".into(),
            },
            runner: VersionedTestComponent {
                id: "python.unittest".into(),
                version: "3.14.7".into(),
            },
            runtime: VersionedTestComponent {
                id: "python".into(),
                version: "3.14.7".into(),
            },
            operation: "test".into(),
            selection_kinds: [TestSelectionKind::Module, TestSelectionKind::Class]
                .into_iter()
                .collect(),
            supported_languages: ["python".into()].into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestRunEligibilityStatus {
    EligiblePassed,
    EligibleFailed,
    Ineligible,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRunEligibilityIssue {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestRunEligibility {
    pub status: TestRunEligibilityStatus,
    pub profile_id: Option<String>,
    pub profile_version: Option<String>,
    pub issues: Vec<TestRunEligibilityIssue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestEvidenceRecordRequest {
    pub workspace_id: String,
    pub session_id: String,
    pub task_id: Option<String>,
    pub request_key: String,
    pub bundle: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestEvidenceRecordResult {
    pub event: CortexEvent,
    pub replayed: bool,
    pub eligibility: TestRunEligibility,
    pub failure_normalization: FailureNormalizationResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventEvidenceInspection {
    pub event: CortexEvent,
    pub decoding: EvidenceDecodeResult,
    pub test_run_eligibility: Option<TestRunEligibility>,
    pub failure_normalization: Option<FailureNormalizationResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceCapabilityLimits {
    pub normalized_payload_bytes: usize,
    pub identifier_bytes: usize,
    pub text_bytes: usize,
    pub parent_cases: usize,
    pub child_observations: usize,
    pub namespace_components: usize,
    pub run_errors: usize,
    pub verification_inputs: usize,
    pub artifacts: usize,
    pub failure_component_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceCapabilities {
    pub contracts: Vec<(String, u16)>,
    pub test_profiles: Vec<TestEvidenceProfile>,
    pub failure_normalizers: Vec<(String, String, String)>,
    pub generic_verifier_rules: Vec<VerifierRule>,
    pub limits: EvidenceCapabilityLimits,
    pub code_analyzer_capabilities_are_separate: bool,
    pub unsupported_test_integrations: Vec<String>,
    pub unsupported_aggregate_proof: Vec<String>,
}
