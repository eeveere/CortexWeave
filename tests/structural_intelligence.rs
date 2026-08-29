use std::{collections::BTreeSet, path::Path, sync::Arc};

use async_trait::async_trait;
use chrono::Utc;
use cortexweave::{
    AppConfig, CortexError, CortexWeaveService, Result,
    domain::{
        ContextRequest, ContextSelectionReason, GraphNode, GraphState, StructuralReadOptions,
        Workspace,
    },
    embedding::EmbeddingProvider,
    retrieval::RetrievalService,
    storage::SqliteStorage,
};
use tempfile::tempdir;

struct FlatEmbeddings;

#[async_trait]
impl EmbeddingProvider for FlatEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 1.0, 1.0, 1.0]).collect())
    }

    fn model_name(&self) -> &str {
        "structural-intelligence-test"
    }

    fn dimension(&self) -> Option<usize> {
        Some(4)
    }
}

async fn fixture() -> (tempfile::TempDir, CortexWeaveService, Workspace) {
    let directory = tempdir().unwrap();
    let source = r#"
export interface Interface { execute(): void; }

export class ImplA implements Interface { execute(): void {} }

export class ImplB implements Interface { execute(): void {} }

export function service() { ImplA(); }

test('test_a', () => { ImplA(); });
test('test_b', () => { ImplB(); });
"#;
    std::fs::write(directory.path().join("architecture.test.ts"), source).unwrap();

    let mut config = AppConfig::default();
    config.retrieval.semantic_weight = 0.2;
    config.retrieval.lexical_weight = 0.1;
    config.retrieval.structural.weight = 0.7;
    config.retrieval.structural.max_depth = 3;
    config.retrieval.structural.candidate_limit = 64;
    let storage = SqliteStorage::in_memory().await.unwrap();
    let workspace = Workspace::new(
        directory.path().to_string_lossy(),
        "structural-intelligence",
    );
    storage.insert_workspace(&workspace).await.unwrap();
    let embeddings: Arc<dyn EmbeddingProvider> = Arc::new(FlatEmbeddings);
    let service =
        CortexWeaveService::from_parts_with_embeddings(config, storage, Arc::clone(&embeddings))
            .unwrap();
    service
        .indexing()
        .reconcile_file(
            &workspace,
            &directory.path().join("architecture.test.ts"),
            Path::new("architecture.test.ts"),
        )
        .await
        .unwrap();
    (directory, service, workspace)
}

async fn node_named(service: &CortexWeaveService, workspace: &Workspace, name: &str) -> GraphNode {
    service
        .storage()
        .graph_nodes(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("missing graph node {name}"))
}

async fn evidence_names(
    service: &CortexWeaveService,
    workspace: &Workspace,
    query: &str,
) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for result in service
        .retrieval()
        .hybrid_search(&workspace.id, query, 20)
        .await
        .unwrap()
    {
        for evidence in result.structural_evidence {
            assert_eq!(evidence.snapshot.graph_state, GraphState::Current);
            assert!(!evidence.path.edges.is_empty());
            names.insert(
                service
                    .storage()
                    .graph_node_by_id(&workspace.id, &evidence.node_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .name,
            );
        }
    }
    names
}

#[tokio::test]
async fn structural_service_answers_bounded_current_graph_queries_and_impact_paths() {
    let (_directory, service, workspace) = fixture().await;
    let options = StructuralReadOptions {
        max_nodes: 50,
        max_edges: 100,
        max_depth: 3,
        ..StructuralReadOptions::default()
    };
    let interface = node_named(&service, &workspace, "Interface").await;
    let impl_a = node_named(&service, &workspace, "ImplA").await;

    let implementations = service
        .structural()
        .implementations(&workspace.id, &interface.id, &options)
        .await
        .unwrap();
    assert_eq!(
        implementations
            .nodes
            .iter()
            .map(|node| node.name.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["ImplA", "ImplB"])
    );
    assert!(
        implementations
            .paths
            .iter()
            .all(|path| path.distance() == 1)
    );

    let callers = service
        .structural()
        .callers(&workspace.id, &impl_a.id, &options)
        .await
        .unwrap();
    let caller_names: BTreeSet<_> = callers
        .nodes
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    assert!(caller_names.contains("service"));
    assert!(caller_names.contains("test_a"));

    let neighbors = service
        .structural()
        .neighbors(&workspace.id, &interface.id, &options)
        .await
        .unwrap();
    assert_eq!(neighbors.limits.max_depth, 1);
    assert!(neighbors.paths.iter().all(|path| path.distance() == 1));

    let tests = service
        .structural()
        .tests(&workspace.id, &impl_a.id, &options)
        .await
        .unwrap();
    assert_eq!(tests.nodes.len(), 1);
    assert_eq!(tests.nodes[0].name, "test_a");

    let report = service
        .structural()
        .impact_from_symbol(&workspace.id, "Interface", &options)
        .await
        .unwrap();
    let impacted: BTreeSet<_> = report
        .impacts
        .iter()
        .map(|impact| impact.node.name.as_str())
        .collect();
    for expected in ["ImplA", "ImplB", "service", "test_a", "test_b"] {
        assert!(impacted.contains(expected), "missing impact {expected}");
    }
    assert!(report.impacts.iter().all(|impact| {
        impact.confidence > 0.0
            && impact.path.node_ids.first() == Some(&interface.id)
            && impact.path.node_ids.last() == Some(&impact.node.id)
    }));

    let file_report = service
        .structural()
        .impact_from_path(&workspace.id, "architecture.test.ts", &options)
        .await
        .unwrap();
    assert_eq!(file_report.seeds.len(), 1);
    assert_eq!(file_report.seeds[0].name, "architecture.test.ts");
    assert!(
        file_report
            .impacts
            .iter()
            .any(|impact| impact.node.name == "service")
    );
    let normalized_file_report = service
        .structural()
        .impact_from_path(&workspace.id, "./architecture.test.ts", &options)
        .await
        .unwrap();
    assert_eq!(normalized_file_report.seeds.len(), 1);

    let bounded = service
        .structural()
        .impact_from_symbol(
            &workspace.id,
            "Interface",
            &StructuralReadOptions {
                max_nodes: 1,
                max_edges: 1,
                max_depth: 1,
                ..StructuralReadOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(bounded.impacts.len(), 1);
    assert!(bounded.truncated);

    let seed_limit = service
        .structural()
        .traverse(
            &workspace.id,
            &[interface.id.clone(), impl_a.id.clone()],
            cortexweave::domain::StructuralDirection::Both,
            &[],
            &StructuralReadOptions {
                max_nodes: 1,
                ..StructuralReadOptions::default()
            },
        )
        .await;
    assert!(matches!(seed_limit, Err(CortexError::Analysis(_))));
    let missing_seed = service
        .structural()
        .traverse(
            &workspace.id,
            &["missing-node".into()],
            cortexweave::domain::StructuralDirection::Both,
            &[],
            &options,
        )
        .await;
    assert!(matches!(missing_seed, Err(CortexError::NotFound(_))));
}

#[tokio::test]
async fn structural_scoring_materially_adds_relationship_evidence_for_breakpoint_questions() {
    let (_directory, service, workspace) = fixture().await;
    let baseline =
        RetrievalService::new(service.storage_handle(), Arc::new(FlatEmbeddings), 0.7, 0.3)
            .unwrap();
    let cases = [
        ("what calls ImplA?", ["service"].as_slice()),
        (
            "where is Interface implemented?",
            ["ImplA", "ImplB"].as_slice(),
        ),
        ("which tests cover ImplA?", ["test_a"].as_slice()),
        (
            "what could break if Interface changes?",
            ["ImplA", "ImplB", "service", "test_a", "test_b"].as_slice(),
        ),
    ];
    for (query, expected) in cases {
        let baseline_results = baseline
            .hybrid_search(&workspace.id, query, 20)
            .await
            .unwrap();
        assert!(
            baseline_results
                .iter()
                .all(|result| result.structural_evidence.is_empty())
        );
        let names = evidence_names(&service, &workspace, query).await;
        for expected in expected {
            assert!(
                names.contains(*expected),
                "{query}: missing {expected} in {names:?}"
            );
        }
    }
}

#[tokio::test]
async fn graph_aware_context_changes_relation_mix_by_question_and_rejects_stale_by_default() {
    let (_directory, service, workspace) = fixture().await;
    let mut signatures = Vec::new();
    for query in [
        "How Interface works?",
        "What uses Interface?",
        "What changes if Interface changes?",
    ] {
        let mut request = ContextRequest::new(&workspace.id);
        request.query = Some(query.into());
        request.include_documents = false;
        request.include_memories = false;
        request.include_events = false;
        request.include_explanation = true;
        let pool = service.build_candidate_pool(request).await.unwrap();
        signatures.push(
            pool.candidates
                .iter()
                .flat_map(|candidate| candidate.reasons.iter())
                .filter(|reason| {
                    matches!(
                        reason,
                        ContextSelectionReason::CallerOfRelevantSymbol
                            | ContextSelectionReason::ReferenceToRelevantSymbol
                            | ContextSelectionReason::ImplementationOfRelevantSymbol
                            | ContextSelectionReason::ImpactedByRelevantSymbol
                    )
                })
                .map(|reason| format!("{reason:?}"))
                .collect::<BTreeSet<_>>(),
        );
    }
    assert!(signatures[0].contains("ImplementationOfRelevantSymbol"));
    assert!(signatures[1].contains("ReferenceToRelevantSymbol"));
    assert!(signatures[2].contains("ImpactedByRelevantSymbol"));
    assert_ne!(signatures[0], signatures[1]);
    assert_ne!(signatures[1], signatures[2]);

    let mut impact_request = ContextRequest::new(&workspace.id);
    impact_request.query = Some("What changes if Interface changes?".into());
    impact_request.include_documents = false;
    impact_request.include_memories = false;
    impact_request.include_events = false;
    let packet = service.semantic_context(impact_request).await.unwrap();
    let packet_evidence: Vec<_> = packet
        .items
        .iter()
        .flat_map(|item| item.structural_evidence.iter())
        .collect();
    assert!(!packet_evidence.is_empty());
    assert!(packet_evidence.iter().all(|evidence| {
        evidence.snapshot.graph_state == GraphState::Current
            && !evidence.limits.allow_stale
            && evidence.limits.max_depth == 3
    }));

    let revision = service
        .storage()
        .workspace_graph_revision(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    service
        .storage()
        .mark_graph_error(
            &workspace.id,
            revision.content_revision,
            "simulated stale graph",
            Utc::now(),
        )
        .await
        .unwrap();
    let current_only = service
        .structural()
        .find_symbol(
            &workspace.id,
            "Interface",
            &StructuralReadOptions::default(),
        )
        .await;
    assert!(matches!(current_only, Err(CortexError::Analysis(_))));
    let degraded = service
        .retrieval()
        .hybrid_search(&workspace.id, "what calls ImplA?", 20)
        .await
        .unwrap();
    assert!(
        degraded
            .iter()
            .all(|result| result.structural_evidence.is_empty())
    );
    let stale = service
        .structural()
        .find_symbol(
            &workspace.id,
            "Interface",
            &StructuralReadOptions {
                allow_stale: true,
                ..StructuralReadOptions::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(stale.snapshot.graph_state, GraphState::Error);
}
