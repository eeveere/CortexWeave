use std::{
    io::{Read, Write},
    process::{Command, Stdio},
};

use serde_json::{Value, json};
use tempfile::tempdir;

#[test]
fn serve_speaks_line_delimited_mcp_over_stdio() {
    let directory = tempdir().unwrap();
    let environment_root = directory.path().join("environment-workspace");
    let argument_root = directory.path().join("argument-workspace");
    std::fs::create_dir_all(&environment_root).unwrap();
    std::fs::create_dir_all(&argument_root).unwrap();
    std::fs::write(
        argument_root.join("main.py"),
        "def run():\n    return True\n",
    )
    .unwrap();
    let database = directory.path().join("cortexweave.sqlite");
    let config = directory.path().join("cortexweave.toml");
    let database_path = database.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &config,
        format!("[database]\npath = \"{database_path}\"\n[languages]\npython = false\n"),
    )
    .unwrap();
    register_workspace(&config, &environment_root, "environment");
    let argument_workspace = register_workspace(&config, &argument_root, "argument");
    let mut child = Command::new(env!("CARGO_BIN_EXE_cortexweave"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "serve",
            "--workspace-root",
            argument_root.to_str().unwrap(),
        ])
        .env("CORTEXWEAVE_WORKSPACE_ROOT", &environment_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18" },
    });
    let tools = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
    let status = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": { "name": "workspace_status", "arguments": {} },
    });
    let readiness = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": { "name": "workspace_readiness", "arguments": {} },
    });
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", serde_json::to_string(&initialize).unwrap()).unwrap();
    writeln!(input, "{}", serde_json::to_string(&tools).unwrap()).unwrap();
    writeln!(input, "{}", serde_json::to_string(&status).unwrap()).unwrap();
    writeln!(input, "{}", serde_json::to_string(&readiness).unwrap()).unwrap();
    drop(input);

    let mut output = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .unwrap();
    let status = child.wait().unwrap();
    assert!(status.success());
    let responses: Vec<Value> = output
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 4);
    assert_eq!(responses[0]["id"], 1);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(responses[0]["result"]["serverInfo"]["version"], "0.5.0");
    assert_eq!(responses[1]["id"], 2);
    assert!(
        responses[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "workspace_reindex")
    );
    assert!(
        responses[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "graph_rebuild")
    );
    assert!(
        responses[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "workspace_readiness")
    );
    assert_eq!(responses[2]["id"], 3);
    assert_eq!(responses[2]["result"]["isError"], false);
    assert_eq!(
        responses[2]["result"]["structuredContent"]["workspace"]["id"],
        argument_workspace["id"]
    );
    assert_eq!(responses[3]["id"], 4);
    assert_eq!(responses[3]["result"]["isError"], false);
    assert_eq!(
        responses[3]["result"]["structuredContent"]["recommendations"][0]["config_key"],
        "languages.python"
    );
}

fn register_workspace(config: &std::path::Path, root: &std::path::Path, name: &str) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_cortexweave"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "workspace",
            "add",
            root.to_str().unwrap(),
            "--name",
            name,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "workspace registration failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
