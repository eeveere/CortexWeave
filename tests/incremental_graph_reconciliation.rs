use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use cortexweave::{
    Result,
    config::{GenericChunkConfig, LanguageConfig},
    domain::{
        AnalysisResult, AnalyzedRelationship, AnalyzerCapabilities, GraphEdge, GraphEdgeType,
        GraphNode, GraphState, RelationshipTarget, Workspace,
    },
    embedding::EmbeddingProvider,
    indexing::IndexingService,
    parsing::{AnalyzerRegistry, GenericAnalyzer, LanguageAnalyzer, languages::RustAnalyzer},
    storage::SqliteStorage,
};
use tempfile::tempdir;

struct CountingEmbeddingProvider {
    embedded_texts: AtomicUsize,
}

impl CountingEmbeddingProvider {
    fn new() -> Self {
        Self {
            embedded_texts: AtomicUsize::new(0),
        }
    }

    fn embedded_texts(&self) -> usize {
        self.embedded_texts.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl EmbeddingProvider for CountingEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embedded_texts
            .fetch_add(texts.len(), Ordering::Relaxed);
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0; 8];
                for (index, byte) in text.bytes().enumerate() {
                    vector[index % 8] += f32::from(byte) / 255.0;
                }
                vector
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "graph-reconciliation-test"
    }

    fn dimension(&self) -> Option<usize> {
        Some(8)
    }
}

fn analyzers() -> Arc<AnalyzerRegistry> {
    Arc::new(AnalyzerRegistry::configured(
        &LanguageConfig::default(),
        &GenericChunkConfig::default(),
    ))
}

fn indexer(
    storage: Arc<SqliteStorage>,
    embeddings: Arc<CountingEmbeddingProvider>,
    analyzers: Arc<AnalyzerRegistry>,
) -> IndexingService {
    IndexingService::new(storage, embeddings, analyzers, 1, 1024 * 1024)
}

async fn reconcile(
    indexer: &IndexingService,
    workspace: &Workspace,
    root: &Path,
    relative_path: &str,
) {
    indexer
        .reconcile_file(
            workspace,
            &root.join(relative_path),
            Path::new(relative_path),
        )
        .await
        .unwrap();
}

async fn assert_current(storage: &SqliteStorage, workspace: &Workspace) {
    let revision = storage
        .workspace_graph_revision(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert!(revision.is_current(), "revision was {revision:#?}");
}

async fn node_named(storage: &SqliteStorage, workspace: &Workspace, name: &str) -> GraphNode {
    storage
        .graph_nodes(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .find(|node| node.name == name)
        .unwrap_or_else(|| panic!("missing graph node {name}"))
}

async fn call_from(
    storage: &SqliteStorage,
    workspace: &Workspace,
    source_document_id: &str,
) -> Option<GraphEdge> {
    storage
        .graph_edges(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .find(|edge| {
            edge.edge_type == GraphEdgeType::Calls
                && edge.source_document_id.as_deref() == Some(source_document_id)
        })
}

#[tokio::test]
async fn analyzer_owned_module_candidates_resolve_local_files_without_core_language_rules() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/shared.ts"), "export function shared() {}\n").unwrap();
    std::fs::write(
        root.join("src/service.ts"),
        "import { shared } from './shared';\nexport function service() { shared(); }\n",
    )
    .unwrap();
    let workspace = Workspace::new(root.to_string_lossy(), "local-module-resolution");
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    storage.insert_workspace(&workspace).await.unwrap();
    let embeddings = Arc::new(CountingEmbeddingProvider::new());
    let indexer = indexer(Arc::clone(&storage), embeddings, analyzers());

    reconcile(&indexer, &workspace, root, "src/shared.ts").await;
    reconcile(&indexer, &workspace, root, "src/service.ts").await;
    assert_current(&storage, &workspace).await;

    let service_document = storage
        .find_document(&workspace.id, "src/service.ts")
        .await
        .unwrap()
        .unwrap();
    let edge = storage
        .graph_edges(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .find(|edge| {
            edge.edge_type == GraphEdgeType::DependsOn
                && edge.source_document_id.as_deref() == Some(&service_document.id)
        })
        .expect("local TypeScript dependency should resolve to the imported file");
    let target = storage
        .graph_node_by_id(&workspace.id, &edge.to_node)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(target.stable_key, "file:src/shared.ts");
    assert_eq!(edge.metadata["resolution_basis"], "alias");
}

#[tokio::test]
async fn dependent_edges_reconcile_incrementally_across_edit_ambiguity_rename_and_delete() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("a.rs"), "pub fn alpha() { beta(); }\n").unwrap();
    std::fs::write(root.join("b.rs"), "pub fn beta() -> i32 { 1 }\n").unwrap();
    std::fs::write(root.join("c.rs"), "pub fn unrelated() -> i32 { 7 }\n").unwrap();
    let workspace = Workspace::new(root.to_string_lossy(), "incremental-graph");
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    storage.insert_workspace(&workspace).await.unwrap();
    let embeddings = Arc::new(CountingEmbeddingProvider::new());
    let indexer = indexer(Arc::clone(&storage), Arc::clone(&embeddings), analyzers());

    for path in ["a.rs", "b.rs", "c.rs"] {
        reconcile(&indexer, &workspace, root, path).await;
        assert_current(&storage, &workspace).await;
    }
    let a = storage
        .find_document(&workspace.id, "a.rs")
        .await
        .unwrap()
        .unwrap();
    let c_before = node_named(&storage, &workspace, "unrelated").await;
    let initial_edge = call_from(&storage, &workspace, &a.id).await.unwrap();
    let nodes_before_no_op = storage.graph_nodes(&workspace.id).await.unwrap();
    let edges_before_no_op = storage.graph_edges(&workspace.id).await.unwrap();
    let embeddings_before_no_op = embeddings.embedded_texts();
    let no_op = indexer
        .reconcile_file(&workspace, &root.join("b.rs"), Path::new("b.rs"))
        .await
        .unwrap();
    assert_eq!(
        no_op.status,
        cortexweave::indexing::ReconcileStatus::Unchanged
    );
    assert_eq!(
        storage.graph_nodes(&workspace.id).await.unwrap(),
        nodes_before_no_op
    );
    assert_eq!(
        storage.graph_edges(&workspace.id).await.unwrap(),
        edges_before_no_op
    );
    assert_eq!(embeddings.embedded_texts(), embeddings_before_no_op);
    assert_eq!(
        storage
            .graph_node_by_id(&workspace.id, &initial_edge.to_node)
            .await
            .unwrap()
            .unwrap()
            .name,
        "beta"
    );

    std::fs::write(root.join("b.rs"), "pub fn beta() -> i32 { 2 }\n").unwrap();
    reconcile(&indexer, &workspace, root, "b.rs").await;
    assert_current(&storage, &workspace).await;
    let body_edit_edge = call_from(&storage, &workspace, &a.id).await.unwrap();
    assert_eq!(body_edit_edge.id, initial_edge.id);
    assert_eq!(
        node_named(&storage, &workspace, "unrelated").await,
        c_before
    );

    std::fs::write(root.join("d.rs"), "pub fn beta() -> i32 { 9 }\n").unwrap();
    reconcile(&indexer, &workspace, root, "d.rs").await;
    assert_current(&storage, &workspace).await;
    assert!(call_from(&storage, &workspace, &a.id).await.is_none());
    let ambiguous = storage
        .unresolved_relationships(&workspace.id)
        .await
        .unwrap()
        .into_iter()
        .find(|relationship| {
            relationship.edge_type == GraphEdgeType::Calls
                && relationship.source_document_id == a.id
        })
        .unwrap();
    assert_eq!(ambiguous.metadata["resolution_state"], "ambiguous");

    std::fs::remove_file(root.join("d.rs")).unwrap();
    reconcile(&indexer, &workspace, root, "d.rs").await;
    assert_current(&storage, &workspace).await;
    assert!(call_from(&storage, &workspace, &a.id).await.is_some());

    std::fs::write(root.join("b.rs"), "pub fn gamma() -> i32 { 2 }\n").unwrap();
    reconcile(&indexer, &workspace, root, "b.rs").await;
    assert_current(&storage, &workspace).await;
    assert!(call_from(&storage, &workspace, &a.id).await.is_none());
    assert!(
        storage
            .graph_nodes(&workspace.id)
            .await
            .unwrap()
            .iter()
            .all(|node| node.name != "beta")
    );

    std::fs::write(root.join("b.rs"), "pub fn beta() -> i32 { 3 }\n").unwrap();
    reconcile(&indexer, &workspace, root, "b.rs").await;
    assert_current(&storage, &workspace).await;
    assert!(call_from(&storage, &workspace, &a.id).await.is_some());

    std::fs::remove_file(root.join("b.rs")).unwrap();
    reconcile(&indexer, &workspace, root, "b.rs").await;
    assert_current(&storage, &workspace).await;
    assert!(call_from(&storage, &workspace, &a.id).await.is_none());
    assert_eq!(
        node_named(&storage, &workspace, "unrelated").await,
        c_before
    );
}

struct MalformedRustAnalyzer;

impl LanguageAnalyzer for MalformedRustAnalyzer {
    fn language_id(&self) -> &'static str {
        RustAnalyzer.language_id()
    }

    fn analyzer_id(&self) -> &'static str {
        RustAnalyzer.analyzer_id()
    }

    fn analyzer_version(&self) -> String {
        RustAnalyzer.analyzer_version()
    }

    fn structure_version(&self) -> String {
        RustAnalyzer.structure_version()
    }

    fn extensions(&self) -> &'static [&'static str] {
        RustAnalyzer.extensions()
    }

    fn capabilities(&self) -> AnalyzerCapabilities {
        RustAnalyzer.capabilities()
    }

    fn analyze(&self, path: &Path, source: &str) -> Result<AnalysisResult> {
        let mut result = RustAnalyzer.analyze(path, source)?;
        result.relationships.push(AnalyzedRelationship {
            relationship_key: "malformed:missing-source".into(),
            from_key: "no-such-symbol".into(),
            target: RelationshipTarget::QualifiedSymbol("beta".into()),
            relationship: GraphEdgeType::Calls,
            confidence: 1.0,
            start_byte: None,
            end_byte: None,
            start_line: None,
            end_line: None,
            metadata: serde_json::json!({}),
        });
        Ok(result)
    }
}

struct BumpedStructureRustAnalyzer;

impl LanguageAnalyzer for BumpedStructureRustAnalyzer {
    fn language_id(&self) -> &'static str {
        RustAnalyzer.language_id()
    }

    fn analyzer_id(&self) -> &'static str {
        RustAnalyzer.analyzer_id()
    }

    fn analyzer_version(&self) -> String {
        RustAnalyzer.analyzer_version()
    }

    fn structure_version(&self) -> String {
        "rust-structure:test-bump".into()
    }

    fn extensions(&self) -> &'static [&'static str] {
        RustAnalyzer.extensions()
    }

    fn capabilities(&self) -> AnalyzerCapabilities {
        RustAnalyzer.capabilities()
    }

    fn analyze(&self, path: &Path, source: &str) -> Result<AnalysisResult> {
        RustAnalyzer.analyze(path, source)
    }
}

#[tokio::test]
async fn failed_graph_update_preserves_graph_and_retry_reuses_embeddings() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("a.rs"), "pub fn alpha() { beta(); }\n").unwrap();
    std::fs::write(root.join("b.rs"), "pub fn beta() -> i32 { 1 }\n").unwrap();
    let workspace = Workspace::new(root.to_string_lossy(), "graph-rollback");
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    storage.insert_workspace(&workspace).await.unwrap();
    let embeddings = Arc::new(CountingEmbeddingProvider::new());
    let healthy = indexer(Arc::clone(&storage), Arc::clone(&embeddings), analyzers());
    reconcile(&healthy, &workspace, root, "a.rs").await;
    reconcile(&healthy, &workspace, root, "b.rs").await;
    let a = storage
        .find_document(&workspace.id, "a.rs")
        .await
        .unwrap()
        .unwrap();
    let edge_before = call_from(&storage, &workspace, &a.id).await.unwrap();
    let a_chunk = storage.list_chunks(&a.id).await.unwrap().remove(0);
    let a_embedding = storage.get_embedding(&a_chunk.id).await.unwrap().unwrap();

    std::fs::write(root.join("b.rs"), "pub fn beta() -> i32 { 2 }\n").unwrap();
    let mut malformed_registry = AnalyzerRegistry::new(Arc::new(GenericAnalyzer::default()));
    malformed_registry.register(Arc::new(MalformedRustAnalyzer));
    let malformed = indexer(
        Arc::clone(&storage),
        Arc::clone(&embeddings),
        Arc::new(malformed_registry),
    );
    assert!(
        malformed
            .reconcile_file(&workspace, &root.join("b.rs"), Path::new("b.rs"))
            .await
            .is_err()
    );
    let failed_revision = storage
        .workspace_graph_revision(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed_revision.graph_state, GraphState::Error);
    assert!(!failed_revision.is_current());
    assert_eq!(
        call_from(&storage, &workspace, &a.id).await.unwrap().id,
        edge_before.id
    );
    let embedded_after_failure = embeddings.embedded_texts();

    reconcile(&healthy, &workspace, root, "b.rs").await;
    assert_current(&storage, &workspace).await;
    assert_eq!(embeddings.embedded_texts(), embedded_after_failure);
    assert_eq!(
        storage.get_embedding(&a_chunk.id).await.unwrap().unwrap(),
        a_embedding
    );
    assert_eq!(
        call_from(&storage, &workspace, &a.id).await.unwrap().id,
        edge_before.id
    );
}

#[tokio::test]
async fn structural_version_rebuild_preserves_content_revision_embeddings_and_edge_identity() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    std::fs::write(root.join("a.rs"), "pub fn alpha() { beta(); }\n").unwrap();
    std::fs::write(root.join("b.rs"), "pub fn beta() -> i32 { 1 }\n").unwrap();
    let workspace = Workspace::new(root.to_string_lossy(), "structure-version");
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    storage.insert_workspace(&workspace).await.unwrap();
    let embeddings = Arc::new(CountingEmbeddingProvider::new());
    let initial = indexer(Arc::clone(&storage), Arc::clone(&embeddings), analyzers());
    reconcile(&initial, &workspace, root, "a.rs").await;
    reconcile(&initial, &workspace, root, "b.rs").await;
    let a = storage
        .find_document(&workspace.id, "a.rs")
        .await
        .unwrap()
        .unwrap();
    let edge_before = call_from(&storage, &workspace, &a.id).await.unwrap();
    let revision_before = storage
        .workspace_graph_revision(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    let embedded_before = embeddings.embedded_texts();

    let mut bumped_registry = AnalyzerRegistry::new(Arc::new(GenericAnalyzer::default()));
    bumped_registry.register(Arc::new(BumpedStructureRustAnalyzer));
    let bumped = indexer(
        Arc::clone(&storage),
        Arc::clone(&embeddings),
        Arc::new(bumped_registry),
    );
    reconcile(&bumped, &workspace, root, "b.rs").await;
    let midway = storage
        .workspace_graph_revision(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(midway.content_revision, revision_before.content_revision);
    assert_eq!(midway.graph_state, GraphState::Stale);

    reconcile(&bumped, &workspace, root, "a.rs").await;
    let after = storage
        .workspace_graph_revision(&workspace.id)
        .await
        .unwrap()
        .unwrap();
    assert!(after.is_current());
    assert_eq!(after.content_revision, revision_before.content_revision);
    assert_eq!(embeddings.embedded_texts(), embedded_before);
    assert_eq!(
        call_from(&storage, &workspace, &a.id).await.unwrap().id,
        edge_before.id
    );
    for document in storage.list_documents(&workspace.id).await.unwrap() {
        assert_eq!(
            storage
                .graph_analysis_state(&document.id)
                .await
                .unwrap()
                .unwrap()
                .structure_version,
            "rust-structure:test-bump"
        );
    }
}
