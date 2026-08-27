use std::{fs, sync::Arc};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result,
    domain::{CortexEvent, EventType, MemoryKind, MemoryRecord},
    embedding::EmbeddingProvider,
    storage::SqliteStorage,
};
use serde_json::json;
use tempfile::tempdir;

struct ContextEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for ContextEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|text| {
                if text.to_ascii_lowercase().contains("retry") {
                    vec![1.0, 0.0]
                } else {
                    vec![0.0, 1.0]
                }
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        "fresh-context-demo"
    }

    fn dimension(&self) -> Option<usize> {
        Some(2)
    }
}

#[tokio::test]
async fn fresh_session_recovers_work_from_explicit_memory_and_current_code() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    let database = directory.path().join("cortexweave.db");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("services")).unwrap();
    fs::create_dir_all(root.join("web")).unwrap();
    fs::write(
        root.join("src/retry.rs"),
        "pub fn retry_limit() -> u32 { 3 }\n",
    )
    .unwrap();
    fs::write(
        root.join("services/cache.py"),
        "class Cache:\n    def get(self, key):\n        return key\n",
    )
    .unwrap();
    fs::write(
        root.join("web/client.ts"),
        "export function retryRequest() { return 'retry'; }\n",
    )
    .unwrap();

    let workspace_id = {
        let service = CortexWeaveService::from_parts_with_embeddings(
            AppConfig::default(),
            SqliteStorage::open(&database).await.unwrap(),
            Arc::new(ContextEmbeddingProvider),
        )
        .unwrap();
        let workspace = service
            .register_workspace(root.to_string_lossy(), "fresh-context")
            .await
            .unwrap();
        service.workspace_reindex(&workspace.id).await.unwrap();
        assert!(
            service
                .hybrid_search(&workspace.id, "retry", 10)
                .await
                .unwrap()
                .len()
                >= 3
        );

        let session = service
            .start_session(&workspace.id, json!({ "phase": "session-a" }))
            .await
            .unwrap();
        let task = service
            .start_task(
                &workspace.id,
                Some(session.id.clone()),
                "Make retry limits explicit",
                json!({ "files": ["src/retry.rs", "services/cache.py", "web/client.ts"] }),
            )
            .await
            .unwrap();

        fs::write(
            root.join("src/retry.rs"),
            "pub fn retry_limit() -> u32 { missing_retry_limit }\n",
        )
        .unwrap();
        service.workspace_reindex(&workspace.id).await.unwrap();
        let mut failure = CortexEvent::new(
            &workspace.id,
            EventType::CompilerResult,
            json!({ "ok": false, "message": "cannot find value missing_retry_limit" }),
        );
        failure.session_id = Some(session.id.clone());
        failure.task_id = Some(task.id.clone());
        service.record_event(failure).await.unwrap();

        fs::write(
            root.join("src/retry.rs"),
            "pub fn retry_limit() -> u32 { 5 }\n",
        )
        .unwrap();
        let fixed = service.workspace_reindex(&workspace.id).await.unwrap();
        assert_eq!(fixed.chunks_embedded, 1);

        record_memory(
            &service,
            &workspace.id,
            &session.id,
            &task.id,
            MemoryKind::Observation,
            "We were making retry limits explicit across the mixed-language request path.",
            &["src/retry.rs", "services/cache.py", "web/client.ts"],
        )
        .await;
        record_memory(
            &service,
            &workspace.id,
            &session.id,
            &task.id,
            MemoryKind::Decision,
            "Decision: keep the retry limit in src/retry.rs bounded at five attempts.",
            &["src/retry.rs"],
        )
        .await;
        record_memory(
            &service,
            &workspace.id,
            &session.id,
            &task.id,
            MemoryKind::Todo,
            "TODO: add exponential backoff coverage for retryRequest.",
            &["src/retry.rs", "web/client.ts"],
        )
        .await;
        service
            .complete_task(&task.id, json!({ "fixed_retry_limit": 5 }))
            .await
            .unwrap();
        service.end_session(&session.id).await.unwrap();
        workspace.id
    };

    // Session B opens only persisted state. It receives no Session A transcript.
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::open(&database).await.unwrap(),
        Arc::new(ContextEmbeddingProvider),
    )
    .unwrap();
    let memories = service
        .search_memories(&workspace_id, "retry", 10)
        .await
        .unwrap();
    let observation = memories
        .iter()
        .find(|memory| memory.kind == MemoryKind::Observation)
        .unwrap();
    let decision = memories
        .iter()
        .find(|memory| memory.kind == MemoryKind::Decision)
        .unwrap();
    let todo_memories = service
        .search_memories(&workspace_id, "backoff", 10)
        .await
        .unwrap();
    let todo = todo_memories
        .iter()
        .find(|memory| memory.kind == MemoryKind::Todo)
        .unwrap();
    assert!(observation.content.contains("making retry limits explicit"));
    assert!(decision.content.contains("five attempts"));
    assert!(todo.content.contains("exponential backoff"));
    assert_eq!(
        observation.related_paths,
        ["src/retry.rs", "services/cache.py", "web/client.ts"]
    );

    let fresh = service
        .semantic_search(&workspace_id, "retry limit", 10)
        .await
        .unwrap();
    assert!(fresh.iter().any(|result| {
        result.path == "src/retry.rs" && result.content.contains("retry_limit() -> u32 { 5 }")
    }));

    fs::write(
        root.join("src/retry.rs"),
        "pub fn retry_limit() -> u32 { 6 }\n",
    )
    .unwrap();
    let update = service.workspace_reindex(&workspace_id).await.unwrap();
    assert_eq!(update.chunks_embedded, 1);
    let refreshed = service
        .semantic_search(&workspace_id, "retry limit", 10)
        .await
        .unwrap();
    assert!(refreshed.iter().any(|result| {
        result.path == "src/retry.rs" && result.content.contains("retry_limit() -> u32 { 6 }")
    }));
}

async fn record_memory(
    service: &CortexWeaveService,
    workspace_id: &str,
    session_id: &str,
    task_id: &str,
    kind: MemoryKind,
    content: &str,
    paths: &[&str],
) {
    let mut memory = MemoryRecord::new(workspace_id, kind, content);
    memory.session_id = Some(session_id.into());
    memory.task_id = Some(task_id.into());
    memory.related_paths = paths.iter().map(|path| (*path).into()).collect();
    service.record_memory(memory).await.unwrap();
}
