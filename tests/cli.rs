use std::process::Command;

use serde_json::Value;
use tempfile::tempdir;

#[test]
fn workspace_and_memory_commands_use_the_service_facade() {
    let directory = tempdir().unwrap();
    let root = directory.path().join("workspace");
    std::fs::create_dir_all(&root).unwrap();
    let database = directory.path().join("cortexweave.sqlite");
    let config = directory.path().join("cortexweave.toml");
    let database_path = database.to_string_lossy().replace('\\', "/");
    std::fs::write(&config, format!("[database]\npath = \"{database_path}\"\n")).unwrap();

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

    let status = run(&config, &["status", workspace_id]);
    let status: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status["workspace"]["name"], "fixture");
    assert_eq!(status["documents_indexed"], 0);

    let memory = run(
        &config,
        &[
            "memory",
            "add",
            workspace_id,
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
