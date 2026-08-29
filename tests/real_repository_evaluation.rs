use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result,
    domain::{ContextRequest, GraphEdgeType, StructuralReadOptions},
    embedding::EmbeddingProvider,
    storage::SqliteStorage,
};
use serde::Serialize;
use tempfile::tempdir;

struct DeterministicEmbeddings;

#[async_trait]
impl EmbeddingProvider for DeterministicEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| vec![text.len() as f32, 1.0, 0.0, 0.0])
            .collect())
    }

    fn model_name(&self) -> &str {
        "real-repository-evaluation"
    }

    fn dimension(&self) -> Option<usize> {
        Some(4)
    }
}

#[derive(Serialize)]
struct EvaluationReport {
    root: String,
    files_seen: usize,
    graph_nodes: usize,
    graph_edges: usize,
    unresolved_relationships: usize,
    languages: Vec<String>,
    symbol_lookup_node_id: String,
    callers: usize,
    implementations: usize,
    tests: usize,
    dependencies: usize,
    impacts: usize,
    acceptance_edit_path: String,
    post_edit_content_revision: i64,
    post_edit_graph_revision: i64,
    graph_aware_context_items: usize,
    indexing_ms: f64,
}

#[tokio::test]
#[ignore = "runs a user-selected medium mixed-language repository through a temporary SQLite database"]
async fn evaluates_selected_mixed_language_repository() {
    let root = std::env::var_os("CORTEXWEAVE_EVALUATION_ROOT")
        .map(PathBuf::from)
        .expect("set CORTEXWEAVE_EVALUATION_ROOT to a repository root before running this test");
    assert!(root.is_dir(), "evaluation root must be a directory");
    let directory = tempdir().unwrap();
    let evaluation_root = directory.path().join("repository");
    copy_evaluation_repository(&root, &evaluation_root);
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::open(directory.path().join("evaluation.sqlite"))
            .await
            .unwrap(),
        Arc::new(DeterministicEmbeddings),
    )
    .unwrap();
    let workspace = service
        .register_workspace(
            evaluation_root.to_string_lossy(),
            "real-repository-evaluation",
        )
        .await
        .unwrap();

    let started = Instant::now();
    let indexed = service.workspace_reindex(&workspace.id).await.unwrap();
    let status = service.workspace_graph_status(&workspace.id).await.unwrap();
    assert!(status.is_current);
    assert!(
        status.nodes > 0,
        "evaluation repository produced no graph nodes"
    );
    assert!(
        status.edges > 0,
        "evaluation repository produced no graph edges"
    );
    assert!(
        status.languages.len() >= 2,
        "evaluation repository is not mixed-language"
    );
    let nodes = service.storage().graph_nodes(&workspace.id).await.unwrap();
    let by_id: BTreeMap<_, _> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    let name_counts: BTreeMap<_, _> = nodes.iter().fold(BTreeMap::new(), |mut counts, node| {
        *counts.entry(node.name.as_str()).or_insert(0usize) += 1;
        counts
    });
    let edges = service.storage().graph_edges(&workspace.id).await.unwrap();
    let sample_node = nodes
        .iter()
        .find(|node| node.document_id.is_some() && node.name != root.to_string_lossy())
        .expect("evaluation repository produced no usable graph node");
    let lookup = service
        .graph_find_symbol(
            &workspace.id,
            &sample_node.name,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(lookup.seeds.iter().any(|node| node.id == sample_node.id));

    let call = edges
        .iter()
        .find(|edge| {
            edge.edge_type == GraphEdgeType::Calls
                && by_id
                    .get(edge.to_node.as_str())
                    .is_some_and(|node| name_counts.get(node.name.as_str()) == Some(&1))
        })
        .expect(
            "evaluation repository needs a resolved direct call edge with a unique target name",
        );
    let callers = service
        .graph_callers(
            &workspace.id,
            &call.to_node,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(callers.nodes.iter().any(|node| node.id == call.from_node));

    let implementation = edges
        .iter()
        .find(|edge| {
            matches!(
                edge.edge_type,
                GraphEdgeType::Implements | GraphEdgeType::Extends | GraphEdgeType::Overrides
            )
        })
        .expect("evaluation repository needs a resolved implementation or inheritance edge");
    let implementations = service
        .graph_implementations(
            &workspace.id,
            &implementation.to_node,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        implementations
            .nodes
            .iter()
            .any(|node| node.id == implementation.from_node)
    );

    let test = edges
        .iter()
        .find(|edge| edge.edge_type == GraphEdgeType::Tests)
        .expect("evaluation repository needs a resolved likely test direct-call association");
    let tests = service
        .graph_tests(
            &workspace.id,
            &test.to_node,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(tests.nodes.iter().any(|node| node.id == test.from_node));

    let dependency = edges
        .iter()
        .find(|edge| {
            matches!(
                edge.edge_type,
                GraphEdgeType::Imports | GraphEdgeType::DependsOn
            )
        })
        .expect("evaluation repository needs a resolved local module dependency edge");
    let dependencies = service
        .graph_dependencies(
            &workspace.id,
            &dependency.from_node,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        dependencies
            .nodes
            .iter()
            .any(|node| node.id == dependency.to_node)
    );

    let impact_target = by_id
        .get(call.to_node.as_str())
        .expect("call target must remain in the workspace graph");
    let documents: BTreeMap<_, _> = service
        .storage()
        .list_documents(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .map(|document| (document.id.clone(), document))
        .collect();
    let changed_document = impact_target
        .document_id
        .as_ref()
        .and_then(|document_id| documents.get(document_id))
        .expect("resolved call target must originate from an indexed document");
    let impact = service
        .graph_impact_symbol(
            &workspace.id,
            &impact_target.name,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        impact
            .impacts
            .iter()
            .any(|item| item.node.id == call.from_node)
    );

    let mut context_request = ContextRequest::new(&workspace.id);
    context_request.query = Some(format!("what calls {}", impact_target.name));
    context_request.include_documents = false;
    context_request.include_memories = false;
    context_request.include_events = false;
    context_request.include_explanation = true;
    context_request.token_budget = 65_536;
    let retrieval = service
        .hybrid_search(&workspace.id, context_request.query.as_deref().unwrap(), 64)
        .await
        .unwrap();
    let retrieval_structural_items = retrieval
        .iter()
        .filter(|item| !item.structural_evidence.is_empty())
        .count();
    let target_chunks = service
        .storage()
        .list_chunks(impact_target.document_id.as_deref().unwrap())
        .await
        .unwrap()
        .len();
    assert!(
        retrieval_structural_items > 0,
        "graph-aware retrieval did not retain structural evidence for target={} callers={} target_chunks={} retrieval_results={}",
        impact_target.name,
        callers.nodes.len(),
        target_chunks,
        retrieval.len(),
    );
    let pool = service
        .build_candidate_pool(context_request.clone())
        .await
        .unwrap();
    let pool_structural_items = pool
        .candidates
        .iter()
        .filter(|item| !item.structural_evidence.is_empty())
        .count();
    assert!(
        pool_structural_items > 0,
        "graph-aware context candidate pool discarded structural evidence"
    );
    let context = service.semantic_context(context_request).await.unwrap();
    let graph_aware_context_items = context
        .items
        .iter()
        .filter(|item| !item.structural_evidence.is_empty())
        .count();
    assert!(
        graph_aware_context_items > 0,
        "graph-aware context did not retain structural evidence after retrieval={retrieval_structural_items}, pool={pool_structural_items}"
    );

    let unrelated_node = nodes
        .iter()
        .find(|node| node.document_id.as_deref() != impact_target.document_id.as_deref())
        .expect("evaluation repository needs an unrelated graph node")
        .id
        .clone();
    let before_revision = status
        .revision
        .as_ref()
        .expect("indexed graph must have a revision");
    let changed_path = evaluation_root.join(&changed_document.relative_path);
    let source = fs::read_to_string(&changed_path).unwrap();
    fs::write(
        &changed_path,
        format!(
            "{source}\n{} cortexweave acceptance edit\n",
            comment_prefix(&changed_document.language)
        ),
    )
    .unwrap();
    let post_edit = service.workspace_reindex(&workspace.id).await.unwrap();
    assert!(post_edit.files_updated >= 1);
    let post_status = service.workspace_graph_status(&workspace.id).await.unwrap();
    assert!(post_status.is_current);
    let post_revision = post_status
        .revision
        .as_ref()
        .expect("reconciled graph must have a revision");
    assert!(post_revision.content_revision > before_revision.content_revision);
    assert_eq!(
        post_revision.graph_content_revision, post_revision.content_revision,
        "the graph projection must be refreshed to the acceptance edit revision"
    );
    let changed_after = service
        .storage()
        .find_document(&workspace.id, &changed_document.relative_path)
        .await
        .unwrap()
        .expect("acceptance-edit document must remain indexed");
    assert!(changed_after.content_revision > changed_document.content_revision);
    let post_nodes = service.storage().graph_nodes(&workspace.id).await.unwrap();
    assert!(
        post_nodes.iter().any(|node| node.id == unrelated_node),
        "an unrelated graph fact must be preserved after the acceptance edit"
    );
    let fresh_callers = service
        .graph_callers(
            &workspace.id,
            &call.to_node,
            &StructuralReadOptions::default(),
        )
        .await
        .unwrap();
    assert!(
        fresh_callers
            .nodes
            .iter()
            .any(|node| node.id == call.from_node),
        "post-edit structural answers must remain fresh for unchanged relationships"
    );

    println!(
        "{}",
        serde_json::to_string_pretty(&EvaluationReport {
            root: root.to_string_lossy().into_owned(),
            files_seen: indexed.files_seen,
            graph_nodes: status.nodes,
            graph_edges: status.edges,
            unresolved_relationships: status.unresolved_relationships,
            languages: status
                .languages
                .into_iter()
                .map(|language| language.language)
                .collect(),
            symbol_lookup_node_id: sample_node.id.clone(),
            callers: callers.nodes.len(),
            implementations: implementations.nodes.len(),
            tests: tests.nodes.len(),
            dependencies: dependencies.nodes.len(),
            impacts: impact.impacts.len(),
            acceptance_edit_path: changed_document.relative_path.clone(),
            post_edit_content_revision: post_revision.content_revision,
            post_edit_graph_revision: post_revision.graph_content_revision,
            graph_aware_context_items,
            indexing_ms: started.elapsed().as_secs_f64() * 1_000.0,
        })
        .unwrap()
    );
}

fn copy_evaluation_repository(source: &Path, destination: &Path) {
    for entry in ignore::WalkBuilder::new(source)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .parents(true)
        .filter_entry(|entry| entry.file_name().to_str() != Some(".git"))
        .build()
    {
        let entry = entry.unwrap();
        let path = entry.path();
        let relative_path = path.strip_prefix(source).unwrap();
        let destination_path = destination.join(relative_path);
        if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_dir())
        {
            fs::create_dir_all(destination_path).unwrap();
        } else if entry
            .file_type()
            .is_some_and(|file_type| file_type.is_file())
        {
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::copy(path, destination_path).unwrap();
        }
    }
}

fn comment_prefix(language: &str) -> &'static str {
    match language {
        "python" => "#",
        _ => "//",
    }
}
