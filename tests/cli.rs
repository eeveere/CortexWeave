use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn version_reports_the_release_identity() {
    let output = Command::new(env!("CARGO_BIN_EXE_cortexweave"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "cortexweave 0.4.1"
    );
}

#[test]
fn workspace_and_memory_commands_use_the_service_facade() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.py"), "def run():\n    return True\n").unwrap();
    let database = directory.path().join("cortexweave.sqlite");
    let config = directory.path().join("cortexweave.toml");
    let database_path = database.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config,
        format!("[database]\npath = \"{database_path}\"\n[languages]\npython = false\n"),
    )
    .unwrap();

    let added = run(
        &config,
        &[
            "workspace",
            "add",
            root.to_str().unwrap(),
            "--name",
            "fixture",
        ],
    );
    let workspace: Value = serde_json::from_slice(&added.stdout).unwrap();
    let workspace_id = workspace["id"].as_str().unwrap();
    let added_again = run(
        &config,
        &[
            "workspace",
            "add",
            root.to_str().unwrap(),
            "--name",
            "replacement",
        ],
    );
    let same_workspace: Value = serde_json::from_slice(&added_again.stdout).unwrap();
    assert_eq!(same_workspace["id"], workspace["id"]);
    assert_eq!(same_workspace["name"], "fixture");
    let listed = run(&config, &["workspace", "list"]);
    let workspaces: Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(workspaces.as_array().unwrap().len(), 1);

    let status = run(&config, &["status", "fixture"]);
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["workspace"]["name"], "fixture");
    assert_eq!(status["documents_indexed"], 0);

    let rebuilt = run(&config, &["graph", "rebuild", "fixture"]);
    let rebuilt: Value = serde_json::from_slice(&rebuilt.stdout).unwrap();
    assert_eq!(rebuilt["final_graph_state"], "completed");
    assert_eq!(rebuilt["embeddings_computed"], 0);

    let readiness = run(&config, &["readiness", "fixture"]);
    let readiness: Value = serde_json::from_slice(&readiness.stdout).unwrap();
    assert_eq!(readiness["read_only"], true);
    assert_eq!(readiness["supported_fallback_files"], 1);
    assert_eq!(
        readiness["recommendations"][0]["config_key"],
        "languages.python"
    );
    assert_eq!(readiness["recommended_rebuild"]["documents"], 0);

    let memory = run(
        &config,
        &[
            "memory",
            "add",
            "fixture",
            "Use BLAKE3 for deterministic change detection.",
            "--kind",
            "decision",
            "--related-path",
            "src/indexing/reconciler.rs",
        ],
    );
    let memory: Value = serde_json::from_slice(&memory.stdout).unwrap();
    assert_eq!(memory["kind"], "decision");
    assert_eq!(memory["related_paths"][0], "src/indexing/reconciler.rs");

    let context = run(
        &config,
        &[
            "context",
            "fixture",
            "BLAKE3",
            "--token-budget",
            "256",
            "--no-code",
            "--no-documents",
            "--no-events",
        ],
    );
    let context: Value = serde_json::from_slice(&context.stdout).unwrap();
    assert_eq!(context["workspace_id"], workspace_id);
    assert_eq!(context["token_budget"], 256);
    assert!(
        context["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["source_type"] == "memory")
    );

    let resume = run(&config, &["resume", "fixture", "--token-budget", "256"]);
    let resume: Value = serde_json::from_slice(&resume.stdout).unwrap();
    assert_eq!(resume["workspace_id"], workspace_id);
    assert_eq!(resume["packet"]["token_budget"], 256);
}

#[test]
fn graph_command_exposes_bounded_structural_subcommands() {
    let output = Command::new(env!("CARGO_BIN_EXE_cortexweave"))
        .args(["graph", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in [
        "status",
        "find",
        "neighbors",
        "callers",
        "implementations",
        "impact-symbol",
        "impact-path",
        "rebuild",
    ] {
        assert!(help.contains(command), "missing graph command {command}");
    }

    let detail = Command::new(env!("CARGO_BIN_EXE_cortexweave"))
        .args(["graph", "impact-symbol", "--help"])
        .output()
        .unwrap();
    assert!(detail.status.success());
    let detail = String::from_utf8(detail.stdout).unwrap();
    for option in ["--allow-stale", "--max-nodes", "--max-edges", "--max-depth"] {
        assert!(detail.contains(option), "missing graph option {option}");
    }
}

fn run(config: &std::path::Path, arguments: &[&str]) -> std::process::Output {
    let output = Command::new(env!("CARGO_BIN_EXE_cortexweave"))
        .arg("--config")
        .arg(config)
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
