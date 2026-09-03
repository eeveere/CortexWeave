use std::{collections::BTreeMap, sync::Arc};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    CortexError, Result,
    domain::{
        CargoTestResultEvidence, DecodedEvidence, DiagnosticLevel, EvidenceContract,
        EvidenceDecodeIssue, EvidenceDecodeResult, EvidenceEnvelope, EvidenceObservation,
        EvidenceSubject, EvidenceSubjectKind, GenericVerifierResultEvidence,
        MAX_EVIDENCE_IDENTIFIER_BYTES, MAX_EVIDENCE_PAYLOAD_BYTES, MAX_EVIDENCE_TEXT_BYTES,
        MAX_RUST_DIAGNOSTICS, MAX_TEST_FAILURES, ProcessOutcome, RustCompilerResultEvidence,
        RustDiagnosticEvidence, SourceChangeEvidence, SourceChangeKind, TestFailureEvidence,
        ToolCompletionEvidence, UnsupportedEvidenceReason, UserAcceptanceEvidence,
        VerificationResult,
    },
};

type DecodeResult<T> = std::result::Result<T, EvidenceDecodeIssue>;

pub trait EventEvidenceDecoder: Send + Sync {
    fn contract(&self) -> EvidenceContract;
    fn version(&self) -> u16;
    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool;
    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation>;
}

/// Deterministic, contract-addressed decoder lookup. It never dispatches by
/// best-effort payload shape, so one payload cannot gain meaning from decoder
/// registration order.
pub struct EventEvidenceDecoderRegistry {
    decoders: BTreeMap<(String, u16), Arc<dyn EventEvidenceDecoder>>,
}

impl EventEvidenceDecoderRegistry {
    pub fn new(decoders: Vec<Arc<dyn EventEvidenceDecoder>>) -> Result<Self> {
        let mut registered = BTreeMap::new();
        for decoder in decoders {
            let key = (decoder.contract().id().to_owned(), decoder.version());
            if registered.insert(key.clone(), decoder).is_some() {
                return Err(CortexError::Configuration(format!(
                    "ambiguous evidence decoder registration for {} v{}",
                    key.0, key.1
                )));
            }
        }
        Ok(Self {
            decoders: registered,
        })
    }

    pub fn standard() -> Result<Self> {
        Self::new(vec![
            Arc::new(ExternalToolCompletionDecoder),
            Arc::new(RustCompilerResultDecoder),
            Arc::new(CargoTestResultDecoder),
            Arc::new(GenericVerifierResultDecoder),
            Arc::new(SourceChangeObservationDecoder),
            Arc::new(UserAcceptanceDecoder),
        ])
    }

    pub fn decode(&self, event: &crate::domain::CortexEvent) -> EvidenceDecodeResult {
        let payload_size = match serde_json::to_vec(&event.payload) {
            Ok(bytes) => bytes.len(),
            Err(error) => {
                return invalid(
                    "payload_serialization_failed",
                    None::<String>,
                    error.to_string(),
                );
            }
        };
        if payload_size > MAX_EVIDENCE_PAYLOAD_BYTES {
            return invalid(
                "payload_too_large",
                None::<String>,
                format!(
                    "evidence payload is {payload_size} bytes; maximum is {MAX_EVIDENCE_PAYLOAD_BYTES}"
                ),
            );
        }

        let (contract, version) = match header(&event.payload) {
            Ok(header) => header,
            Err(result) => return result,
        };
        let Some(contract) = EvidenceContract::from_id(&contract) else {
            return EvidenceDecodeResult::Unsupported {
                reason: UnsupportedEvidenceReason::UnknownContract { contract },
            };
        };
        let Some(decoder) = self.decoders.get(&(contract.id().to_owned(), version)) else {
            return EvidenceDecodeResult::Unsupported {
                reason: UnsupportedEvidenceReason::UnsupportedVersion { contract, version },
            };
        };
        if let Err(issue) = validate_event_provenance(event) {
            return EvidenceDecodeResult::Invalid { issue };
        }
        if !decoder.supports_event_type(&event.event_type) {
            return invalid(
                "event_type_mismatch",
                Some("event_type"),
                format!(
                    "{} v{} does not support event type {}",
                    contract.id(),
                    version,
                    event.event_type.storage_name()
                ),
            );
        }
        match decoder.decode_observation(event) {
            Ok(observation) => EvidenceDecodeResult::Decoded {
                evidence: Box::new(DecodedEvidence {
                    envelope: EvidenceEnvelope {
                        event_id: event.id.clone(),
                        workspace_id: event.workspace_id.clone(),
                        session_id: event
                            .session_id
                            .clone()
                            .expect("validated evidence session provenance"),
                        task_id: event.task_id.clone(),
                        occurred_at: event.created_at,
                        contract,
                        version,
                    },
                    observation,
                }),
            },
            Err(issue) => EvidenceDecodeResult::Invalid { issue },
        }
    }

    pub fn identities(&self) -> Vec<(String, u16)> {
        self.decoders.keys().cloned().collect()
    }
}

pub struct EvidenceService {
    registry: EventEvidenceDecoderRegistry,
}

impl EvidenceService {
    pub fn standard() -> Result<Self> {
        Ok(Self {
            registry: EventEvidenceDecoderRegistry::standard()?,
        })
    }

    pub fn with_registry(registry: EventEvidenceDecoderRegistry) -> Self {
        Self { registry }
    }

    /// Diagnoses an immutable event without persisting or mutating anything.
    pub fn diagnose(&self, event: &crate::domain::CortexEvent) -> EvidenceDecodeResult {
        self.registry.decode(event)
    }

    pub fn registry(&self) -> &EventEvidenceDecoderRegistry {
        &self.registry
    }
}

struct ExternalToolCompletionDecoder;

impl EventEvidenceDecoder for ExternalToolCompletionDecoder {
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::ExternalToolCompletion
    }

    fn version(&self) -> u16 {
        1
    }

    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool {
        *event_type == crate::domain::EventType::ExternalToolFinished
    }

    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation> {
        let payload: ExternalToolCompletionPayload = parse_payload(&event.payload)?;
        validate_contract_fields(
            &payload.contract,
            payload.version,
            self.contract(),
            self.version(),
        )?;
        identifier(&payload.tool, "tool")?;
        identifier(&payload.operation, "operation")?;
        optional_identifier(payload.error_class.as_deref(), "error_class")?;
        optional_text(payload.message.as_deref(), "message")?;
        Ok(EvidenceObservation::ExternalToolCompletion(
            ToolCompletionEvidence {
                tool: payload.tool,
                operation: payload.operation,
                exit_code: payload.exit_code,
                error_class: payload.error_class,
                message: payload.message,
            },
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalToolCompletionPayload {
    contract: String,
    version: u16,
    tool: String,
    operation: String,
    exit_code: i64,
    error_class: Option<String>,
    message: Option<String>,
}

struct RustCompilerResultDecoder;

impl EventEvidenceDecoder for RustCompilerResultDecoder {
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::RustCompilerResult
    }

    fn version(&self) -> u16 {
        1
    }

    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool {
        *event_type == crate::domain::EventType::CompilerResult
    }

    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation> {
        let payload: RustCompilerResultPayload = parse_payload(&event.payload)?;
        validate_contract_fields(
            &payload.contract,
            payload.version,
            self.contract(),
            self.version(),
        )?;
        let subject = validate_compiler_subject(payload.subject)?;
        let process_outcome = process_outcome(payload.exit_code);
        if payload.diagnostics.len() > MAX_RUST_DIAGNOSTICS {
            return Err(issue(
                "too_many_diagnostics",
                Some("diagnostics"),
                format!("at most {MAX_RUST_DIAGNOSTICS} diagnostics are allowed"),
            ));
        }
        if process_outcome == ProcessOutcome::ExitedNonzero
            && !payload
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
        {
            return Err(issue(
                "missing_error_diagnostic",
                Some("diagnostics"),
                "a nonzero Rust compiler exit requires an error diagnostic",
            ));
        }
        if process_outcome == ProcessOutcome::ExitedZero
            && payload
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == DiagnosticLevel::Error)
        {
            return Err(issue(
                "conflicting_exit_and_diagnostics",
                Some("diagnostics"),
                "a zero Rust compiler exit cannot contain an error diagnostic",
            ));
        }
        let diagnostics = payload
            .diagnostics
            .into_iter()
            .map(validate_rust_diagnostic)
            .collect::<DecodeResult<Vec<_>>>()?;
        Ok(EvidenceObservation::RustCompilerResult(
            RustCompilerResultEvidence {
                subject,
                process_outcome,
                exit_code: payload.exit_code,
                diagnostics,
            },
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustCompilerResultPayload {
    contract: String,
    version: u16,
    subject: SubjectPayload,
    exit_code: i64,
    diagnostics: Vec<RustDiagnosticPayload>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RustDiagnosticPayload {
    level: DiagnosticLevel,
    code: Option<String>,
    message: String,
    expected_type: Option<String>,
    actual_type: Option<String>,
    path: Option<String>,
    start_line: Option<u64>,
    start_column: Option<u64>,
}

fn validate_rust_diagnostic(
    payload: RustDiagnosticPayload,
) -> DecodeResult<RustDiagnosticEvidence> {
    optional_identifier(payload.code.as_deref(), "diagnostics[].code")?;
    text(&payload.message, "diagnostics[].message")?;
    optional_text(
        payload.expected_type.as_deref(),
        "diagnostics[].expected_type",
    )?;
    optional_text(payload.actual_type.as_deref(), "diagnostics[].actual_type")?;
    if payload.expected_type.is_some() != payload.actual_type.is_some() {
        return Err(issue(
            "incomplete_type_distinction",
            Some("diagnostics[]"),
            "expected_type and actual_type must be provided together",
        ));
    }
    if let Some(path) = &payload.path {
        workspace_relative_path(path, "diagnostics[].path")?;
    }
    if payload.start_column.is_some() && payload.start_line.is_none() {
        return Err(issue(
            "incomplete_location",
            Some("diagnostics[].start_column"),
            "start_column requires start_line",
        ));
    }
    if payload.start_line == Some(0) || payload.start_column == Some(0) {
        return Err(issue(
            "invalid_location",
            Some("diagnostics[]"),
            "source lines and columns are one-based",
        ));
    }
    Ok(RustDiagnosticEvidence {
        level: payload.level,
        code: payload.code,
        message: payload.message,
        expected_type: payload.expected_type,
        actual_type: payload.actual_type,
        path: payload.path,
        start_line: payload.start_line,
        start_column: payload.start_column,
    })
}

struct CargoTestResultDecoder;

impl EventEvidenceDecoder for CargoTestResultDecoder {
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::CargoTestResult
    }

    fn version(&self) -> u16 {
        1
    }

    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool {
        *event_type == crate::domain::EventType::TestResult
    }

    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation> {
        let payload: CargoTestResultPayload = parse_payload(&event.payload)?;
        validate_contract_fields(
            &payload.contract,
            payload.version,
            self.contract(),
            self.version(),
        )?;
        let subject = validate_test_subject(payload.subject)?;
        let process_outcome = process_outcome(payload.exit_code);
        if payload.failures.len() > MAX_TEST_FAILURES {
            return Err(issue(
                "too_many_test_failures",
                Some("failures"),
                format!("at most {MAX_TEST_FAILURES} failures are allowed"),
            ));
        }
        if process_outcome == ProcessOutcome::ExitedZero && !payload.failures.is_empty() {
            return Err(issue(
                "conflicting_exit_and_failures",
                Some("failures"),
                "a zero Cargo test exit cannot contain failures",
            ));
        }
        if process_outcome == ProcessOutcome::ExitedNonzero && payload.failures.is_empty() {
            return Err(issue(
                "missing_test_failure",
                Some("failures"),
                "a nonzero Cargo test exit requires at least one failure observation",
            ));
        }
        if payload.executed_test_count > 1_000_000 {
            return Err(issue(
                "too_many_executed_tests",
                Some("executed_test_count"),
                "at most 1000000 executed tests are allowed",
            ));
        }
        let failures = payload
            .failures
            .into_iter()
            .map(validate_test_failure)
            .collect::<DecodeResult<Vec<_>>>()?;
        Ok(EvidenceObservation::CargoTestResult(
            CargoTestResultEvidence {
                subject,
                process_outcome,
                exit_code: payload.exit_code,
                executed_test_count: payload.executed_test_count,
                failures,
            },
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CargoTestResultPayload {
    contract: String,
    version: u16,
    subject: SubjectPayload,
    exit_code: i64,
    executed_test_count: u64,
    failures: Vec<TestFailurePayload>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TestFailurePayload {
    test_name: Option<String>,
    assertion_class: Option<String>,
    message: Option<String>,
}

fn validate_test_failure(payload: TestFailurePayload) -> DecodeResult<TestFailureEvidence> {
    optional_text(payload.test_name.as_deref(), "failures[].test_name")?;
    optional_identifier(
        payload.assertion_class.as_deref(),
        "failures[].assertion_class",
    )?;
    optional_text(payload.message.as_deref(), "failures[].message")?;
    if payload.test_name.is_none() && payload.message.is_none() {
        return Err(issue(
            "empty_test_failure",
            Some("failures[]"),
            "a test failure requires a test name or bounded message",
        ));
    }
    Ok(TestFailureEvidence {
        test_name: payload.test_name,
        assertion_class: payload.assertion_class,
        message: payload.message,
    })
}

struct GenericVerifierResultDecoder;

impl EventEvidenceDecoder for GenericVerifierResultDecoder {
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::GenericVerifierResult
    }

    fn version(&self) -> u16 {
        1
    }

    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool {
        *event_type == crate::domain::EventType::ExternalToolFinished
    }

    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation> {
        let payload: GenericVerifierResultPayload = parse_payload(&event.payload)?;
        validate_contract_fields(
            &payload.contract,
            payload.version,
            self.contract(),
            self.version(),
        )?;
        identifier(&payload.verifier_id, "verifier_id")?;
        identifier(&payload.verifier_version, "verifier_version")?;
        identifier(&payload.tool, "tool")?;
        identifier(&payload.operation, "operation")?;
        verifier_exit_consistency(payload.result, payload.exit_code)?;
        let subject = validate_subject(payload.subject)?;
        Ok(EvidenceObservation::GenericVerifierResult(
            GenericVerifierResultEvidence {
                verifier_id: payload.verifier_id,
                verifier_version: payload.verifier_version,
                tool: payload.tool,
                operation: payload.operation,
                reported_result: payload.result,
                process_outcome: process_outcome(payload.exit_code),
                exit_code: payload.exit_code,
                subject,
            },
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenericVerifierResultPayload {
    contract: String,
    version: u16,
    verifier_id: String,
    verifier_version: String,
    tool: String,
    operation: String,
    result: VerificationResult,
    exit_code: i64,
    subject: SubjectPayload,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubjectPayload {
    kind: EvidenceSubjectKind,
    value: String,
}

fn validate_subject(payload: SubjectPayload) -> DecodeResult<EvidenceSubject> {
    match payload.kind {
        EvidenceSubjectKind::Path => workspace_relative_path(&payload.value, "subject.value")?,
        EvidenceSubjectKind::Workspace
        | EvidenceSubjectKind::Package
        | EvidenceSubjectKind::Target
        | EvidenceSubjectKind::Test => text(&payload.value, "subject.value")?,
    }
    Ok(EvidenceSubject {
        kind: payload.kind,
        value: payload.value,
    })
}

fn validate_compiler_subject(payload: SubjectPayload) -> DecodeResult<EvidenceSubject> {
    if matches!(
        payload.kind,
        EvidenceSubjectKind::Test | EvidenceSubjectKind::Path
    ) {
        return Err(issue(
            "invalid_compiler_subject",
            Some("subject.kind"),
            "a Rust compiler subject must be a workspace, package, or target",
        ));
    }
    validate_subject(payload)
}

fn validate_test_subject(payload: SubjectPayload) -> DecodeResult<EvidenceSubject> {
    if payload.kind == EvidenceSubjectKind::Path {
        return Err(issue(
            "invalid_test_subject",
            Some("subject.kind"),
            "a Cargo test subject must be a workspace, package, target, or test",
        ));
    }
    validate_subject(payload)
}

struct SourceChangeObservationDecoder;

impl EventEvidenceDecoder for SourceChangeObservationDecoder {
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::SourceChangeObservation
    }

    fn version(&self) -> u16 {
        1
    }

    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool {
        matches!(
            event_type,
            crate::domain::EventType::FileCreated
                | crate::domain::EventType::FileModified
                | crate::domain::EventType::FileRemoved
                | crate::domain::EventType::FileRenamed
        )
    }

    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation> {
        required_field(&event.payload, "content_hash")?;
        let payload: SourceChangeObservationPayload = parse_payload(&event.payload)?;
        validate_contract_fields(
            &payload.contract,
            payload.version,
            self.contract(),
            self.version(),
        )?;
        workspace_relative_path(&payload.path, "path")?;
        if payload.workspace_content_revision < 0 || payload.document_content_revision < 0 {
            return Err(issue(
                "negative_content_revision",
                Some("workspace_content_revision"),
                "content revisions must be non-negative",
            ));
        }
        validate_source_change_event_type(payload.change, &event.event_type)?;
        match payload.change {
            SourceChangeKind::Renamed => {
                let previous_path = payload.previous_path.as_deref().ok_or_else(|| {
                    issue(
                        "missing_previous_path",
                        Some("previous_path"),
                        "a rename requires a previous workspace-relative path",
                    )
                })?;
                workspace_relative_path(previous_path, "previous_path")?;
                if previous_path == payload.path {
                    return Err(issue(
                        "unchanged_rename_path",
                        Some("previous_path"),
                        "a rename requires a different previous path",
                    ));
                }
            }
            SourceChangeKind::Created | SourceChangeKind::Modified | SourceChangeKind::Removed => {
                if payload.previous_path.is_some() {
                    return Err(issue(
                        "unexpected_previous_path",
                        Some("previous_path"),
                        "only a rename may include previous_path",
                    ));
                }
            }
        }
        match (payload.change, payload.content_hash.as_deref()) {
            (SourceChangeKind::Removed, None) => {}
            (SourceChangeKind::Removed, Some(_)) => {
                return Err(issue(
                    "removed_content_hash_present",
                    Some("content_hash"),
                    "a removal requires an explicit null content_hash",
                ));
            }
            (_, Some(hash)) => blake3_hash(hash, "content_hash")?,
            (_, None) => {
                return Err(issue(
                    "missing_content_hash",
                    Some("content_hash"),
                    "a created, modified, or renamed source change requires a content hash",
                ));
            }
        }
        Ok(EvidenceObservation::SourceChange(SourceChangeEvidence {
            change: payload.change,
            path: payload.path,
            previous_path: payload.previous_path,
            reported_workspace_content_revision: payload.workspace_content_revision,
            reported_document_content_revision: payload.document_content_revision,
            reported_content_hash: payload.content_hash,
        }))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceChangeObservationPayload {
    contract: String,
    version: u16,
    change: SourceChangeKind,
    path: String,
    previous_path: Option<String>,
    workspace_content_revision: i64,
    document_content_revision: i64,
    content_hash: Option<String>,
}

fn validate_source_change_event_type(
    change: SourceChangeKind,
    event_type: &crate::domain::EventType,
) -> DecodeResult<()> {
    let expected = match change {
        SourceChangeKind::Created => crate::domain::EventType::FileCreated,
        SourceChangeKind::Modified => crate::domain::EventType::FileModified,
        SourceChangeKind::Removed => crate::domain::EventType::FileRemoved,
        SourceChangeKind::Renamed => crate::domain::EventType::FileRenamed,
    };
    if *event_type == expected {
        Ok(())
    } else {
        Err(issue(
            "conflicting_change_and_event_type",
            Some("change"),
            format!(
                "source change {} requires event type {}",
                serde_json::to_value(change)
                    .ok()
                    .and_then(|value| value.as_str().map(ToOwned::to_owned))
                    .unwrap_or_else(|| "unknown".into()),
                expected.storage_name()
            ),
        ))
    }
}

struct UserAcceptanceDecoder;

impl EventEvidenceDecoder for UserAcceptanceDecoder {
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::UserAcceptance
    }

    fn version(&self) -> u16 {
        1
    }

    fn supports_event_type(&self, event_type: &crate::domain::EventType) -> bool {
        *event_type == crate::domain::EventType::UserAcceptance
    }

    fn decode_observation(
        &self,
        event: &crate::domain::CortexEvent,
    ) -> DecodeResult<EvidenceObservation> {
        let payload: UserAcceptancePayload = parse_payload(&event.payload)?;
        validate_contract_fields(
            &payload.contract,
            payload.version,
            self.contract(),
            self.version(),
        )?;
        if !payload.accepted {
            return Err(issue(
                "acceptance_not_granted",
                Some("accepted"),
                "a user acceptance event requires accepted to be true",
            ));
        }
        identifier(&payload.accepted_by, "accepted_by")?;
        let subject = validate_subject(payload.subject)?;
        Ok(EvidenceObservation::UserAcceptance(
            UserAcceptanceEvidence {
                reported_actor: payload.accepted_by,
                subject,
            },
        ))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserAcceptancePayload {
    contract: String,
    version: u16,
    accepted: bool,
    accepted_by: String,
    subject: SubjectPayload,
}

fn header(payload: &Value) -> std::result::Result<(String, u16), EvidenceDecodeResult> {
    let Some(object) = payload.as_object() else {
        return Err(invalid(
            "payload_not_object",
            None::<String>,
            "a versioned evidence payload must be a JSON object",
        ));
    };
    match (object.get("contract"), object.get("version")) {
        (None, None) => Err(EvidenceDecodeResult::Unsupported {
            reason: UnsupportedEvidenceReason::LegacyUnversionedPayload,
        }),
        (Some(contract), Some(version)) => {
            let Some(contract) = contract.as_str() else {
                return Err(invalid(
                    "invalid_contract",
                    Some("contract"),
                    "contract must be a string",
                ));
            };
            if contract.trim().is_empty() || contract.len() > MAX_EVIDENCE_IDENTIFIER_BYTES {
                return Err(invalid(
                    "invalid_contract",
                    Some("contract"),
                    format!(
                        "contract must be non-empty and at most {MAX_EVIDENCE_IDENTIFIER_BYTES} bytes"
                    ),
                ));
            }
            let Some(version) = version.as_u64() else {
                return Err(invalid(
                    "invalid_version",
                    Some("version"),
                    "version must be an unsigned integer",
                ));
            };
            let Ok(version) = u16::try_from(version) else {
                return Err(invalid(
                    "invalid_version",
                    Some("version"),
                    "version exceeds the supported unsigned-16-bit range",
                ));
            };
            Ok((contract.to_owned(), version))
        }
        (None, Some(_)) => Err(invalid(
            "missing_contract",
            Some("contract"),
            "versioned evidence payloads require contract",
        )),
        (Some(_), None) => Err(invalid(
            "missing_version",
            Some("version"),
            "versioned evidence payloads require version",
        )),
    }
}

fn validate_event_provenance(event: &crate::domain::CortexEvent) -> DecodeResult<()> {
    identifier(&event.workspace_id, "workspace_id")?;
    let Some(session_id) = event.session_id.as_deref() else {
        return Err(issue(
            "missing_session_provenance",
            Some("session_id"),
            "typed evidence requires explicit session provenance",
        ));
    };
    identifier(session_id, "session_id")?;
    optional_identifier(event.task_id.as_deref(), "task_id")
}

fn validate_contract_fields(
    contract: &str,
    version: u16,
    expected_contract: EvidenceContract,
    expected_version: u16,
) -> DecodeResult<()> {
    if contract != expected_contract.id() {
        return Err(issue(
            "conflicting_contract",
            Some("contract"),
            format!("payload must declare {}", expected_contract.id()),
        ));
    }
    if version != expected_version {
        return Err(issue(
            "conflicting_version",
            Some("version"),
            format!("payload must declare version {expected_version}"),
        ));
    }
    Ok(())
}

fn parse_payload<T: for<'de> Deserialize<'de>>(payload: &Value) -> DecodeResult<T> {
    serde_json::from_value(payload.clone()).map_err(|error| {
        issue(
            "invalid_payload_shape",
            None::<String>,
            format!("payload does not match the registered contract schema: {error}"),
        )
    })
}

fn required_field(payload: &Value, field: &str) -> DecodeResult<()> {
    if payload
        .as_object()
        .is_some_and(|object| object.contains_key(field))
    {
        Ok(())
    } else {
        Err(issue(
            "missing_required_field",
            Some(field),
            format!("payload requires {field}"),
        ))
    }
}

fn process_outcome(exit_code: i64) -> ProcessOutcome {
    if exit_code == 0 {
        ProcessOutcome::ExitedZero
    } else {
        ProcessOutcome::ExitedNonzero
    }
}

fn verifier_exit_consistency(result: VerificationResult, exit_code: i64) -> DecodeResult<()> {
    let valid = match result {
        VerificationResult::Passed => exit_code == 0,
        VerificationResult::Failed => exit_code != 0,
    };
    if valid {
        Ok(())
    } else {
        Err(issue(
            "conflicting_result_and_exit_code",
            Some("result"),
            "result conflicts with exit_code",
        ))
    }
}

fn identifier(value: &str, field: &str) -> DecodeResult<()> {
    if value.trim().is_empty()
        || value.len() > MAX_EVIDENCE_IDENTIFIER_BYTES
        || value.contains('\0')
    {
        return Err(issue(
            "invalid_identifier",
            Some(field),
            format!(
                "{field} must be non-empty, contain no NUL, and be at most {MAX_EVIDENCE_IDENTIFIER_BYTES} bytes"
            ),
        ));
    }
    Ok(())
}

fn optional_identifier(value: Option<&str>, field: &str) -> DecodeResult<()> {
    if let Some(value) = value {
        identifier(value, field)?;
    }
    Ok(())
}

fn text(value: &str, field: &str) -> DecodeResult<()> {
    if value.trim().is_empty() || value.len() > MAX_EVIDENCE_TEXT_BYTES || value.contains('\0') {
        return Err(issue(
            "invalid_text",
            Some(field),
            format!(
                "{field} must be non-empty, contain no NUL, and be at most {MAX_EVIDENCE_TEXT_BYTES} bytes"
            ),
        ));
    }
    Ok(())
}

fn optional_text(value: Option<&str>, field: &str) -> DecodeResult<()> {
    if let Some(value) = value {
        text(value, field)?;
    }
    Ok(())
}

fn workspace_relative_path(value: &str, field: &str) -> DecodeResult<()> {
    if value.len() > MAX_EVIDENCE_TEXT_BYTES
        || value.is_empty()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err(issue(
            "invalid_workspace_relative_path",
            Some(field),
            "path must be a normalized slash-separated workspace-relative path",
        ));
    }
    Ok(())
}

fn blake3_hash(value: &str, field: &str) -> DecodeResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(issue(
            "invalid_blake3_hash",
            Some(field),
            "content_hash must be a lowercase 64-character BLAKE3 hexadecimal digest",
        ));
    }
    Ok(())
}

fn issue(
    code: impl Into<String>,
    field: Option<impl Into<String>>,
    message: impl Into<String>,
) -> EvidenceDecodeIssue {
    EvidenceDecodeIssue::new(code, field, message)
}

fn invalid(
    code: impl Into<String>,
    field: Option<impl Into<String>>,
    message: impl Into<String>,
) -> EvidenceDecodeResult {
    EvidenceDecodeResult::Invalid {
        issue: issue(code, field, message),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use serde_json::{Value, json};

    use crate::domain::{CortexEvent, EventType};

    use super::*;

    fn event(event_type: EventType, payload: Value) -> CortexEvent {
        let mut event = CortexEvent::new("workspace", event_type, payload);
        event.id = "event-1".into();
        event.session_id = Some("session-1".into());
        event.task_id = Some("task-1".into());
        event.created_at = Utc::now();
        event
    }

    fn contract(contract: EvidenceContract, body: Value) -> Value {
        let mut body = body.as_object().cloned().unwrap();
        body.insert("contract".into(), json!(contract.id()));
        body.insert("version".into(), json!(1));
        Value::Object(body)
    }

    fn external_tool_completion_v1() -> Value {
        contract(
            EvidenceContract::ExternalToolCompletion,
            json!({
                "tool": "cargo",
                "operation": "check",
                "exit_code": 1,
                "error_class": "exit_nonzero",
                "message": "check failed"
            }),
        )
    }

    fn rust_compiler_result_v1() -> Value {
        contract(
            EvidenceContract::RustCompilerResult,
            json!({
                "subject": { "kind": "target", "value": "cortexweave" },
                "exit_code": 1,
                "diagnostics": [{
                    "level": "error",
                    "code": "E0308",
                    "message": "mismatched types",
                    "expected_type": "String",
                    "actual_type": "u32",
                    "path": "src/lib.rs",
                    "start_line": 12,
                    "start_column": 5
                }]
            }),
        )
    }

    fn cargo_test_result_v1() -> Value {
        contract(
            EvidenceContract::CargoTestResult,
            json!({
                "subject": { "kind": "test", "value": "service::works" },
                "exit_code": 101,
                "executed_test_count": 1,
                "failures": [{
                    "test_name": "service::works",
                    "assertion_class": "assert_eq",
                    "message": "left != right"
                }]
            }),
        )
    }

    fn generic_verifier_result_v1() -> Value {
        contract(
            EvidenceContract::GenericVerifierResult,
            json!({
                "verifier_id": "cargo.check",
                "verifier_version": "1",
                "tool": "cargo",
                "operation": "check",
                "result": "passed",
                "exit_code": 0,
                "subject": { "kind": "package", "value": "cortexweave" }
            }),
        )
    }

    fn source_change_observation_v1() -> Value {
        contract(
            EvidenceContract::SourceChangeObservation,
            json!({
                "change": "modified",
                "path": "src/lib.rs",
                "previous_path": null,
                "workspace_content_revision": 12,
                "document_content_revision": 3,
                "content_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }),
        )
    }

    fn user_acceptance_v1() -> Value {
        contract(
            EvidenceContract::UserAcceptance,
            json!({
                "accepted": true,
                "accepted_by": "captain",
                "subject": { "kind": "test", "value": "service::works" }
            }),
        )
    }

    #[test]
    fn v1_fixture_builders_decode_exactly() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();
        let cases = [
            (
                EventType::ExternalToolFinished,
                external_tool_completion_v1(),
                EvidenceContract::ExternalToolCompletion,
            ),
            (
                EventType::CompilerResult,
                rust_compiler_result_v1(),
                EvidenceContract::RustCompilerResult,
            ),
            (
                EventType::TestResult,
                cargo_test_result_v1(),
                EvidenceContract::CargoTestResult,
            ),
            (
                EventType::ExternalToolFinished,
                generic_verifier_result_v1(),
                EvidenceContract::GenericVerifierResult,
            ),
            (
                EventType::FileModified,
                source_change_observation_v1(),
                EvidenceContract::SourceChangeObservation,
            ),
            (
                EventType::UserAcceptance,
                user_acceptance_v1(),
                EvidenceContract::UserAcceptance,
            ),
        ];

        for (event_type, payload, expected_contract) in cases {
            let result = registry.decode(&event(event_type, payload));
            let decoded = result.decoded().expect("fixture must decode");
            assert_eq!(decoded.envelope.event_id, "event-1");
            assert_eq!(decoded.envelope.workspace_id, "workspace");
            assert_eq!(decoded.envelope.session_id, "session-1");
            assert_eq!(decoded.envelope.task_id.as_deref(), Some("task-1"));
            assert_eq!(decoded.envelope.contract, expected_contract);
            assert_eq!(decoded.envelope.version, 1);
        }

        assert_observation(
            &registry,
            EventType::ExternalToolFinished,
            external_tool_completion_v1(),
            EvidenceObservation::ExternalToolCompletion(ToolCompletionEvidence {
                tool: "cargo".into(),
                operation: "check".into(),
                exit_code: 1,
                error_class: Some("exit_nonzero".into()),
                message: Some("check failed".into()),
            }),
        );
        assert_observation(
            &registry,
            EventType::CompilerResult,
            rust_compiler_result_v1(),
            EvidenceObservation::RustCompilerResult(RustCompilerResultEvidence {
                subject: EvidenceSubject {
                    kind: EvidenceSubjectKind::Target,
                    value: "cortexweave".into(),
                },
                process_outcome: ProcessOutcome::ExitedNonzero,
                exit_code: 1,
                diagnostics: vec![RustDiagnosticEvidence {
                    level: DiagnosticLevel::Error,
                    code: Some("E0308".into()),
                    message: "mismatched types".into(),
                    expected_type: Some("String".into()),
                    actual_type: Some("u32".into()),
                    path: Some("src/lib.rs".into()),
                    start_line: Some(12),
                    start_column: Some(5),
                }],
            }),
        );
        assert_observation(
            &registry,
            EventType::TestResult,
            cargo_test_result_v1(),
            EvidenceObservation::CargoTestResult(CargoTestResultEvidence {
                subject: EvidenceSubject {
                    kind: EvidenceSubjectKind::Test,
                    value: "service::works".into(),
                },
                process_outcome: ProcessOutcome::ExitedNonzero,
                exit_code: 101,
                executed_test_count: 1,
                failures: vec![TestFailureEvidence {
                    test_name: Some("service::works".into()),
                    assertion_class: Some("assert_eq".into()),
                    message: Some("left != right".into()),
                }],
            }),
        );
        assert_observation(
            &registry,
            EventType::ExternalToolFinished,
            generic_verifier_result_v1(),
            EvidenceObservation::GenericVerifierResult(GenericVerifierResultEvidence {
                verifier_id: "cargo.check".into(),
                verifier_version: "1".into(),
                tool: "cargo".into(),
                operation: "check".into(),
                reported_result: VerificationResult::Passed,
                process_outcome: ProcessOutcome::ExitedZero,
                exit_code: 0,
                subject: EvidenceSubject {
                    kind: EvidenceSubjectKind::Package,
                    value: "cortexweave".into(),
                },
            }),
        );
        assert_observation(
            &registry,
            EventType::FileModified,
            source_change_observation_v1(),
            EvidenceObservation::SourceChange(SourceChangeEvidence {
                change: SourceChangeKind::Modified,
                path: "src/lib.rs".into(),
                previous_path: None,
                reported_workspace_content_revision: 12,
                reported_document_content_revision: 3,
                reported_content_hash: Some(
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                ),
            }),
        );
        assert_observation(
            &registry,
            EventType::UserAcceptance,
            user_acceptance_v1(),
            EvidenceObservation::UserAcceptance(UserAcceptanceEvidence {
                reported_actor: "captain".into(),
                subject: EvidenceSubject {
                    kind: EvidenceSubjectKind::Test,
                    value: "service::works".into(),
                },
            }),
        );
    }

    #[test]
    fn invalid_missing_wrong_type_oversized_and_conflicting_fields_are_visible() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();

        let mut missing = rust_compiler_result_v1();
        missing.as_object_mut().unwrap().remove("subject");
        assert_invalid(&registry.decode(&event(EventType::CompilerResult, missing)));

        let mut wrong_type = cargo_test_result_v1();
        wrong_type
            .as_object_mut()
            .unwrap()
            .insert("exit_code".into(), json!("1"));
        assert_invalid(&registry.decode(&event(EventType::TestResult, wrong_type)));

        let mut oversized = external_tool_completion_v1();
        oversized.as_object_mut().unwrap().insert(
            "message".into(),
            json!("x".repeat(MAX_EVIDENCE_PAYLOAD_BYTES)),
        );
        assert!(matches!(
            registry.decode(&event(EventType::ExternalToolFinished, oversized)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "payload_too_large"
        ));

        let mut conflict = generic_verifier_result_v1();
        conflict
            .as_object_mut()
            .unwrap()
            .insert("exit_code".into(), json!(1));
        assert!(matches!(
            registry.decode(&event(EventType::ExternalToolFinished, conflict)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "conflicting_result_and_exit_code"
        ));

        let mut incomplete_types = rust_compiler_result_v1();
        incomplete_types["diagnostics"][0]
            .as_object_mut()
            .unwrap()
            .remove("actual_type");
        assert!(matches!(
            registry.decode(&event(EventType::CompilerResult, incomplete_types)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "incomplete_type_distinction"
        ));
    }

    #[test]
    fn legacy_unknown_and_missing_provenance_remain_explicitly_classified() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();
        assert!(matches!(
            registry.decode(&event(EventType::CompilerResult, json!({ "ok": false }))),
            EvidenceDecodeResult::Unsupported {
                reason: UnsupportedEvidenceReason::LegacyUnversionedPayload
            }
        ));
        assert!(matches!(
            registry.decode(&event(
                EventType::CompilerResult,
                json!({ "contract": "example.unknown", "version": 1 })
            )),
            EvidenceDecodeResult::Unsupported {
                reason: UnsupportedEvidenceReason::UnknownContract { .. }
            }
        ));

        let mut typed = event(EventType::CompilerResult, rust_compiler_result_v1());
        typed.session_id = None;
        assert!(matches!(
            registry.decode(&typed),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "missing_session_provenance"
        ));
    }

    #[test]
    fn source_change_rejects_absolute_paths_and_event_type_conflicts() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();
        let mut absolute = source_change_observation_v1();
        absolute
            .as_object_mut()
            .unwrap()
            .insert("path".into(), json!("C:/secrets/token.rs"));
        assert!(matches!(
            registry.decode(&event(EventType::FileModified, absolute)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "invalid_workspace_relative_path"
        ));

        assert!(matches!(
            registry.decode(&event(EventType::FileCreated, source_change_observation_v1())),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "conflicting_change_and_event_type"
        ));
    }

    #[test]
    fn verification_subjects_are_typed_and_symbol_claims_are_rejected() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();

        let mut compiler_subject = rust_compiler_result_v1();
        compiler_subject.as_object_mut().unwrap().insert(
            "subject".into(),
            json!({ "kind": "test", "value": "works" }),
        );
        assert!(matches!(
            registry.decode(&event(EventType::CompilerResult, compiler_subject)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "invalid_compiler_subject"
        ));

        let mut test_subject = cargo_test_result_v1();
        test_subject.as_object_mut().unwrap().insert(
            "subject".into(),
            json!({ "kind": "path", "value": "src/lib.rs" }),
        );
        assert!(matches!(
            registry.decode(&event(EventType::TestResult, test_subject)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "invalid_test_subject"
        ));

        let mut claimed_symbol = rust_compiler_result_v1();
        claimed_symbol["diagnostics"][0]
            .as_object_mut()
            .unwrap()
            .insert("symbol_key".into(), json!("crate::run"));
        assert!(matches!(
            registry.decode(&event(EventType::CompilerResult, claimed_symbol)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "invalid_payload_shape"
        ));
    }

    #[test]
    fn process_outcomes_are_derived_and_cannot_conflict_with_details() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();

        let mut compiler = rust_compiler_result_v1();
        compiler
            .as_object_mut()
            .unwrap()
            .insert("exit_code".into(), json!(0));
        assert!(matches!(
            registry.decode(&event(EventType::CompilerResult, compiler)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "conflicting_exit_and_diagnostics"
        ));

        let mut tests = cargo_test_result_v1();
        tests
            .as_object_mut()
            .unwrap()
            .insert("exit_code".into(), json!(0));
        assert!(matches!(
            registry.decode(&event(EventType::TestResult, tests)),
            EvidenceDecodeResult::Invalid { issue } if issue.code == "conflicting_exit_and_failures"
        ));
    }

    #[test]
    fn registry_rejects_ambiguous_contract_ownership() {
        let result = EventEvidenceDecoderRegistry::new(vec![
            Arc::new(ExternalToolCompletionDecoder),
            Arc::new(ExternalToolCompletionDecoder),
        ]);
        assert!(matches!(
            result,
            Err(CortexError::Configuration(message)) if message.contains("ambiguous evidence decoder")
        ));
    }

    #[test]
    fn decoder_does_not_mutate_the_raw_event() {
        let registry = EventEvidenceDecoderRegistry::standard().unwrap();
        let event = event(EventType::CompilerResult, rust_compiler_result_v1());
        let before = event.clone();
        let _ = registry.decode(&event);
        assert_eq!(event, before);
    }

    fn assert_invalid(result: &EvidenceDecodeResult) {
        assert!(matches!(result, EvidenceDecodeResult::Invalid { .. }));
    }

    fn assert_observation(
        registry: &EventEvidenceDecoderRegistry,
        event_type: EventType,
        payload: Value,
        expected: EvidenceObservation,
    ) {
        let result = registry.decode(&event(event_type, payload));
        assert_eq!(
            result.decoded().expect("fixture must decode").observation,
            expected
        );
    }
}
