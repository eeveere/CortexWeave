use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result, embedding::EmbeddingProvider, storage::SqliteStorage,
};
use serde::Serialize;
use tempfile::tempdir;

struct BenchmarkProvider {
    calls: AtomicUsize,
    texts: AtomicUsize,
}

impl BenchmarkProvider {
    fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            texts: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for BenchmarkProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.texts.fetch_add(texts.len(), Ordering::Relaxed);
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0; 16];
                for (index, byte) in text.bytes().enumerate() {
                    vector[index % 16] += f32::from(byte) / 255.0;
                }
                vector
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "baseline-model"
    }

    fn dimension(&self) -> Option<usize> {
        Some(16)
    }

    fn batch_size(&self) -> Option<usize> {
        Some(16)
    }
}

#[derive(Serialize)]
struct Baseline {
    fixture: &'static str,
    files: usize,
    chunks: usize,
    initial_indexing_ms: f64,
    startup_reconciliation_ms: f64,
    startup_embedding_calls: usize,
    single_function_edit_ms: f64,
    embedding_calls_per_edit: usize,
    chunks_reembedded: usize,
    chunks_actually_changed: usize,
    reembedded_to_changed_ratio: f64,
    semantic_query_ms: f64,
    hybrid_query_ms: f64,
    database_bytes: u64,
    working_set_bytes: Option<u64>,
    analyzer_average_ms: f64,
    analyzer_max_ms: f64,
}

#[tokio::test]
async fn performance_baseline_preserves_incremental_efficiency() {
    let single = run_fixture("single-language", false).await;
    let mixed = run_fixture("mixed-language", true).await;

    assert_eq!(single.startup_embedding_calls, 0);
    assert_eq!(mixed.startup_embedding_calls, 0);
    assert_eq!(single.embedding_calls_per_edit, 1);
    assert_eq!(mixed.embedding_calls_per_edit, 1);
    assert_eq!(single.reembedded_to_changed_ratio, 1.0);
    assert_eq!(mixed.reembedded_to_changed_ratio, 1.0);

    println!(
        "{}",
        serde_json::to_string_pretty(&vec![single, mixed]).unwrap()
    );
}

async fn run_fixture(name: &'static str, mixed: bool) -> Baseline {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    fs::create_dir_all(&root).unwrap();
    write_fixture(&root, mixed);
    let database = directory.path().join("cortexweave.db");
    let provider = Arc::new(BenchmarkProvider::new());
    let provider_trait: Arc<dyn EmbeddingProvider> = provider.clone();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::open(&database).await.unwrap(),
        provider_trait,
    )
    .unwrap();
    let workspace = service
        .register_workspace(root.to_string_lossy(), name)
        .await
        .unwrap();

    let initial_started = Instant::now();
    service.workspace_reindex(&workspace.id).await.unwrap();
    let initial_indexing_ms = elapsed_ms(initial_started);
    let initial_snapshot = service.instrumentation(Some(&workspace.id)).await.unwrap();
    let files = initial_snapshot.documents;
    let chunks = initial_snapshot.chunks;
    drop(service);

    let restart_calls_before = provider.calls.load(Ordering::Relaxed);
    let provider_trait: Arc<dyn EmbeddingProvider> = provider.clone();
    let restarted = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::open(&database).await.unwrap(),
        provider_trait,
    )
    .unwrap();
    let startup_started = Instant::now();
    restarted.workspace_reindex(&workspace.id).await.unwrap();
    let startup_reconciliation_ms = elapsed_ms(startup_started);
    let startup_embedding_calls = provider.calls.load(Ordering::Relaxed) - restart_calls_before;

    fs::write(
        root.join("lib.rs"),
        "pub fn stable() -> i32 { 1 }\npub fn edited() -> i32 { 20 }\n",
    )
    .unwrap();
    let edit_calls_before = provider.calls.load(Ordering::Relaxed);
    let edit_started = Instant::now();
    let edit = restarted.workspace_reindex(&workspace.id).await.unwrap();
    let single_function_edit_ms = elapsed_ms(edit_started);
    let embedding_calls_per_edit = provider.calls.load(Ordering::Relaxed) - edit_calls_before;

    let semantic_started = Instant::now();
    restarted
        .semantic_search(&workspace.id, "edited function", 5)
        .await
        .unwrap();
    let semantic_query_ms = elapsed_ms(semantic_started);
    let hybrid_started = Instant::now();
    restarted
        .hybrid_search(&workspace.id, "edited function", 5)
        .await
        .unwrap();
    let hybrid_query_ms = elapsed_ms(hybrid_started);
    let snapshot = restarted
        .instrumentation(Some(&workspace.id))
        .await
        .unwrap();
    let chunks_actually_changed = 1;
    let chunks_reembedded = edit.chunks_embedded;

    Baseline {
        fixture: name,
        files,
        chunks,
        initial_indexing_ms,
        startup_reconciliation_ms,
        startup_embedding_calls,
        single_function_edit_ms,
        embedding_calls_per_edit,
        chunks_reembedded,
        chunks_actually_changed,
        reembedded_to_changed_ratio: chunks_reembedded as f64 / chunks_actually_changed as f64,
        semantic_query_ms,
        hybrid_query_ms,
        database_bytes: fs::metadata(&database).unwrap().len(),
        working_set_bytes: working_set_bytes(),
        analyzer_average_ms: snapshot.analyzer_latency.average_ms,
        analyzer_max_ms: snapshot.analyzer_latency.max_ms,
    }
}

fn write_fixture(root: &Path, mixed: bool) {
    fs::write(
        root.join("lib.rs"),
        "pub fn stable() -> i32 { 1 }\npub fn edited() -> i32 { 2 }\n",
    )
    .unwrap();
    if !mixed {
        return;
    }
    fs::write(root.join("main.py"), "def python_value():\n    return 1\n").unwrap();
    fs::write(
        root.join("app.js"),
        "export function javascriptValue() { return 1; }\n",
    )
    .unwrap();
    fs::write(
        root.join("view.ts"),
        "export function typescriptValue(): number { return 1; }\n",
    )
    .unwrap();
    fs::write(
        root.join("Program.cs"),
        "class Program { static int CsharpValue() { return 1; } }\n",
    )
    .unwrap();
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

#[cfg(windows)]
fn working_set_bytes() -> Option<u64> {
    use std::{ffi::c_void, mem::size_of};

    #[repr(C)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
    }
    #[link(name = "psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCounters,
            size: u32,
        ) -> i32;
    }

    let mut counters = ProcessMemoryCounters {
        cb: size_of::<ProcessMemoryCounters>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    // SAFETY: both functions are called with the current process pseudo-handle
    // and a correctly sized writable PROCESS_MEMORY_COUNTERS buffer.
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            size_of::<ProcessMemoryCounters>() as u32,
        )
    };
    (ok != 0).then_some(counters.working_set_size as u64)
}

#[cfg(not(windows))]
fn working_set_bytes() -> Option<u64> {
    None
}
