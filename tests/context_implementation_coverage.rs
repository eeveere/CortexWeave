use std::{fs, sync::Arc};

use async_trait::async_trait;
use cortexweave::{
    AppConfig, CortexWeaveService, Result,
    domain::ContextRequest,
    embedding::EmbeddingProvider,
    evaluation::{
        ImplementationCoverageCase, ImplementationEvidence, evaluate_implementation_coverage,
    },
    storage::SqliteStorage,
};
use tempfile::tempdir;

struct CoverageEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for CoverageEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
    }

    fn model_name(&self) -> &str {
        "implementation-coverage"
    }

    fn dimension(&self) -> Option<usize> {
        Some(2)
    }
}

#[tokio::test]
async fn opihype_packets_cover_named_implementation_evidence() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("opihype");
    write_fixture(&root);
    let service = CortexWeaveService::from_parts_with_embeddings(
        AppConfig::default(),
        SqliteStorage::in_memory().await.unwrap(),
        Arc::new(CoverageEmbeddingProvider),
    )
    .unwrap();
    let workspace = service
        .register_workspace(root.to_string_lossy(), "opihype")
        .await
        .unwrap();
    service.workspace_reindex(&workspace.id).await.unwrap();

    let lifecycle = packet(
        &service,
        &workspace.id,
        "ProcessManager ensure_ready restart MemoryWatchdog FastAPI lifespan lifecycle",
    )
    .await;
    let active_restart = packet(
        &service,
        &workspace.id,
        "MemoryWatchdog _begin_restart _restart JobRunner _wait_for_watchdog active run lifecycle test",
    )
    .await;
    let metrics = evaluate_implementation_coverage(&[
        ImplementationCoverageCase {
            id: "llama-server-lifecycle".into(),
            packet: lifecycle,
            expected: vec![
                evidence("opihype/process_manager.py", "ProcessManager.ensure_ready"),
                evidence("opihype/process_manager.py", "ProcessManager.restart"),
                evidence("opihype/watchdog.py", "MemoryWatchdog"),
                evidence("opihype/app.py", "lifespan"),
            ],
        },
        ImplementationCoverageCase {
            id: "watchdog-restart-during-active-run".into(),
            packet: active_restart,
            expected: vec![
                evidence(
                    "opihype/watchdog_restart.py",
                    "MemoryWatchdog._begin_restart",
                ),
                evidence("opihype/watchdog_restart.py", "MemoryWatchdog._restart"),
                evidence("opihype/job_runner.py", "JobRunner._wait_for_watchdog"),
                evidence(
                    "tests/test_lifecycle.py",
                    "test_watchdog_restart_during_active_run",
                ),
            ],
        },
    ]);

    assert_eq!(metrics.case_count, 2);
    assert_eq!(metrics.passed_cases, 2, "{metrics:#?}");
    assert_eq!(metrics.pass_rate, 1.0);
    assert_eq!(metrics.mean_coverage, 1.0);
    assert!(metrics.cases.iter().all(|case| case.passed));
}

async fn packet(
    service: &CortexWeaveService,
    workspace_id: &str,
    query: &str,
) -> cortexweave::domain::ContextPacket {
    let mut request = ContextRequest::new(workspace_id);
    request.query = Some(query.into());
    request.token_budget = 4_096;
    request.include_memories = false;
    request.include_events = false;
    service.semantic_context(request).await.unwrap()
}

fn evidence(path: &str, symbol: &str) -> ImplementationEvidence {
    ImplementationEvidence {
        path: path.into(),
        symbol: symbol.into(),
    }
}

fn write_fixture(root: &std::path::Path) {
    fs::create_dir_all(root.join("opihype")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(
        root.join("opihype/process_manager.py"),
        "class ProcessManager:\n    def ensure_ready(self):\n        return self.running\n\n    def restart(self):\n        self.running = False\n        return self.ensure_ready()\n",
    )
    .unwrap();
    fs::write(
        root.join("opihype/watchdog.py"),
        "class MemoryWatchdog:\n    pass\n",
    )
    .unwrap();
    fs::write(
        root.join("opihype/watchdog_restart.py"),
        "class MemoryWatchdog:\n    def _begin_restart(self):\n        self.restarting = True\n\n    def _restart(self):\n        self._begin_restart()\n        return 'restarted'\n",
    )
    .unwrap();
    fs::write(
        root.join("opihype/job_runner.py"),
        "class JobRunner:\n    def _wait_for_watchdog(self, watchdog):\n        return not watchdog.restarting\n",
    )
    .unwrap();
    fs::write(
        root.join("opihype/app.py"),
        "def lifespan(app):\n    app.process_manager.ensure_ready()\n    yield\n",
    )
    .unwrap();
    fs::write(
        root.join("tests/test_lifecycle.py"),
        "def test_watchdog_restart_during_active_run():\n    assert run_active_job_with_restart()\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/restart-plan.md"),
        "The plan mentions ProcessManager.ensure_ready, ProcessManager.restart, MemoryWatchdog._begin_restart, MemoryWatchdog._restart, JobRunner._wait_for_watchdog, lifespan, and test_watchdog_restart_during_active_run without implementing them.\n",
    )
    .unwrap();
}
