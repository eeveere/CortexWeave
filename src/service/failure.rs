use std::{collections::BTreeMap, sync::Arc};

use crate::{
    CortexError, Result,
    domain::{
        DecodedEvidence, DiagnosticLevel, EvidenceContract, EvidenceObservation, EvidenceSubject,
        FailureDomain, FailureIdentityCapability, FailureNormalization,
        FailureNormalizationDiagnostic, FailureNormalizationResult, FailureScope, FailureSignature,
        ProcessOutcome, UnsupportedFailureNormalization, VerificationResult, VerificationSubject,
        VerificationSubjectKind, VerifierRule,
    },
};

pub trait FailureNormalizer: Send + Sync {
    fn id(&self) -> &'static str;
    fn version(&self) -> &'static str;
    fn contract(&self) -> EvidenceContract;
    fn normalize(&self, evidence: &DecodedEvidence) -> FailureNormalizationResult;
}

/// Deterministic contract lookup. A decoder's output never receives a
/// best-effort failure meaning from a normalizer for another contract.
pub struct FailureNormalizerRegistry {
    normalizers: BTreeMap<String, Arc<dyn FailureNormalizer>>,
}

impl FailureNormalizerRegistry {
    pub fn new(normalizers: Vec<Arc<dyn FailureNormalizer>>) -> Result<Self> {
        let mut registered = BTreeMap::new();
        for normalizer in normalizers {
            let contract = normalizer.contract();
            if registered
                .insert(contract.id().to_owned(), normalizer)
                .is_some()
            {
                return Err(CortexError::Configuration(format!(
                    "ambiguous failure normalizer registration for {}",
                    contract.id()
                )));
            }
        }
        Ok(Self {
            normalizers: registered,
        })
    }

    pub fn standard(rules: Arc<VerifierRuleRegistry>) -> Result<Self> {
        Self::new(vec![
            Arc::new(RustCompilerFailureNormalizer),
            Arc::new(CargoTestFailureNormalizer),
            Arc::new(RegisteredToolFailureNormalizer { rules }),
        ])
    }

    pub fn normalize(&self, evidence: &DecodedEvidence) -> FailureNormalizationResult {
        match self.normalizers.get(evidence.envelope.contract.id()) {
            Some(normalizer) => normalizer.normalize(evidence),
            None => unsupported(
                "unsupported_evidence_contract",
                "no failure normalizer is registered for this evidence contract",
            ),
        }
    }

    pub fn identities(&self) -> Vec<(String, String, String)> {
        self.normalizers
            .values()
            .map(|normalizer| {
                (
                    normalizer.contract().id().to_owned(),
                    normalizer.id().to_owned(),
                    normalizer.version().to_owned(),
                )
            })
            .collect()
    }
}

/// Registered rules make generic verifier reports eligible only by exact,
/// versioned identity; arbitrary commands and unregistered reports never form
/// signatures.
pub struct VerifierRuleRegistry {
    rules: BTreeMap<(String, String), VerifierRule>,
}

impl VerifierRuleRegistry {
    pub fn new(rules: Vec<VerifierRule>) -> Result<Self> {
        let mut registered = BTreeMap::new();
        for rule in rules {
            rule.validate()?;
            let key = (
                rule.declared_verifier_id.clone(),
                rule.declared_verifier_version.clone(),
            );
            if registered.insert(key.clone(), rule).is_some() {
                return Err(CortexError::Configuration(format!(
                    "ambiguous verifier rule registration for {} v{}",
                    key.0, key.1
                )));
            }
        }
        Ok(Self { rules: registered })
    }

    pub fn standard() -> Result<Self> {
        Self::new(vec![VerifierRule::cargo_check_v1()])
    }

    pub fn find(&self, id: &str, version: &str) -> Option<&VerifierRule> {
        self.rules.get(&(id.to_owned(), version.to_owned()))
    }

    pub fn identities(&self) -> Vec<(String, String)> {
        self.rules.keys().cloned().collect()
    }

    pub fn catalog(&self) -> Vec<VerifierRule> {
        self.rules.values().cloned().collect()
    }
}

pub struct FailureNormalizationService {
    registry: FailureNormalizerRegistry,
    rules: Arc<VerifierRuleRegistry>,
}

impl FailureNormalizationService {
    pub fn new(registry: FailureNormalizerRegistry, rules: Arc<VerifierRuleRegistry>) -> Self {
        Self { registry, rules }
    }

    pub fn standard() -> Result<Self> {
        let rules = Arc::new(VerifierRuleRegistry::standard()?);
        let registry = FailureNormalizerRegistry::standard(Arc::clone(&rules))?;
        Ok(Self::new(registry, rules))
    }

    pub fn normalize(&self, evidence: &DecodedEvidence) -> FailureNormalizationResult {
        self.registry.normalize(evidence)
    }

    pub fn verifier_rules(&self) -> &VerifierRuleRegistry {
        &self.rules
    }

    pub fn normalizer_identities(&self) -> Vec<(String, String, String)> {
        self.registry.identities()
    }
}

struct RustCompilerFailureNormalizer;

impl FailureNormalizer for RustCompilerFailureNormalizer {
    fn id(&self) -> &'static str {
        "cortexweave.rust_compiler_failure"
    }
    fn version(&self) -> &'static str {
        "2"
    }
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::RustCompilerResult
    }

    fn normalize(&self, evidence: &DecodedEvidence) -> FailureNormalizationResult {
        let EvidenceObservation::RustCompilerResult(report) = &evidence.observation else {
            return unsupported(
                "evidence_contract_mismatch",
                "Rust compiler normalizer received another observation",
            );
        };
        if report.process_outcome != ProcessOutcome::ExitedNonzero {
            return unsupported(
                "not_a_failure",
                "a zero-exit compiler report has no failure signature",
            );
        }
        let errors: Vec<_> = report
            .diagnostics
            .iter()
            .filter(|item| item.level == DiagnosticLevel::Error)
            .collect();
        if errors.len() != 1 {
            return unsupported(
                "ambiguous_compiler_diagnostics",
                "exactly one compiler error is required for a v2 normalized class",
            );
        }
        let diagnostic = errors[0];
        let Some(code) = diagnostic
            .code
            .as_deref()
            .filter(|value| is_rust_error_code(value))
        else {
            return unsupported(
                "missing_structured_diagnostic_code",
                "a Rust error code is required; message text is not guessed",
            );
        };
        let subject = match verification_subject(&report.subject) {
            Ok(value) => value,
            Err(reason) => return reason,
        };
        let path = match diagnostic.path.as_deref() {
            Some(path) if workspace_relative_path(path) => Some(path.to_owned()),
            Some(_) => {
                return unsupported(
                    "unsafe_diagnostic_path",
                    "diagnostic paths must be workspace-relative",
                );
            }
            None => {
                return unsupported(
                    "missing_failure_scope_path",
                    "Rust compiler signatures require a workspace-relative path while no deterministic symbol mapping exists",
                );
            }
        };
        let mut components = BTreeMap::from([
            ("diagnostic_code".into(), code.to_owned()),
            (
                "subject_kind".into(),
                subject_kind_name(subject.kind).into(),
            ),
            ("subject_value".into(), subject.value.clone()),
        ]);
        let mut diagnostics = vec![diagnostic_note(
            "missing_deterministic_symbol",
            "no deterministic diagnostic-to-symbol mapping was available; no symbol was inferred",
        )];
        let identity_capability = if code == "E0308" {
            let (Some(expected), Some(actual)) = (
                diagnostic.expected_type.as_deref(),
                diagnostic.actual_type.as_deref(),
            ) else {
                return unsupported(
                    "missing_e0308_type_distinction",
                    "E0308 requires structured expected_type and actual_type fields; message text is not parsed",
                );
            };
            if !stable_component(expected) || !stable_component(actual) {
                return unsupported(
                    "unsafe_e0308_type_distinction",
                    "E0308 type fields must use bounded stable type-class text",
                );
            }
            components.insert("expected_type".into(), expected.to_owned());
            components.insert("actual_type".into(), actual.to_owned());
            FailureIdentityCapability::Exact
        } else {
            diagnostics.push(diagnostic_note(
                "compatible_only_identity",
                "this diagnostic code has no registered structured discriminator, so its normalized class cannot support exact equality",
            ));
            FailureIdentityCapability::CompatibleOnly
        };
        signature(
            FailureDomain::RustCompiler,
            identity_capability,
            components,
            self.id(),
            self.version(),
            scope(
                &evidence.envelope.workspace_id,
                "rustc",
                Some(subject.value),
                path,
            ),
            diagnostics,
        )
    }
}

struct CargoTestFailureNormalizer;

impl FailureNormalizer for CargoTestFailureNormalizer {
    fn id(&self) -> &'static str {
        "cortexweave.cargo_test_failure"
    }
    fn version(&self) -> &'static str {
        "2"
    }
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::CargoTestResult
    }

    fn normalize(&self, evidence: &DecodedEvidence) -> FailureNormalizationResult {
        let EvidenceObservation::CargoTestResult(report) = &evidence.observation else {
            return unsupported(
                "evidence_contract_mismatch",
                "Cargo test normalizer received another observation",
            );
        };
        if report.process_outcome != ProcessOutcome::ExitedNonzero {
            return unsupported(
                "not_a_failure",
                "a zero-exit test report has no failure signature",
            );
        }
        if report.failures.len() != 1 {
            return unsupported(
                "ambiguous_test_failures",
                "exactly one test failure is required for a v2 normalized class",
            );
        }
        let failure = &report.failures[0];
        let subject = match verification_subject(&report.subject) {
            Ok(value) => value,
            Err(reason) => return reason,
        };
        let mut components = BTreeMap::from([
            (
                "subject_kind".into(),
                subject_kind_name(subject.kind).into(),
            ),
            ("subject_value".into(), subject.value.clone()),
        ]);
        if let Some(name) = failure
            .test_name
            .as_deref()
            .filter(|value| stable_component(value))
        {
            components.insert("test_name".into(), name.to_owned());
        }
        if let Some(class) = failure
            .assertion_class
            .as_deref()
            .filter(|value| stable_component(value))
        {
            components.insert("assertion_class".into(), class.to_owned());
        }
        if components.len() == 2 {
            return unsupported(
                "missing_test_identity",
                "a stable test name or assertion class is required; failure prose is excluded",
            );
        }
        signature(
            FailureDomain::CargoTest,
            FailureIdentityCapability::CompatibleOnly,
            components,
            self.id(),
            self.version(),
            scope(
                &evidence.envelope.workspace_id,
                "cargo",
                Some(subject.value),
                None,
            ),
            vec![
                diagnostic_note(
                    "raw_failure_message_excluded",
                    "failure message text is intentionally excluded from the canonical signature",
                ),
                diagnostic_note(
                    "compatible_only_identity",
                    "a test name and assertion class do not identify one stable failing assertion or case, so this normalized class cannot support exact equality",
                ),
            ],
        )
    }
}

struct RegisteredToolFailureNormalizer {
    rules: Arc<VerifierRuleRegistry>,
}

impl FailureNormalizer for RegisteredToolFailureNormalizer {
    fn id(&self) -> &'static str {
        "cortexweave.registered_tool_failure"
    }
    fn version(&self) -> &'static str {
        "2"
    }
    fn contract(&self) -> EvidenceContract {
        EvidenceContract::GenericVerifierResult
    }

    fn normalize(&self, evidence: &DecodedEvidence) -> FailureNormalizationResult {
        let EvidenceObservation::GenericVerifierResult(report) = &evidence.observation else {
            return unsupported(
                "evidence_contract_mismatch",
                "registered-tool normalizer received another observation",
            );
        };
        if report.process_outcome != ProcessOutcome::ExitedNonzero
            || report.reported_result != VerificationResult::Failed
        {
            return unsupported(
                "not_a_failure",
                "generic verification requires an explicit failed result and nonzero exit",
            );
        }
        let Some(rule) = self
            .rules
            .find(&report.verifier_id, &report.verifier_version)
        else {
            return unsupported(
                "unregistered_verifier_rule",
                "generic verifier reports require an exact registered rule",
            );
        };
        if report.tool != rule.required_tool || report.operation != rule.required_operation {
            return unsupported(
                "verifier_rule_mismatch",
                "reported tool or operation does not match its registered verifier rule",
            );
        }
        let subject = match verification_subject(&report.subject) {
            Ok(value) => value,
            Err(reason) => return reason,
        };
        if !rule.allowed_subject_kinds.contains(&subject.kind) {
            return unsupported(
                "verifier_subject_not_allowed",
                "registered verifier rule does not allow this subject kind",
            );
        }
        let components = BTreeMap::from([
            ("rule_id".into(), rule.id.clone()),
            ("rule_version".into(), rule.version.clone()),
            (
                "subject_kind".into(),
                subject_kind_name(subject.kind).into(),
            ),
            ("subject_value".into(), subject.value.clone()),
            ("exit_class".into(), "nonzero".into()),
        ]);
        signature(
            FailureDomain::RegisteredTool,
            FailureIdentityCapability::CompatibleOnly,
            components,
            self.id(),
            self.version(),
            scope(
                &evidence.envelope.workspace_id,
                &report.tool,
                Some(subject.value),
                None,
            ),
            vec![diagnostic_note(
                "compatible_only_identity",
                "the registered rule's generic nonzero exit class has no structured failure discriminator and cannot support exact equality",
            )],
        )
    }
}

fn signature(
    domain: FailureDomain,
    identity_capability: FailureIdentityCapability,
    components: BTreeMap<String, String>,
    id: &str,
    version: &str,
    scope: FailureScope,
    diagnostics: Vec<FailureNormalizationDiagnostic>,
) -> FailureNormalizationResult {
    match FailureSignature::new(domain, identity_capability, components, id, version, scope) {
        Ok(signature) => FailureNormalizationResult::Normalized {
            normalization: Box::new(FailureNormalization {
                signature,
                diagnostics,
            }),
        },
        Err(error) => unsupported("invalid_normalization_input", &error.to_string()),
    }
}

fn scope(
    workspace_id: &str,
    tool: &str,
    target: Option<String>,
    path: Option<String>,
) -> FailureScope {
    FailureScope {
        workspace_id: workspace_id.to_owned(),
        language: Some("rust".into()),
        tool: tool.into(),
        target,
        path,
        symbol_key: None,
    }
}

fn verification_subject(
    subject: &EvidenceSubject,
) -> std::result::Result<VerificationSubject, FailureNormalizationResult> {
    if !stable_component(&subject.value) {
        return Err(unsupported(
            "invalid_verification_subject",
            "verification subject must be a bounded stable identifier",
        ));
    }
    Ok(VerificationSubject {
        kind: match subject.kind {
            crate::domain::EvidenceSubjectKind::Workspace => VerificationSubjectKind::Workspace,
            crate::domain::EvidenceSubjectKind::Package => VerificationSubjectKind::Package,
            crate::domain::EvidenceSubjectKind::Target => VerificationSubjectKind::Target,
            crate::domain::EvidenceSubjectKind::Test => VerificationSubjectKind::Test,
            crate::domain::EvidenceSubjectKind::Path => VerificationSubjectKind::Path,
        },
        value: subject.value.clone(),
    })
}

fn subject_kind_name(kind: VerificationSubjectKind) -> &'static str {
    match kind {
        VerificationSubjectKind::Workspace => "workspace",
        VerificationSubjectKind::Package => "package",
        VerificationSubjectKind::Target => "target",
        VerificationSubjectKind::Test => "test",
        VerificationSubjectKind::Path => "path",
    }
}
fn unsupported(code: &str, message: &str) -> FailureNormalizationResult {
    FailureNormalizationResult::Unsupported {
        reason: UnsupportedFailureNormalization {
            code: code.into(),
            message: message.into(),
        },
    }
}
fn diagnostic_note(code: &str, message: &str) -> FailureNormalizationDiagnostic {
    FailureNormalizationDiagnostic {
        code: code.into(),
        message: message.into(),
    }
}
fn stable_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'_' | b'-' | b'.' | b':' | b'<' | b'>' | b',' | b' ' | b'&'
                )
        })
}
fn workspace_relative_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && !value.starts_with('/')
        && !value.starts_with('\\')
        && !value.contains(':')
        && !value.split('/').any(|part| part == ".." || part.is_empty())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/'))
}
fn is_rust_error_code(value: &str) -> bool {
    value.len() > 1
        && value.starts_with('E')
        && value[1..].bytes().all(|byte| byte.is_ascii_digit())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        CargoTestResultEvidence, EvidenceEnvelope, EvidenceObservation, EvidenceSubjectKind,
        GenericVerifierResultEvidence, RustCompilerResultEvidence, RustDiagnosticEvidence,
        TestFailureEvidence,
    };
    use chrono::Utc;

    fn envelope(contract: EvidenceContract, workspace: &str) -> EvidenceEnvelope {
        EvidenceEnvelope {
            event_id: "event-1".into(),
            workspace_id: workspace.into(),
            session_id: "session-1".into(),
            task_id: None,
            occurred_at: Utc::now(),
            contract,
            version: 1,
        }
    }
    fn subject(value: &str) -> EvidenceSubject {
        EvidenceSubject {
            kind: EvidenceSubjectKind::Target,
            value: value.into(),
        }
    }
    fn rust(
        message: &str,
        expected_type: Option<&str>,
        actual_type: Option<&str>,
        path: Option<&str>,
        target: &str,
    ) -> DecodedEvidence {
        DecodedEvidence {
            envelope: envelope(EvidenceContract::RustCompilerResult, "workspace-a"),
            observation: EvidenceObservation::RustCompilerResult(RustCompilerResultEvidence {
                subject: subject(target),
                process_outcome: ProcessOutcome::ExitedNonzero,
                exit_code: 1,
                diagnostics: vec![RustDiagnosticEvidence {
                    level: DiagnosticLevel::Error,
                    code: Some("E0308".into()),
                    message: message.into(),
                    expected_type: expected_type.map(Into::into),
                    actual_type: actual_type.map(Into::into),
                    path: path.map(Into::into),
                    start_line: Some(4),
                    start_column: Some(2),
                }],
            }),
        }
    }
    fn cargo(workspace: &str, message: &str) -> DecodedEvidence {
        DecodedEvidence {
            envelope: envelope(EvidenceContract::CargoTestResult, workspace),
            observation: EvidenceObservation::CargoTestResult(CargoTestResultEvidence {
                subject: subject("core-tests"),
                process_outcome: ProcessOutcome::ExitedNonzero,
                exit_code: 101,
                executed_test_count: 1,
                failures: vec![TestFailureEvidence {
                    test_name: Some("parser::round_trip".into()),
                    assertion_class: Some("assert_eq".into()),
                    message: Some(message.into()),
                }],
            }),
        }
    }
    fn generic(exit_code: i32) -> DecodedEvidence {
        DecodedEvidence {
            envelope: envelope(EvidenceContract::GenericVerifierResult, "workspace-a"),
            observation: EvidenceObservation::GenericVerifierResult(
                GenericVerifierResultEvidence {
                    verifier_id: "cargo.check".into(),
                    verifier_version: "1".into(),
                    tool: "cargo".into(),
                    operation: "check".into(),
                    reported_result: VerificationResult::Failed,
                    process_outcome: ProcessOutcome::ExitedNonzero,
                    exit_code: exit_code.into(),
                    subject: subject("core"),
                },
            ),
        }
    }
    fn key(result: FailureNormalizationResult) -> String {
        match result {
            FailureNormalizationResult::Normalized { normalization } => {
                normalization.signature.normalized_key
            }
            FailureNormalizationResult::Unsupported { reason } => {
                panic!("unexpected unsupported: {}", reason.code)
            }
        }
    }
    fn capability(result: FailureNormalizationResult) -> FailureIdentityCapability {
        match result {
            FailureNormalizationResult::Normalized { normalization } => {
                normalization.signature.identity_capability
            }
            FailureNormalizationResult::Unsupported { reason } => {
                panic!("unexpected unsupported: {}", reason.code)
            }
        }
    }

    #[test]
    fn compiler_signature_ignores_line_and_keeps_e0308_type_direction() {
        let normalizer = RustCompilerFailureNormalizer;
        let first = rust(
            "mismatched types in C:/tmp/build-1",
            Some("String"),
            Some("u32"),
            Some("src/lib.rs"),
            "core",
        );
        let mut second = first.clone();
        if let EvidenceObservation::RustCompilerResult(report) = &mut second.observation {
            report.diagnostics[0].start_line = Some(900);
            report.diagnostics[0].message = "mismatched types in D:/tmp/build-2".into();
        }
        assert_eq!(
            key(normalizer.normalize(&first)),
            key(normalizer.normalize(&second))
        );
        assert_ne!(
            key(normalizer.normalize(&first)),
            key(normalizer.normalize(&rust(
                "mismatched types",
                Some("u32"),
                Some("String"),
                Some("src/lib.rs"),
                "core"
            )))
        );
        assert_eq!(
            capability(normalizer.normalize(&first)),
            FailureIdentityCapability::Exact
        );
        assert_ne!(
            key(normalizer.normalize(&first)),
            key(normalizer.normalize(&rust(
                "mismatched types",
                Some("String"),
                Some("u32"),
                Some("src/other.rs"),
                "other"
            )))
        );
    }
    #[test]
    fn compiler_refuses_missing_structured_details() {
        let normalizer = RustCompilerFailureNormalizer;
        let result = normalizer.normalize(&rust(
            "mismatched types: expected `String`, found `u32`",
            None,
            None,
            Some("src/lib.rs"),
            "core",
        ));
        assert!(matches!(
            result,
            FailureNormalizationResult::Unsupported { reason }
                if reason.code == "missing_e0308_type_distinction"
        ));
        let result = normalizer.normalize(&rust(
            "mismatched types",
            Some("String"),
            Some("u32"),
            None,
            "core",
        ));
        assert!(matches!(
            result,
            FailureNormalizationResult::Unsupported { reason }
                if reason.code == "missing_failure_scope_path"
        ));
        let result = normalizer.normalize(&rust(
            "mismatched types",
            Some("String"),
            Some("u32"),
            Some("C:/temporary/lib.rs"),
            "core",
        ));
        assert!(matches!(
            result,
            FailureNormalizationResult::Unsupported { .. }
        ));
    }
    #[test]
    fn cargo_signature_scopes_workspace_and_excludes_prose() {
        let normalizer = CargoTestFailureNormalizer;
        assert_eq!(
            key(normalizer.normalize(&cargo("workspace-a", "temporary path 1"))),
            key(normalizer.normalize(&cargo("workspace-a", "temporary path 2")))
        );
        assert_ne!(
            key(normalizer.normalize(&cargo("workspace-a", "x"))),
            key(normalizer.normalize(&cargo("workspace-b", "x")))
        );
        assert_eq!(
            capability(normalizer.normalize(&cargo("workspace-a", "x"))),
            FailureIdentityCapability::CompatibleOnly
        );
    }

    #[test]
    fn exact_capability_metrics_have_no_false_equivalence_or_false_split() {
        struct FixturePair {
            left: DecodedEvidence,
            right: DecodedEvidence,
            exact_equivalence_supported: bool,
        }

        let base = rust(
            "mismatched types in C:/tmp/build-1",
            Some("String"),
            Some("u32"),
            Some("src/lib.rs"),
            "core",
        );
        let mut shifted = base.clone();
        if let EvidenceObservation::RustCompilerResult(report) = &mut shifted.observation {
            report.diagnostics[0].start_line = Some(800);
            report.diagnostics[0].start_column = Some(21);
            report.diagnostics[0].message = "mismatched types in D:/tmp/build-9".into();
        }
        let fixtures = vec![
            FixturePair {
                left: base.clone(),
                right: shifted,
                exact_equivalence_supported: true,
            },
            FixturePair {
                left: cargo("workspace-a", "C:/tmp/run-1/assertion failed"),
                right: cargo("workspace-a", "D:/tmp/run-2/assertion failed"),
                exact_equivalence_supported: false,
            },
            FixturePair {
                left: base.clone(),
                right: rust(
                    "mismatched types",
                    Some("u32"),
                    Some("String"),
                    Some("src/lib.rs"),
                    "core",
                ),
                exact_equivalence_supported: false,
            },
            FixturePair {
                left: base,
                right: rust(
                    "mismatched types",
                    Some("String"),
                    Some("u32"),
                    Some("src/other.rs"),
                    "other",
                ),
                exact_equivalence_supported: false,
            },
            FixturePair {
                left: cargo("workspace-a", "failed"),
                right: cargo("workspace-b", "failed"),
                exact_equivalence_supported: false,
            },
        ];

        let service = FailureNormalizationService::standard().unwrap();
        let mut true_positive = 0;
        let mut true_negative = 0;
        let mut false_positive = 0;
        let mut false_negative = 0;
        for fixture in fixtures {
            let left = service.normalize(&fixture.left);
            let right = service.normalize(&fixture.right);
            let predicted_same = capability(left.clone()) == FailureIdentityCapability::Exact
                && capability(right.clone()) == FailureIdentityCapability::Exact
                && key(left) == key(right);
            match (fixture.exact_equivalence_supported, predicted_same) {
                (true, true) => true_positive += 1,
                (false, false) => true_negative += 1,
                (false, true) => false_positive += 1,
                (true, false) => false_negative += 1,
            }
        }
        assert_eq!(
            (true_positive, true_negative, false_positive, false_negative),
            (1, 4, 0, 0)
        );
    }

    #[test]
    fn same_scope_coarse_classes_never_claim_exact_identity() {
        let service = FailureNormalizationService::standard().unwrap();

        let mut rust_left = rust(
            "cannot borrow `left` as mutable",
            None,
            None,
            Some("src/lib.rs"),
            "core",
        );
        let mut rust_right = rust_left.clone();
        for (evidence, message, line) in [
            (&mut rust_left, "cannot borrow `left` as mutable", 4),
            (&mut rust_right, "cannot borrow `right` as mutable", 400),
        ] {
            let EvidenceObservation::RustCompilerResult(report) = &mut evidence.observation else {
                unreachable!()
            };
            report.diagnostics[0].code = Some("E0499".into());
            report.diagnostics[0].message = message.into();
            report.diagnostics[0].start_line = Some(line);
        }
        let rust_left = service.normalize(&rust_left);
        let rust_right = service.normalize(&rust_right);
        assert_eq!(key(rust_left.clone()), key(rust_right.clone()));
        assert_eq!(
            capability(rust_left),
            FailureIdentityCapability::CompatibleOnly
        );
        assert_eq!(
            capability(rust_right),
            FailureIdentityCapability::CompatibleOnly
        );

        let cargo_left = service.normalize(&cargo("workspace-a", "left invariant failed"));
        let cargo_right = service.normalize(&cargo("workspace-a", "right invariant failed"));
        assert_eq!(key(cargo_left.clone()), key(cargo_right.clone()));
        assert_eq!(
            capability(cargo_left),
            FailureIdentityCapability::CompatibleOnly
        );
        assert_eq!(
            capability(cargo_right),
            FailureIdentityCapability::CompatibleOnly
        );

        let generic_left = service.normalize(&generic(1));
        let generic_right = service.normalize(&generic(101));
        assert_eq!(key(generic_left.clone()), key(generic_right.clone()));
        assert_eq!(
            capability(generic_left),
            FailureIdentityCapability::CompatibleOnly
        );
        assert_eq!(
            capability(generic_right),
            FailureIdentityCapability::CompatibleOnly
        );
    }
    #[test]
    fn generic_failures_require_an_exact_rule() {
        let service = FailureNormalizationService::standard().unwrap();
        let evidence = generic(101);
        assert!(matches!(
            service.normalize(&evidence),
            FailureNormalizationResult::Normalized { .. }
        ));
        assert_eq!(
            capability(service.normalize(&evidence)),
            FailureIdentityCapability::CompatibleOnly
        );
        let mut unknown = evidence;
        if let EvidenceObservation::GenericVerifierResult(report) = &mut unknown.observation {
            report.verifier_id = "unknown".into();
        }
        assert!(matches!(
            service.normalize(&unknown),
            FailureNormalizationResult::Unsupported { .. }
        ));
    }
}
