use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use async_trait::async_trait;
use chrono::Utc;
use cortexweave::{
    Result,
    domain::{Document, EmbeddingRecord, StoredChunk, Workspace},
    embedding::EmbeddingProvider,
    retrieval::RetrievalService,
    storage::SqliteStorage,
};

const DIMENSION: usize = 6;

struct EvaluationEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for EvaluationEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|text| concept_vector(text)).collect())
    }

    fn model_name(&self) -> &str {
        "evaluation-space"
    }
}

#[derive(Clone, Copy)]
enum SearchMode {
    Semantic,
    Lexical,
    Hybrid,
}

impl SearchMode {
    fn label(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Lexical => "lexical",
            Self::Hybrid => "hybrid",
        }
    }
}

struct EvaluationCase {
    query: &'static str,
    expected_chunk_id: &'static str,
}

#[derive(Debug)]
struct Metrics {
    recall_at_k: f64,
    mrr: f64,
    average_latency: Duration,
}

#[tokio::test]
async fn fixed_cross_language_retrieval_evaluation() {
    let (storage, workspace) = evaluation_corpus().await;
    let provider: Arc<dyn EmbeddingProvider> = Arc::new(EvaluationEmbeddingProvider);
    let default =
        RetrievalService::new(Arc::clone(&storage), Arc::clone(&provider), 0.70, 0.30).unwrap();
    let balanced =
        RetrievalService::new(Arc::clone(&storage), Arc::clone(&provider), 0.50, 0.50).unwrap();
    let lexical_heavy = RetrievalService::new(storage, provider, 0.30, 0.70).unwrap();
    let cases = evaluation_cases();

    let semantic = evaluate(&default, &workspace.id, &cases, SearchMode::Semantic, 3).await;
    let lexical = evaluate(&default, &workspace.id, &cases, SearchMode::Lexical, 3).await;
    let hybrid = evaluate(&default, &workspace.id, &cases, SearchMode::Hybrid, 3).await;
    let hybrid_top_one = evaluate(&default, &workspace.id, &cases, SearchMode::Hybrid, 1).await;
    let balanced_top_one = evaluate(&balanced, &workspace.id, &cases, SearchMode::Hybrid, 1).await;
    let lexical_heavy_top_one =
        evaluate(&lexical_heavy, &workspace.id, &cases, SearchMode::Hybrid, 1).await;

    print_metrics(SearchMode::Semantic, &semantic);
    print_metrics(SearchMode::Lexical, &lexical);
    print_metrics(SearchMode::Hybrid, &hybrid);
    println!(
        "hybrid weight comparison recall@1: 70/30={:.3}, 50/50={:.3}, 30/70={:.3}",
        hybrid_top_one.recall_at_k, balanced_top_one.recall_at_k, lexical_heavy_top_one.recall_at_k,
    );

    assert_eq!(semantic.recall_at_k, 1.0);
    assert_eq!(semantic.mrr, 1.0);
    assert_eq!(lexical.recall_at_k, 0.4);
    assert_eq!(hybrid.recall_at_k, 1.0);
    assert_eq!(hybrid.mrr, 1.0);
    assert_eq!(hybrid_top_one.recall_at_k, 1.0);
    assert!(hybrid_top_one.recall_at_k > balanced_top_one.recall_at_k);
    assert!(hybrid_top_one.recall_at_k > lexical_heavy_top_one.recall_at_k);
}

async fn evaluate(
    service: &RetrievalService,
    workspace_id: &str,
    cases: &[EvaluationCase],
    mode: SearchMode,
    k: usize,
) -> Metrics {
    let mut hits = 0_usize;
    let mut reciprocal_rank = 0.0_f64;
    let mut elapsed = Duration::ZERO;

    for case in cases {
        let started = Instant::now();
        let results = match mode {
            SearchMode::Semantic => service.semantic_search(workspace_id, case.query, k).await,
            SearchMode::Lexical => service.lexical_search(workspace_id, case.query, k).await,
            SearchMode::Hybrid => service.hybrid_search(workspace_id, case.query, k).await,
        }
        .unwrap();
        elapsed += started.elapsed();
        if let Some(index) = results
            .iter()
            .position(|result| result.chunk_id == case.expected_chunk_id)
        {
            hits += 1;
            reciprocal_rank += 1.0 / (index + 1) as f64;
        }
    }

    Metrics {
        recall_at_k: hits as f64 / cases.len() as f64,
        mrr: reciprocal_rank / cases.len() as f64,
        average_latency: elapsed / u32::try_from(cases.len()).unwrap(),
    }
}

fn print_metrics(mode: SearchMode, metrics: &Metrics) {
    println!(
        "{} recall@3={:.3} mrr={:.3} avg_latency_us={}",
        mode.label(),
        metrics.recall_at_k,
        metrics.mrr,
        metrics.average_latency.as_micros(),
    );
}

fn evaluation_cases() -> Vec<EvaluationCase> {
    vec![
        EvaluationCase {
            query: "Where is document reconciliation implemented?",
            expected_chunk_id: "target-reconcile",
        },
        EvaluationCase {
            query: "Find the Python class that manages caching.",
            expected_chunk_id: "target-cache",
        },
        EvaluationCase {
            query: "Which TypeScript function handles retries?",
            expected_chunk_id: "target-retries",
        },
        EvaluationCase {
            query: "EmbeddingProvider",
            expected_chunk_id: "target-provider",
        },
        EvaluationCase {
            query: "E0425",
            expected_chunk_id: "target-e0425",
        },
    ]
}

async fn evaluation_corpus() -> (Arc<SqliteStorage>, Workspace) {
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    let workspace = Workspace::new("C:/evaluation", "evaluation");
    storage.insert_workspace(&workspace).await.unwrap();
    let fixtures = [
        Fixture {
            id: "target-reconcile",
            path: "src/indexing/reconciler.rs",
            language: "rust",
            symbol: "reconcile_document",
            content: "fn reconcile_document() { /* document reconciliation */ }",
            vector: one_hot(0),
        },
        Fixture {
            id: "target-cache",
            path: "services/cache.py",
            language: "python",
            symbol: "CacheManager",
            content: "class CacheManager:\n    # manages cached values\n    pass",
            vector: one_hot(1),
        },
        Fixture {
            id: "target-retries",
            path: "web/retry.ts",
            language: "typescript",
            symbol: "retryRequest",
            content: "export function retryRequest() { /* retry failed requests */ }",
            vector: one_hot(2),
        },
        Fixture {
            id: "target-provider",
            path: "src/embedding/provider.rs",
            language: "rust",
            symbol: "EmbeddingProvider",
            content: "pub trait EmbeddingProvider { fn embed(&self); }",
            vector: one_hot(3),
        },
        Fixture {
            id: "target-e0425",
            path: "src/diagnostics/rust.rs",
            language: "rust",
            symbol: "explain_e0425",
            content: "fn explain_e0425() { /* compiler error E0425 */ }",
            vector: one_hot(4),
        },
        Fixture {
            id: "distractor-retries",
            path: "docs/questions.ts",
            language: "typescript",
            symbol: "questionText",
            content: "Which TypeScript function handles retries? This is documentation, not the implementation.",
            vector: one_hot(5),
        },
        Fixture {
            id: "distractor-helper",
            path: "src/helpers.js",
            language: "javascript",
            symbol: "formatOutput",
            content: "export function formatOutput(value) { return String(value); }",
            vector: one_hot(5),
        },
    ];
    for fixture in fixtures {
        insert_fixture(&storage, &workspace, fixture).await;
    }
    (storage, workspace)
}

struct Fixture {
    id: &'static str,
    path: &'static str,
    language: &'static str,
    symbol: &'static str,
    content: &'static str,
    vector: Vec<f32>,
}

async fn insert_fixture(storage: &SqliteStorage, workspace: &Workspace, fixture: Fixture) {
    let document = Document {
        id: format!("document-{}", fixture.id),
        workspace_id: workspace.id.clone(),
        relative_path: fixture.path.into(),
        language: fixture.language.into(),
        analyzer_id: "evaluation".into(),
        analyzer_version: "1".into(),
        segmentation_id: "evaluation-v1".into(),
        content_revision: 0,
        content_hash: blake3::hash(fixture.content.as_bytes())
            .to_hex()
            .to_string(),
        size_bytes: fixture.content.len() as i64,
        modified_at_ns: None,
        indexed_at: Utc::now(),
    };
    let mut chunk = StoredChunk::new(
        &document.id,
        format!("{}::symbol:{}", fixture.path, fixture.symbol),
        fixture.content,
    );
    chunk.id = fixture.id.into();
    chunk.language = fixture.language.into();
    chunk.symbol = Some(fixture.symbol.into());
    chunk.qualified_symbol = Some(fixture.symbol.into());
    let embedding = EmbeddingRecord::new(&chunk.id, "evaluation-space", fixture.vector);
    storage
        .persist_document_tree(&document, &[chunk], &[embedding])
        .await
        .unwrap();
}

fn concept_vector(text: &str) -> Vec<f32> {
    let text = text.to_ascii_lowercase();
    if text.contains("reconcil") {
        one_hot(0)
    } else if text.contains("cach") {
        one_hot(1)
    } else if text.contains("retr") {
        one_hot(2)
    } else if text.contains("embeddingprovider") || text.contains("embedding provider") {
        one_hot(3)
    } else if text.contains("e0425") {
        one_hot(4)
    } else {
        one_hot(5)
    }
}

fn one_hot(index: usize) -> Vec<f32> {
    let mut vector = vec![0.0; DIMENSION];
    vector[index] = 1.0;
    vector
}
