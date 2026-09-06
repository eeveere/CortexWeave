use std::collections::BTreeMap;

use serde_json::json;

use crate::{
    CortexError, Result,
    domain::{
        DecodedEvidence, EvidenceObservation, MAX_FAILURE_COMPONENT_BYTES, ProcessOutcome,
        TestCaseIdentity, TestCaseStatus, TestEvidenceProfile, TestExecutionOrigin,
        TestRunEligibility, TestRunEligibilityIssue, TestRunEligibilityStatus,
        TestRunResultEvidence, TestSnapshotMode, VerificationInputKind, VerificationInputState,
    },
};

/// Exact registry for capture profiles qualified by a concrete producer,
/// runner and runtime version. Unknown versions remain inspectable evidence but
/// never acquire automatic verification authority.
pub struct TestEvidenceProfileRegistry {
    profiles: BTreeMap<(String, String), TestEvidenceProfile>,
}

impl TestEvidenceProfileRegistry {
    pub fn new(profiles: Vec<TestEvidenceProfile>) -> Result<Self> {
        let mut registered = BTreeMap::new();
        for profile in profiles {
            if profile.id.trim().is_empty()
                || profile.version.trim().is_empty()
                || profile.operation.trim().is_empty()
                || profile.selection_kinds.is_empty()
                || profile.supported_languages.is_empty()
            {
                return Err(CortexError::Configuration(
                    "test evidence profiles require IDs, versions, an operation, selection kinds, and languages"
                        .into(),
                ));
            }
            let key = (profile.id.clone(), profile.version.clone());
            if registered.insert(key.clone(), profile).is_some() {
                return Err(CortexError::Configuration(format!(
                    "ambiguous test evidence profile registration for {} v{}",
                    key.0, key.1
                )));
            }
        }
        Ok(Self {
            profiles: registered,
        })
    }

    pub fn standard() -> Result<Self> {
        Self::new(vec![
            TestEvidenceProfile::vitest_4_1_10_node_24_19_0(),
            TestEvidenceProfile::unittest_python_3_14_7(),
        ])
    }

    pub fn find(&self, id: &str, version: &str) -> Option<&TestEvidenceProfile> {
        self.profiles.get(&(id.to_owned(), version.to_owned()))
    }

    pub fn catalog(&self) -> Vec<TestEvidenceProfile> {
        self.profiles.values().cloned().collect()
    }
}

pub struct TestEvidenceService {
    profiles: TestEvidenceProfileRegistry,
}

impl TestEvidenceService {
    pub fn new(profiles: TestEvidenceProfileRegistry) -> Self {
        Self { profiles }
    }

    pub fn standard() -> Result<Self> {
        Ok(Self::new(TestEvidenceProfileRegistry::standard()?))
    }

    pub fn profiles(&self) -> &TestEvidenceProfileRegistry {
        &self.profiles
    }

    pub fn assess(&self, evidence: &DecodedEvidence) -> TestRunEligibility {
        let EvidenceObservation::TestRunResult(report) = &evidence.observation else {
            return ineligible(
                None,
                vec![eligibility_issue(
                    "not_test_run_evidence",
                    "the decoded observation is not a normalized test run",
                )],
            );
        };
        self.assess_report(report)
    }

    pub fn assess_report(&self, report: &TestRunResultEvidence) -> TestRunEligibility {
        let Some(profile) = self
            .profiles
            .find(&report.profile.id, &report.profile.version)
        else {
            return ineligible(
                None,
                vec![eligibility_issue(
                    "unsupported_test_profile",
                    "no exact qualified test evidence profile matches the report",
                )],
            );
        };

        let profile_identity = Some((profile.id.clone(), profile.version.clone()));
        let mut issues = Vec::new();
        if report.producer != profile.producer
            || report.runner != profile.runner
            || report.runtime != profile.runtime
            || report.operation != profile.operation
        {
            issues.push(eligibility_issue(
                "test_profile_component_mismatch",
                "producer, runner, runtime, and operation must exactly match the qualified profile",
            ));
        }
        if !profile.selection_kinds.contains(&report.selection.kind) {
            issues.push(eligibility_issue(
                "unsupported_test_selection",
                "the selected file, module, or class scope is not allowed by this profile",
            ));
        }
        if report
            .language
            .as_ref()
            .is_some_and(|language| !profile.supported_languages.contains(language))
        {
            issues.push(eligibility_issue(
                "unsupported_test_language",
                "the reported language is not qualified for this runner profile",
            ));
        }
        validate_profile_file(profile, report, &mut issues);
        validate_failure_identity_bounds(report, &mut issues);

        if !report.completed || report.process_outcome.is_none() || report.exit_code.is_none() {
            issues.push(eligibility_issue(
                "incomplete_test_run",
                "automatic verification requires a completed process with an observed exit",
            ));
        }
        if !report.selection.inventory_complete {
            issues.push(eligibility_issue(
                "incomplete_test_inventory",
                "the selected scope does not carry a complete case inventory",
            ));
        }
        if report.counts.parents.discovered == 0 || report.counts.parents.executed == 0 {
            issues.push(eligibility_issue(
                "empty_test_run",
                "automatic verification requires a nonempty executed parent inventory",
            ));
        }
        if !report.errors.is_empty() {
            issues.push(eligibility_issue(
                "test_run_errors",
                "discovery, import, setup, teardown, cleanup, or runtime errors are ineligible",
            ));
        }
        if !report.children.is_empty() {
            issues.push(eligibility_issue(
                "child_observations_unsupported",
                "the initial profiles preserve child observations but do not verify subtest identity",
            ));
        }

        for (count, code, message) in [
            (
                report.counts.parents.skipped,
                "skipped_tests",
                "skipped cases make the selected inventory incomplete for automatic verification",
            ),
            (
                report.counts.parents.todo,
                "todo_tests",
                "todo cases are not executed verification evidence",
            ),
            (
                report.counts.parents.pending,
                "pending_tests",
                "pending cases are not completed verification evidence",
            ),
            (
                report.counts.parents.expected_failed,
                "expected_failures",
                "expected failures remain distinct and are ineligible in the initial profile",
            ),
            (
                report.counts.parents.unexpected_successful,
                "unexpected_successes",
                "unexpected successes remain distinct and are ineligible in the initial profile",
            ),
            (
                report.counts.parents.errored,
                "test_execution_errors",
                "execution errors are not assertion failures or verified passes",
            ),
        ] {
            if count > 0 {
                issues.push(eligibility_issue(code, message));
            }
        }
        if report
            .cases
            .iter()
            .any(|case| case.retry_configured || case.retry_count > 0 || case.flaky)
        {
            issues.push(eligibility_issue(
                "test_retries_unsupported",
                "retried or flaky case results are ineligible in the initial profile",
            ));
        }
        if report
            .cases
            .iter()
            .any(|case| case.repeat_configured || case.repeat_count > 0)
        {
            issues.push(eligibility_issue(
                "test_repeats_unsupported",
                "repeated case results are ineligible in the initial profile",
            ));
        }
        for (active, code, message) in [
            (
                report.settings.focused_only,
                "focused_selection_unsupported",
                "focused-only execution cannot prove a complete selected inventory",
            ),
            (
                report.settings.name_filter,
                "name_filter_unsupported",
                "case-name filtering cannot prove a complete selected inventory",
            ),
            (
                report.settings.sharded,
                "sharded_execution_unsupported",
                "one shard cannot prove the complete selected inventory",
            ),
            (
                report.settings.bail,
                "bail_execution_unsupported",
                "bail execution can omit later cases",
            ),
            (
                report.settings.watch,
                "watch_execution_unsupported",
                "watch reruns are outside the initial completed-run profile",
            ),
            (
                report.settings.pass_with_no_tests,
                "pass_with_no_tests_unsupported",
                "profiles that permit empty success are outside the initial qualification",
            ),
        ] {
            if active {
                issues.push(eligibility_issue(code, message));
            }
        }
        if matches!(
            report.settings.snapshot_mode,
            TestSnapshotMode::CreateMissing | TestSnapshotMode::UpdateAll
        ) {
            issues.push(eligibility_issue(
                "snapshot_update_unsupported",
                "snapshot-writing runs can change their own verification inputs",
            ));
        }
        if report.settings.execution_origin != TestExecutionOrigin::Current {
            issues.push(eligibility_issue(
                "non_current_test_execution",
                "cached or replayed results are not fresh verification evidence",
            ));
        }

        validate_verification_input_eligibility(report, &mut issues);
        if report.artifacts.is_empty() {
            issues.push(eligibility_issue(
                "missing_capture_artifact",
                "a qualified run requires at least one digest-bound capture artifact",
            ));
        }

        let assertion_failures = report.counts.parents.assertion_failed;
        if assertion_failures > 1 {
            issues.push(eligibility_issue(
                "multiple_test_failures",
                "the initial profile normalizes exactly one failed parent case",
            ));
        }
        let candidate_status = match (
            report.process_outcome,
            assertion_failures,
            report.counts.parents.passed == report.counts.parents.executed,
        ) {
            (Some(ProcessOutcome::ExitedZero), 0, true) => TestRunEligibilityStatus::EligiblePassed,
            (Some(ProcessOutcome::ExitedNonzero), 1, _) => TestRunEligibilityStatus::EligibleFailed,
            _ => {
                issues.push(eligibility_issue(
                    "unsupported_test_outcome",
                    "the initial profile supports all-pass runs or exactly one assertion-failed parent",
                ));
                TestRunEligibilityStatus::Ineligible
            }
        };

        if issues.is_empty() {
            TestRunEligibility {
                status: candidate_status,
                profile_id: profile_identity.as_ref().map(|value| value.0.clone()),
                profile_version: profile_identity.map(|value| value.1),
                issues,
            }
        } else {
            ineligible(profile_identity, issues)
        }
    }

    /// Canonical verification scope intentionally excludes outcome, run ID,
    /// timing, artifacts and diagnostic prose. Failure and later pass compare
    /// equal only when the complete selected inventory and verification inputs
    /// are unchanged.
    pub fn scope_fingerprint(&self, report: &TestRunResultEvidence) -> Result<String> {
        let mut cases: Vec<TestCaseIdentity> = report
            .cases
            .iter()
            .map(|case| case.identity.clone())
            .collect();
        cases.sort();
        let mut inputs = report.verification_inputs.clone();
        inputs.sort_by(|left, right| {
            (left.kind, left.path.as_str()).cmp(&(right.kind, right.path.as_str()))
        });
        let scope = json!({
            "profile": report.profile,
            "producer": report.producer,
            "runner": report.runner,
            "runtime": report.runtime,
            "operation": report.operation,
            "language": report.language,
            "environment_fingerprint": report.environment_fingerprint,
            "selection": {
                "project_root": report.selection.project_root,
                "import_root": report.selection.import_root,
                "test_file": report.selection.test_file,
                "kind": report.selection.kind,
                "value": report.selection.value,
                "settings_digest": report.selection.settings_digest,
            },
            "cases": cases,
            "settings": report.settings,
            "verification_inputs": inputs,
        });
        let canonical = crate::domain::canonical_json(&scope)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"cortexweave.test-verification-scope.v1\0");
        hasher.update(canonical.as_bytes());
        Ok(hasher.finalize().to_hex().to_string())
    }
}

fn validate_failure_identity_bounds(
    report: &TestRunResultEvidence,
    issues: &mut Vec<TestRunEligibilityIssue>,
) {
    let namespace = report
        .cases
        .iter()
        .find(|case| case.status == TestCaseStatus::AssertionFailed)
        .map(|case| {
            let mut value = case.identity.namespace.len().to_string();
            value.push(':');
            for component in &case.identity.namespace {
                value.push_str(&component.len().to_string());
                value.push(':');
                value.push_str(component);
            }
            value
        });
    let oversize = [
        Some(report.profile.id.as_str()),
        Some(report.profile.version.as_str()),
        Some(report.runner.id.as_str()),
        Some(report.runner.version.as_str()),
        Some(report.selection.project_root.as_str()),
        Some(report.selection.import_root.as_str()),
        Some(report.selection.value.as_str()),
        Some(report.selection.test_file.as_str()),
        namespace.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|value| value.len() > MAX_FAILURE_COMPONENT_BYTES);
    let failed_case_oversize = report.cases.iter().any(|case| {
        case.status == TestCaseStatus::AssertionFailed
            && (case.identity.name.len() > MAX_FAILURE_COMPONENT_BYTES
                || case
                    .error_class
                    .as_ref()
                    .is_some_and(|value| value.len() > MAX_FAILURE_COMPONENT_BYTES))
    });
    if oversize || failed_case_oversize {
        issues.push(eligibility_issue(
            "test_failure_identity_too_large",
            "runner, selection, path, or failed-case identity exceeds the canonical failure-signature bound",
        ));
    }
}

fn validate_profile_file(
    profile: &TestEvidenceProfile,
    report: &TestRunResultEvidence,
    issues: &mut Vec<TestRunEligibilityIssue>,
) {
    let path = report.selection.test_file.to_ascii_lowercase();
    let supported = match profile.runner.id.as_str() {
        "vitest" => [".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"]
            .iter()
            .any(|extension| path.ends_with(extension)),
        "python.unittest" => path.ends_with(".py"),
        _ => false,
    };
    if !supported {
        issues.push(eligibility_issue(
            "unsupported_test_file",
            "the selected test file extension is not supported by the qualified runner profile",
        ));
    }
    if report.selection.kind == crate::domain::TestSelectionKind::File
        && report.selection.value != report.selection.test_file
    {
        issues.push(eligibility_issue(
            "file_selection_mismatch",
            "a file selection value must equal the normalized selected test file",
        ));
    }
}

fn validate_verification_input_eligibility(
    report: &TestRunResultEvidence,
    issues: &mut Vec<TestRunEligibilityIssue>,
) {
    let mut selected_file_present = false;
    for input in &report.verification_inputs {
        match &input.observed {
            VerificationInputState::Present {
                before_digest,
                after_digest,
            } => {
                if before_digest != after_digest {
                    issues.push(eligibility_issue(
                        "verification_input_changed",
                        "test, configuration, and dependency inputs must remain unchanged during capture",
                    ));
                }
                if input.kind == VerificationInputKind::TestFile
                    && input.path == report.selection.test_file
                {
                    selected_file_present = true;
                }
            }
            VerificationInputState::Absent => {
                if input.kind == VerificationInputKind::TestFile
                    && input.path == report.selection.test_file
                {
                    issues.push(eligibility_issue(
                        "selected_test_file_absent",
                        "the selected test file cannot be represented by an absence marker",
                    ));
                }
            }
        }
    }
    if !selected_file_present {
        issues.push(eligibility_issue(
            "missing_test_file_fingerprint",
            "verification inputs must include an unchanged digest for the selected test file",
        ));
    }
}

fn ineligible(
    profile: Option<(String, String)>,
    issues: Vec<TestRunEligibilityIssue>,
) -> TestRunEligibility {
    TestRunEligibility {
        status: TestRunEligibilityStatus::Ineligible,
        profile_id: profile.as_ref().map(|value| value.0.clone()),
        profile_version: profile.map(|value| value.1),
        issues,
    }
}

fn eligibility_issue(code: &str, message: &str) -> TestRunEligibilityIssue {
    TestRunEligibilityIssue {
        code: code.into(),
        message: message.into(),
    }
}
