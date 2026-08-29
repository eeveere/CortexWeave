use std::sync::Arc;

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result, embedding::EmbeddingProvider, storage::SqliteStorage,
};
use tempfile::tempdir;

struct DeterministicEmbeddings;

#[async_trait]
impl EmbeddingProvider for DeterministicEmbeddings {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0, 0.0, 0.0]).collect())
    }

    fn model_name(&self) -> &str {
        "new-language-extension-test"
    }

    fn dimension(&self) -> Option<usize> {
        Some(4)
    }
}

#[tokio::test]
async fn unsupported_language_keeps_deterministic_chunks_without_structural_claims() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("plugin.lua"),
        "local function run()\n  return 'deterministic fallback'\nend\n",
    )
    .unwrap();
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(DeterministicEmbeddings),
    )
    .unwrap();
    let workspace = service
        .register_workspace(root.to_string_lossy(), "new-language")
        .await
        .unwrap();

    let indexed = service.workspace_reindex(&workspace.id).await.unwrap();
    assert_eq!(indexed.files_seen, 1);
    assert!(indexed.chunks_embedded >= 1);

    let status = service.workspace_graph_status(&workspace.id).await.unwrap();
    assert!(status.is_current);
    assert_eq!(status.languages.len(), 1);
    let language = &status.languages[0];
    assert_eq!(language.language, "text");
    assert_eq!(language.analyzer_id, "generic");
    assert!(language.using_generic_fallback);
    assert!(!language.capabilities.structural_chunks);
    assert!(!language.capabilities.calls);
    assert!(!language.capabilities.references);
    assert!(!language.capabilities.implementations);
}
