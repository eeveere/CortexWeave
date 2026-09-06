use std::{fs, path::PathBuf, sync::Arc};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, FailureNormalizationService, Result,
    domain::{
        AttemptResult, ChildTestCounts, ConsolidationAcceptance, ConsolidationAcceptanceRequest,
        ConsolidationPreview, ConsolidationRequest, CortexEvent, EpisodeCreator,
        EpisodeEventAssociationRequest, EpisodeStartRequest, EpisodeTerminalRequest, EpisodeType,
        EventType, EvidenceDecodeResult, ExperienceAssessmentKind,
        ExperienceAssessmentReviewRequest, ExperienceOutcome, FailureDomain,
        FailureNormalizationResult, MemoryKind, MemoryRecord, NativeDeliveryRequest,
        NativeOperation, ParentTestCounts, ProcessOutcome, ProposalDisposition,
        TestArtifactEvidence, TestCaseIdentity, TestCaseObservation, TestCaseStatus,
        TestEvidenceProfile, TestEvidenceRecordRequest, TestExecutionOrigin, TestRunCounts,
        TestRunEligibilityStatus, TestRunResultEvidence, TestRunSelection, TestRunSettings,
        TestSelectionKind, TestSnapshotMode, TestVerificationInput, VerificationInputKind,
        VerificationInputState, VerificationKind, VerificationStatus,
        WorkspaceDeregistrationOutcome, WorkspaceDeregistrationRequest,
        WorkspaceDeregistrationStaleReason,
    },
    embedding::EmbeddingProvider,
    service::TestEvidenceService,
    storage::SqliteStorage,
    workspace::WorkspaceSelector,
};
use serde_json::{Map, Value};
use tempfile::tempdir;

const DIGEST_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct ContractEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for ContractEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0, 1.0]).collect())
    }

    fn model_name(&self) -> &str {
        "test-evidence-contract"
    }

    fn dimension(&self) -> Option<usize> {
        Some(2)
    }
}

fn counts(cases: &[TestCaseObservation]) -> TestRunCounts {
    let mut parents = ParentTestCounts {
        discovered: cases.len() as u64,
        ..ParentTestCounts::default()
    };
    for case in cases {
        if case.status.was_executed() {
            parents.executed += 1;
        }
        match case.status {
            TestCaseStatus::Passed => parents.passed += 1,
            TestCaseStatus::AssertionFailed => parents.assertion_failed += 1,
            TestCaseStatus::Errored => parents.errored += 1,
            TestCaseStatus::Skipped => parents.skipped += 1,
            TestCaseStatus::Todo => parents.todo += 1,
            TestCaseStatus::Pending => parents.pending += 1,
            TestCaseStatus::ExpectedFailed => parents.expected_failed += 1,
            TestCaseStatus::UnexpectedSuccessful => parents.unexpected_successful += 1,
        }
    }
    TestRunCounts {
        parents,
        children: ChildTestCounts::default(),
    }
}

fn case(status: TestCaseStatus, namespace: &[&str], name: &str) -> TestCaseObservation {
    TestCaseObservation {
        identity: TestCaseIdentity {
            namespace: namespace.iter().map(|value| (*value).to_owned()).collect(),
            name: name.into(),
        },
        status,
        error_class: matches!(
            status,
            TestCaseStatus::AssertionFailed
                | TestCaseStatus::Errored
                | TestCaseStatus::ExpectedFailed
        )
        .then(|| {
            if status == TestCaseStatus::Errored {
                "TypeError".into()
            } else {
                "AssertionError".into()
            }
        }),
        retry_configured: false,
        retry_count: 0,
        repeat_configured: false,
        repeat_count: 0,
        flaky: false,
    }
}

fn report(vitest: bool, cases: Vec<TestCaseObservation>) -> TestRunResultEvidence {
    let profile = if vitest {
        TestEvidenceProfile::vitest_4_1_10_node_24_19_0()
    } else {
        TestEvidenceProfile::unittest_python_3_14_7()
    };
    let failed = cases.iter().any(|case| {
        matches!(
            case.status,
            TestCaseStatus::AssertionFailed
                | TestCaseStatus::Errored
                | TestCaseStatus::UnexpectedSuccessful
        )
    });
    let test_file = if vitest {
        "tests/unit/hybrid-retrieval.test.ts"
    } else {
        "tests/test_core.py"
    };
    let selection_kind = if vitest {
        TestSelectionKind::File
    } else {
        TestSelectionKind::Class
    };
    let selection_value = if vitest {
        test_file
    } else {
        "tests.test_core.StagnationTests"
    };
    let language = if vitest { "typescript" } else { "python" };
    let counts = counts(&cases);
    TestRunResultEvidence {
        producer: profile.producer.clone(),
        runner: profile.runner.clone(),
        runtime: profile.runtime.clone(),
        profile: cortexweave::domain::VersionedTestComponent {
            id: profile.id,
            version: profile.version,
        },
        run_id: "run-0001".into(),
        operation: "test".into(),
        completed: true,
        process_outcome: Some(if failed {
            ProcessOutcome::ExitedNonzero
        } else {
            ProcessOutcome::ExitedZero
        }),
        exit_code: Some(if failed { 1 } else { 0 }),
        language: Some(language.into()),
        environment_fingerprint: DIGEST_A.into(),
        selection: TestRunSelection {
            project_root: ".".into(),
            import_root: ".".into(),
            test_file: test_file.into(),
            kind: selection_kind,
            value: selection_value.into(),
            settings_digest: DIGEST_A.into(),
            inventory_complete: true,
        },
        cases,
        children: Vec::new(),
        counts,
        errors: Vec::new(),
        settings: TestRunSettings {
            focused_only: false,
            name_filter: false,
            sharded: false,
            bail: false,
            watch: false,
            snapshot_mode: if vitest {
                TestSnapshotMode::Disabled
            } else {
                TestSnapshotMode::NotApplicable
            },
            pass_with_no_tests: false,
            execution_origin: TestExecutionOrigin::Current,
        },
        verification_inputs: vec![TestVerificationInput {
            kind: VerificationInputKind::TestFile,
            path: test_file.into(),
            observed: VerificationInputState::Present {
                before_digest: DIGEST_A.into(),
                after_digest: DIGEST_A.into(),
            },
        }],
        artifacts: vec![TestArtifactEvidence {
            kind: "normalized_report".into(),
            digest: DIGEST_B.into(),
            reference: "capture/report.json".into(),
        }],
    }
}

fn event(report: &TestRunResultEvidence) -> CortexEvent {
    let mut payload = serde_json::to_value(report).unwrap();
    let object = payload.as_object_mut().unwrap();
    object.remove("process_outcome");
    object.insert(
        "contract".into(),
        Value::String("cortexweave.test_run_result".into()),
    );
    object.insert("version".into(), Value::from(1));
    let mut event = CortexEvent::new("workspace-a", EventType::TestResult, payload);
    event.session_id = Some("session-a".into());
    event
}

fn tool_action(workspace_id: &str, operation: &str) -> CortexEvent {
    let mut event = CortexEvent::new(
        workspace_id,
        EventType::ExternalToolFinished,
        serde_json::json!({
            "contract": "cortexweave.external_tool_completion",
            "version": 1,
            "tool": "editor",
            "operation": operation,
            "exit_code": 0,
            "error_class": null,
            "message": null
        }),
    );
    event.id = format!("event-{operation}");
    event
}

fn decode(report: &TestRunResultEvidence) -> cortexweave::domain::DecodedEvidence {
    match cortexweave::EventEvidenceDecoderRegistry::standard()
        .unwrap()
        .decode(&event(report))
    {
        EvidenceDecodeResult::Decoded { evidence } => *evidence,
        other => panic!("unexpected decode result: {other:?}"),
    }
}

fn normalized(report: &TestRunResultEvidence) -> cortexweave::domain::FailureNormalization {
    match FailureNormalizationService::standard()
        .unwrap()
        .normalize(&decode(report))
    {
        FailureNormalizationResult::Normalized { normalization } => *normalization,
        other => panic!("unexpected normalization result: {other:?}"),
    }
}

#[test]
fn qualified_vitest_and_unittest_share_one_contract() {
    let service = TestEvidenceService::standard().unwrap();
    let vitest_failure = report(
        true,
        vec![case(
            TestCaseStatus::AssertionFailed,
            &["hybrid retrieval"],
            "uses a stable tie break",
        )],
    );
    let unittest_failure = report(
        false,
        vec![case(
            TestCaseStatus::AssertionFailed,
            &["tests.test_core", "StagnationTests"],
            "test_stagnation_limit",
        )],
    );

    for report in [&vitest_failure, &unittest_failure] {
        let evidence = decode(report);
        let assessment = service.assess(&evidence);
        assert_eq!(assessment.status, TestRunEligibilityStatus::EligibleFailed);
        let normalization = normalized(report);
        assert_eq!(normalization.signature.domain, FailureDomain::TestRun);
        assert!(!normalization.signature.is_exact_capable());
    }

    let vitest_pass = report(
        true,
        vec![case(
            TestCaseStatus::Passed,
            &["hybrid retrieval"],
            "uses a stable tie break",
        )],
    );
    let unittest_pass = report(
        false,
        vec![case(
            TestCaseStatus::Passed,
            &["tests.test_core", "StagnationTests"],
            "test_stagnation_limit",
        )],
    );
    for report in [&vitest_pass, &unittest_pass] {
        assert_eq!(
            service.assess(&decode(report)).status,
            TestRunEligibilityStatus::EligiblePassed
        );
        assert!(matches!(
            FailureNormalizationService::standard()
                .unwrap()
                .normalize(&decode(report)),
            FailureNormalizationResult::Unsupported { .. }
        ));
    }
}

#[test]
fn runner_qualified_identity_and_language_do_not_cross() {
    let vitest = report(
        true,
        vec![case(
            TestCaseStatus::AssertionFailed,
            &["shared"],
            "same name",
        )],
    );
    let mut unittest = report(
        false,
        vec![case(
            TestCaseStatus::AssertionFailed,
            &["shared"],
            "same name",
        )],
    );
    unittest.language = None;
    let vitest_signature = normalized(&vitest).signature;
    let unittest_signature = normalized(&unittest).signature;
    assert_ne!(
        vitest_signature.normalized_key,
        unittest_signature.normalized_key
    );
    assert_eq!(
        vitest_signature.scope.language.as_deref(),
        Some("typescript")
    );
    assert_eq!(unittest_signature.scope.language, None);
}

#[test]
fn distinct_outcomes_remain_visible_but_ineligible() {
    let service = TestEvidenceService::standard().unwrap();
    for status in [
        TestCaseStatus::Errored,
        TestCaseStatus::Skipped,
        TestCaseStatus::Todo,
        TestCaseStatus::Pending,
        TestCaseStatus::ExpectedFailed,
        TestCaseStatus::UnexpectedSuccessful,
    ] {
        let decoded = decode(&report(true, vec![case(status, &["suite"], "case")]));
        let cortexweave::domain::EvidenceObservation::TestRunResult(observation) =
            &decoded.observation
        else {
            unreachable!()
        };
        assert_eq!(observation.cases[0].status, status);
        assert_eq!(
            service.assess(&decoded).status,
            TestRunEligibilityStatus::Ineligible,
            "{status:?} unexpectedly became eligible"
        );
    }
}

#[test]
fn partial_retry_repeat_child_and_input_drift_are_ineligible() {
    let service = TestEvidenceService::standard().unwrap();
    let base = report(
        false,
        vec![case(
            TestCaseStatus::Passed,
            &["tests.test_core", "StagnationTests"],
            "test_stagnation_limit",
        )],
    );
    let mut variants = Vec::new();

    let mut partial = base.clone();
    partial.selection.inventory_complete = false;
    variants.push(partial);

    let mut retry = base.clone();
    retry.cases[0].retry_configured = true;
    retry.cases[0].retry_count = 1;
    retry.cases[0].flaky = true;
    variants.push(retry);

    let mut repeat = base.clone();
    repeat.cases[0].repeat_configured = true;
    repeat.cases[0].repeat_count = 1;
    variants.push(repeat);

    let mut child = base.clone();
    child
        .children
        .push(cortexweave::domain::TestChildObservation {
            parent: child.cases[0].identity.clone(),
            ordinal: 0,
            description: Some("value=1".into()),
            status: TestCaseStatus::Passed,
            error_class: None,
        });
    child.counts.children.observed = 1;
    child.counts.children.passed = 1;
    variants.push(child);

    let mut changed = base;
    changed.verification_inputs[0].observed = VerificationInputState::Present {
        before_digest: DIGEST_A.into(),
        after_digest: DIGEST_B.into(),
    };
    variants.push(changed);

    for variant in variants {
        assert_eq!(
            service.assess(&decode(&variant)).status,
            TestRunEligibilityStatus::Ineligible
        );
    }
}

#[test]
fn decoder_rejects_count_conflicts_and_parent_overflow() {
    let decoder = cortexweave::EventEvidenceDecoderRegistry::standard().unwrap();
    let base = report(true, vec![case(TestCaseStatus::Passed, &["suite"], "case")]);
    let mut mismatched = event(&base);
    mismatched.payload["counts"]["parents"]["passed"] = Value::from(2);
    assert!(matches!(
        decoder.decode(&mismatched),
        EvidenceDecodeResult::Invalid { issue } if issue.code == "test_count_mismatch"
    ));

    let mut overflow = base;
    overflow.cases = (0..257)
        .map(|index| case(TestCaseStatus::Passed, &["suite"], &format!("case-{index}")))
        .collect();
    overflow.counts = counts(&overflow.cases);
    assert!(matches!(
        decoder.decode(&event(&overflow)),
        EvidenceDecodeResult::Invalid { issue }
            if matches!(issue.code.as_str(), "too_many_test_cases" | "payload_too_large")
    ));
}

#[test]
fn unknown_profile_and_multiple_failures_never_normalize() {
    let service = TestEvidenceService::standard().unwrap();
    let mut unknown = report(
        true,
        vec![case(TestCaseStatus::AssertionFailed, &["suite"], "case")],
    );
    unknown.profile.version = "2".into();
    assert_eq!(
        service.assess(&decode(&unknown)).status,
        TestRunEligibilityStatus::Ineligible
    );

    let multiple = report(
        true,
        vec![
            case(TestCaseStatus::AssertionFailed, &["suite"], "first"),
            case(TestCaseStatus::AssertionFailed, &["suite"], "second"),
        ],
    );
    assert_eq!(
        service.assess(&decode(&multiple)).status,
        TestRunEligibilityStatus::Ineligible
    );
    assert!(matches!(
        FailureNormalizationService::standard()
            .unwrap()
            .normalize(&decode(&multiple)),
        FailureNormalizationResult::Unsupported { .. }
    ));
}

#[test]
fn payload_schema_rejects_unknown_fields_and_exit_conflicts() {
    let decoder = cortexweave::EventEvidenceDecoderRegistry::standard().unwrap();
    let passed = report(true, vec![case(TestCaseStatus::Passed, &["suite"], "case")]);
    let mut unknown = event(&passed);
    unknown.payload["selection"]
        .as_object_mut()
        .unwrap()
        .insert("framework_private_state".into(), Value::Bool(true));
    assert!(matches!(
        decoder.decode(&unknown),
        EvidenceDecodeResult::Invalid { issue } if issue.code == "invalid_payload_shape"
    ));

    let mut conflict = event(&passed);
    conflict.payload["exit_code"] = Value::from(1);
    assert!(matches!(
        decoder.decode(&conflict),
        EvidenceDecodeResult::Invalid { issue }
            if issue.code == "conflicting_test_result_and_exit_code"
    ));

    let _type_check: Map<String, Value> = conflict.payload.as_object().unwrap().clone();
}

#[tokio::test]
async fn native_facade_uses_the_same_profile_assessment() {
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();
    let evidence = decode(&report(
        true,
        vec![case(TestCaseStatus::Passed, &["suite"], "case")],
    ));
    assert_eq!(
        service.assess_decoded_test_run(&evidence).status,
        TestRunEligibilityStatus::EligiblePassed
    );
}

#[test]
fn bp0_runner_fixtures_are_portable_json() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test_evidence");
    for file in [
        "vitest-4.1.10-observations.json",
        "unittest-python-3.14.7-observations.json",
    ] {
        let text = fs::read_to_string(root.join(file)).unwrap();
        let fixture: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            fixture["fixture_format"],
            "cortexweave.bp0.runner_observations.v1"
        );
        assert!(!text.contains("C:/"));
        assert!(!text.contains("C:\\"));
    }
}

#[tokio::test]
async fn both_runners_complete_the_same_native_consolidation_cycle() {
    let directory = tempdir().unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();

    for vitest in [true, false] {
        let label = if vitest { "vitest" } else { "unittest" };
        let root = directory.path().join(label);
        fs::create_dir_all(&root).unwrap();
        let workspace = service
            .register_workspace(root.to_string_lossy(), label)
            .await
            .unwrap();
        let session = service
            .start_session(&workspace.id, serde_json::json!({"runner": label}))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                format!("repair {label} failure"),
                serde_json::json!({}),
            )
            .await
            .unwrap();
        let episode = service
            .start_episode(EpisodeStartRequest {
                workspace_id: workspace.id.clone(),
                session_id: session.id.clone(),
                task_id: Some(task.id.clone()),
                episode_type: EpisodeType::Debugging,
                title: Some(format!("{label} evidence cycle")),
                created_by: EpisodeCreator::NativeHarness,
            })
            .await
            .unwrap();

        let namespace = if vitest {
            vec!["suite"]
        } else {
            vec!["tests.test_core", "StagnationTests"]
        };
        let mut first_failure = report(
            vitest,
            vec![
                case(TestCaseStatus::AssertionFailed, &namespace, "target case"),
                case(TestCaseStatus::Passed, &namespace, "control case"),
            ],
        );
        first_failure.run_id = format!("{label}-failure-1");
        let mut second_failure = first_failure.clone();
        second_failure.run_id = format!("{label}-failure-2");
        let mut terminal_pass = report(
            vitest,
            vec![
                case(TestCaseStatus::Passed, &namespace, "target case"),
                case(TestCaseStatus::Passed, &namespace, "control case"),
            ],
        );
        terminal_pass.run_id = format!("{label}-pass");

        let mut events = [
            event(&first_failure),
            tool_action(&workspace.id, &format!("{label}-edit-1")),
            event(&second_failure),
            tool_action(&workspace.id, &format!("{label}-edit-2")),
            event(&terminal_pass),
        ];
        for (index, item) in events.iter_mut().enumerate() {
            item.workspace_id = workspace.id.clone();
            item.session_id = Some(session.id.clone());
            item.task_id = Some(task.id.clone());
            if item.payload["contract"] == "cortexweave.test_run_result" {
                *item = service
                    .record_test_evidence(TestEvidenceRecordRequest {
                        workspace_id: workspace.id.clone(),
                        session_id: session.id.clone(),
                        task_id: Some(task.id.clone()),
                        request_key: format!("{label}-import-{index}"),
                        bundle: item.payload.clone(),
                    })
                    .await
                    .unwrap()
                    .event;
            } else {
                *item = service.record_event(item.clone()).await.unwrap();
            }
        }
        let associated = service
            .add_episode_events(EpisodeEventAssociationRequest {
                workspace_id: workspace.id.clone(),
                episode_id: episode.id.clone(),
                expected_version: episode.version,
                request_key: format!("{label}-associate"),
                event_ids: events.iter().map(|item| item.id.clone()).collect(),
            })
            .await
            .unwrap();
        let closed = service
            .close_episode(EpisodeTerminalRequest {
                workspace_id: workspace.id.clone(),
                episode_id: episode.id,
                expected_version: associated.version,
                request_key: format!("{label}-close"),
            })
            .await
            .unwrap();
        let request = ConsolidationRequest {
            workspace_id: workspace.id,
            episode_id: closed.id,
            expected_episode_version: closed.version,
        };
        let ConsolidationPreview::Proposal {
            proposal,
            disposition,
        } = service.preview_experience(&request).await.unwrap()
        else {
            panic!("{label} sequence did not produce a proposal")
        };
        assert_eq!(disposition, ProposalDisposition::Automatic);
        assert_eq!(
            proposal.record.attempts[0].result,
            AttemptResult::VerificationFailed
        );
        assert_eq!(
            proposal.record.attempts[1].result,
            AttemptResult::VerificationPassed
        );
        assert_eq!(
            proposal.record.experience.outcome,
            ExperienceOutcome::Success
        );
        assert_eq!(
            proposal.record.experience.verification.status,
            VerificationStatus::VerifiedPassed
        );
        assert!(
            proposal
                .record
                .experience
                .verification
                .observations
                .iter()
                .all(|observation| observation.kind == VerificationKind::TestRun)
        );

        let ConsolidationAcceptance::Accepted { record } = service
            .accept_experience(&ConsolidationAcceptanceRequest {
                request,
                expected_fingerprint: proposal.fingerprint.clone(),
                expected_proposal_hash: proposal.proposal_hash.clone(),
            })
            .await
            .unwrap()
        else {
            panic!("{label} automatic proposal was not accepted")
        };
        assert_eq!(record.experience.extractor_version, "2");
    }
}

#[tokio::test]
async fn capture_ingress_is_exactly_replayable_inspectable_and_not_bypassable() {
    let directory = tempdir().unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();
    let workspace = service
        .register_workspace(directory.path().to_string_lossy(), "capture-ingress")
        .await
        .unwrap();
    let session = service
        .start_session(&workspace.id, serde_json::json!({}))
        .await
        .unwrap();
    let task = service
        .start_task(
            &workspace.id,
            Some(session.id.clone()),
            "capture evidence",
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let mut captured = report(
        true,
        vec![case(TestCaseStatus::Passed, &["suite"], "target case")],
    );
    captured.run_id = "capture-exact-1".into();
    let mut captured_event = event(&captured);
    captured_event.workspace_id = workspace.id.clone();
    captured_event.session_id = Some(session.id.clone());
    captured_event.task_id = Some(task.id.clone());
    let request = TestEvidenceRecordRequest {
        workspace_id: workspace.id.clone(),
        session_id: session.id.clone(),
        task_id: Some(task.id.clone()),
        request_key: "capture-request-1".into(),
        bundle: captured_event.payload.clone(),
    };

    let first = service.record_test_evidence(request.clone()).await.unwrap();
    assert!(!first.replayed);
    assert_eq!(
        first.eligibility.status,
        TestRunEligibilityStatus::EligiblePassed
    );
    let replay = service.record_test_evidence(request.clone()).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.event, first.event);

    let inspection = service
        .inspect_event_evidence(&workspace.id, &first.event.id)
        .await
        .unwrap();
    assert_eq!(inspection.event, first.event);
    assert!(matches!(
        inspection.decoding,
        EvidenceDecodeResult::Decoded { .. }
    ));
    assert_eq!(
        inspection.test_run_eligibility.unwrap().status,
        TestRunEligibilityStatus::EligiblePassed
    );

    let mut changed_key = request.clone();
    changed_key.request_key = "capture-request-2".into();
    assert!(service.record_test_evidence(changed_key).await.is_err());

    let mut changed_content = request.clone();
    changed_content.bundle["environment_fingerprint"] = Value::String(DIGEST_B.into());
    assert!(service.record_test_evidence(changed_content).await.is_err());

    let mut changed_capture = request.clone();
    changed_capture.bundle["run_id"] = Value::String("capture-exact-2".into());
    assert!(service.record_test_evidence(changed_capture).await.is_err());

    assert!(service.record_event(captured_event.clone()).await.is_err());
    assert!(
        service
            .deliver_native(NativeDeliveryRequest {
                request_key: "generic-native-bypass".into(),
                operation: NativeOperation::RecordEvent {
                    event: captured_event,
                },
            })
            .await
            .is_err()
    );
    assert_eq!(
        service.recent_events(&workspace.id, 10).await.unwrap(),
        vec![first.event]
    );

    let capabilities = service.evidence_capabilities();
    assert!(
        capabilities
            .contracts
            .iter()
            .any(|(id, version)| id == "cortexweave.test_run_result" && *version == 1)
    );
    assert_eq!(capabilities.test_profiles.len(), 2);
    assert!(
        capabilities
            .unsupported_test_integrations
            .contains(&"pytest".to_owned())
    );
    assert!(capabilities.code_analyzer_capabilities_are_separate);
}

#[tokio::test]
async fn concurrent_capture_import_has_one_commit_and_one_exact_replay() {
    let directory = tempdir().unwrap();
    let database = directory.path().join("concurrent.sqlite");
    let service_a = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::open(&database).await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();
    let root = directory.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    let workspace = service_a
        .register_workspace(root.to_string_lossy(), "concurrent-capture")
        .await
        .unwrap();
    let session = service_a
        .start_session(&workspace.id, serde_json::json!({}))
        .await
        .unwrap();
    let task = service_a
        .start_task(
            &workspace.id,
            Some(session.id.clone()),
            "concurrent capture",
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let service_b = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::open(&database).await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();
    let mut captured = report(
        false,
        vec![case(
            TestCaseStatus::Passed,
            &["tests.test_core", "StagnationTests"],
            "test_stagnation_limit",
        )],
    );
    captured.run_id = "concurrent-run".into();
    let request = TestEvidenceRecordRequest {
        workspace_id: workspace.id.clone(),
        session_id: session.id,
        task_id: Some(task.id),
        request_key: "concurrent-request".into(),
        bundle: event(&captured).payload,
    };

    let (left, right) = tokio::join!(
        service_a.record_test_evidence(request.clone()),
        service_b.record_test_evidence(request)
    );
    let left = left.unwrap();
    let right = right.unwrap();
    assert_ne!(left.replayed, right.replayed);
    assert_eq!(left.event, right.event);
    assert_eq!(
        service_a.recent_events(&workspace.id, 10).await.unwrap(),
        vec![left.event]
    );
}

#[tokio::test]
async fn receipt_failure_and_owner_mismatch_leave_no_partial_event() {
    let directory = tempdir().unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();
    let workspace = service
        .register_workspace(directory.path().to_string_lossy(), "atomic-capture")
        .await
        .unwrap();
    let session = service
        .start_session(&workspace.id, serde_json::json!({}))
        .await
        .unwrap();
    let mut captured = report(
        true,
        vec![case(TestCaseStatus::Passed, &["suite"], "atomic case")],
    );
    captured.run_id = "atomic-run".into();
    let request = TestEvidenceRecordRequest {
        workspace_id: workspace.id.clone(),
        session_id: session.id.clone(),
        task_id: None,
        request_key: "atomic-request".into(),
        bundle: event(&captured).payload,
    };

    let mut wrong_owner = request.clone();
    wrong_owner.session_id = "missing-session".into();
    assert!(service.record_test_evidence(wrong_owner).await.is_err());

    sqlx::query(
        "CREATE TRIGGER fail_test_evidence_receipt BEFORE INSERT ON test_evidence_import_receipts BEGIN SELECT RAISE(ABORT, 'injected receipt failure'); END",
    )
    .execute(service.storage_handle().pool())
    .await
    .unwrap();
    assert!(service.record_test_evidence(request.clone()).await.is_err());
    assert!(
        service
            .recent_events(&workspace.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let event_frontier_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM historical_write_order WHERE workspace_id = ? AND entity_kind = 'event'",
    )
    .bind(&workspace.id)
    .fetch_one(service.storage_handle().pool())
    .await
    .unwrap();
    assert_eq!(event_frontier_count, 0);
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM test_evidence_import_receipts")
            .fetch_one(service.storage_handle().pool())
            .await
            .unwrap();
    assert_eq!(receipt_count, 0);

    sqlx::query("DROP TRIGGER fail_test_evidence_receipt")
        .execute(service.storage_handle().pool())
        .await
        .unwrap();
    let recovered = service.record_test_evidence(request).await.unwrap();
    assert!(!recovered.replayed);
    assert_eq!(
        service.recent_events(&workspace.id, 10).await.unwrap(),
        vec![recovered.event]
    );
}

#[tokio::test]
async fn populated_workspace_deregistration_is_stale_safe_atomic_and_replayable() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("preserved-workspace");
    fs::create_dir_all(root.join("src")).unwrap();
    let source = root.join("src/lib.rs");
    fs::write(&source, "pub fn preserved_marker() -> bool { true }\n").unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(ContractEmbeddingProvider),
    )
    .unwrap();
    assert!(
        service
            .preview_workspace_deregistration(WorkspaceSelector::Default)
            .await
            .is_err()
    );
    let workspace = service
        .register_workspace(root.to_string_lossy(), "deregister-populated")
        .await
        .unwrap();
    service.workspace_reindex(&workspace.id).await.unwrap();
    let session = service
        .start_session(&workspace.id, serde_json::json!({}))
        .await
        .unwrap();
    let task = service
        .start_task(
            &workspace.id,
            Some(session.id.clone()),
            "persist then deregister",
            serde_json::json!({}),
        )
        .await
        .unwrap();
    let mut memory = MemoryRecord::new(
        &workspace.id,
        MemoryKind::Observation,
        "bounded deregistration fixture",
    );
    memory.session_id = Some(session.id.clone());
    memory.task_id = Some(task.id.clone());
    service.record_memory(memory).await.unwrap();
    let episode = service
        .start_episode(EpisodeStartRequest {
            workspace_id: workspace.id.clone(),
            session_id: session.id.clone(),
            task_id: Some(task.id.clone()),
            episode_type: EpisodeType::Debugging,
            title: Some("deregistration fixture".into()),
            created_by: EpisodeCreator::NativeHarness,
        })
        .await
        .unwrap();
    let namespace = ["suite"];
    let mut failed = report(
        true,
        vec![
            case(TestCaseStatus::AssertionFailed, &namespace, "target"),
            case(TestCaseStatus::Passed, &namespace, "control"),
        ],
    );
    failed.run_id = "deregister-failed".into();
    let mut passed = report(
        true,
        vec![
            case(TestCaseStatus::Passed, &namespace, "target"),
            case(TestCaseStatus::Passed, &namespace, "control"),
        ],
    );
    passed.run_id = "deregister-passed".into();
    let failed_event = service
        .record_test_evidence(TestEvidenceRecordRequest {
            workspace_id: workspace.id.clone(),
            session_id: session.id.clone(),
            task_id: Some(task.id.clone()),
            request_key: "deregister-failed-import".into(),
            bundle: event(&failed).payload,
        })
        .await
        .unwrap()
        .event;
    let mut action = tool_action(&workspace.id, "deregister-edit");
    action.session_id = Some(session.id.clone());
    action.task_id = Some(task.id.clone());
    let action = service.record_event(action).await.unwrap();
    let passed_event = service
        .record_test_evidence(TestEvidenceRecordRequest {
            workspace_id: workspace.id.clone(),
            session_id: session.id.clone(),
            task_id: Some(task.id.clone()),
            request_key: "deregister-passed-import".into(),
            bundle: event(&passed).payload,
        })
        .await
        .unwrap()
        .event;
    let associated = service
        .add_episode_events(EpisodeEventAssociationRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id.clone(),
            expected_version: episode.version,
            request_key: "deregister-associate".into(),
            event_ids: vec![failed_event.id.clone(), action.id, passed_event.id.clone()],
        })
        .await
        .unwrap();
    let closed = service
        .close_episode(EpisodeTerminalRequest {
            workspace_id: workspace.id.clone(),
            episode_id: episode.id,
            expected_version: associated.version,
            request_key: "deregister-close".into(),
        })
        .await
        .unwrap();
    let consolidation = ConsolidationRequest {
        workspace_id: workspace.id.clone(),
        episode_id: closed.id,
        expected_episode_version: closed.version,
    };
    let ConsolidationPreview::Proposal { proposal, .. } =
        service.preview_experience(&consolidation).await.unwrap()
    else {
        panic!("fixture must consolidate")
    };
    let ConsolidationAcceptance::Accepted { record } = service
        .accept_experience(&ConsolidationAcceptanceRequest {
            request: consolidation,
            expected_fingerprint: proposal.fingerprint.clone(),
            expected_proposal_hash: proposal.proposal_hash.clone(),
        })
        .await
        .unwrap()
    else {
        panic!("fixture must be accepted")
    };
    let mut review_event = CortexEvent::new(
        &workspace.id,
        EventType::UserAcceptance,
        serde_json::json!({"review": "confirmed"}),
    );
    review_event.session_id = Some(session.id.clone());
    review_event.task_id = Some(task.id.clone());
    let review_event = service.record_event(review_event).await.unwrap();
    service
        .review_experience_assessment(ExperienceAssessmentReviewRequest {
            workspace_id: workspace.id.clone(),
            experience_id: record.experience.id.clone(),
            kind: ExperienceAssessmentKind::Confirmed,
            reviewed_by: "test-reviewer".into(),
            request_key: "deregister-review".into(),
            reason: "exercise populated cascade".into(),
            replacement_experience_id: None,
            evidence_event_ids: vec![review_event.id],
        })
        .await
        .unwrap();

    let stale_plan = service
        .preview_workspace_deregistration(WorkspaceSelector::Id(workspace.id.clone()))
        .await
        .unwrap();
    assert!(stale_plan.counts.documents > 0);
    assert!(stale_plan.counts.chunks > 0);
    assert!(stale_plan.counts.embeddings > 0);
    assert_eq!(stale_plan.counts.experiences, 1);
    assert_eq!(stale_plan.counts.assessments, 1);
    assert_eq!(stale_plan.counts.test_evidence_receipts, 2);
    let mut later_event = CortexEvent::new(
        &workspace.id,
        EventType::TaskUpdated,
        serde_json::json!({"after": "preview"}),
    );
    later_event.session_id = Some(session.id);
    later_event.task_id = Some(task.id);
    service.record_event(later_event).await.unwrap();
    assert_eq!(
        service
            .deregister_workspace(WorkspaceDeregistrationRequest {
                workspace_id: workspace.id.clone(),
                plan_id: stale_plan.plan_id,
                request_key: "stale-delete".into(),
            })
            .await
            .unwrap(),
        WorkspaceDeregistrationOutcome::StalePreview {
            reason: WorkspaceDeregistrationStaleReason::WorkspaceChanged
        }
    );

    let plan = service
        .preview_workspace_deregistration(WorkspaceSelector::Id(workspace.id.clone()))
        .await
        .unwrap();
    let request = WorkspaceDeregistrationRequest {
        workspace_id: workspace.id.clone(),
        plan_id: plan.plan_id,
        request_key: "delete-populated".into(),
    };
    sqlx::query(
        "CREATE TRIGGER fail_workspace_deregistration_receipt BEFORE INSERT ON workspace_deregistration_receipts BEGIN SELECT RAISE(ABORT, 'injected deregistration receipt failure'); END",
    )
    .execute(service.storage_handle().pool())
    .await
    .unwrap();
    assert!(service.deregister_workspace(request.clone()).await.is_err());
    assert!(
        service
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == workspace.id)
    );
    sqlx::query("DROP TRIGGER fail_workspace_deregistration_receipt")
        .execute(service.storage_handle().pool())
        .await
        .unwrap();

    let first = service.deregister_workspace(request.clone()).await.unwrap();
    let WorkspaceDeregistrationOutcome::Deregistered {
        receipt,
        replayed: false,
    } = first
    else {
        panic!("current plan must deregister")
    };
    assert_eq!(receipt.counts, plan.counts);
    assert!(
        !service
            .list_workspaces()
            .await
            .unwrap()
            .iter()
            .any(|candidate| candidate.id == workspace.id)
    );
    assert!(source.exists());
    assert!(
        fs::read_to_string(&source)
            .unwrap()
            .contains("preserved_marker")
    );
    assert_eq!(
        service.deregister_workspace(request).await.unwrap(),
        WorkspaceDeregistrationOutcome::Deregistered {
            receipt: receipt.clone(),
            replayed: true
        }
    );

    let fresh = service
        .register_workspace(root.to_string_lossy(), "fresh-registration")
        .await
        .unwrap();
    assert_ne!(fresh.id, workspace.id);
    assert!(
        service
            .recent_events(&fresh.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        service
            .get_experience(&fresh.id, &record.experience.id)
            .await
            .unwrap()
            .is_none()
    );
}
