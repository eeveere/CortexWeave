use std::{fs, path::PathBuf};

fn source(relative: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)).unwrap()
}

fn production_source(relative: &str) -> String {
    source(relative)
        .split_once("#[cfg(test)]")
        .map_or_else(|| source(relative), |(production, _)| production.to_owned())
}

fn assert_absent(source: &str, path: &str, forbidden: &[&str]) {
    for token in forbidden {
        assert!(
            !source.contains(token),
            "{path} must not depend on or implement {token}"
        );
    }
}

#[test]
fn experience_domain_and_services_are_transport_independent() {
    let forbidden = [
        "crate::adapters",
        "McpServer",
        "clap::",
        "jsonrpc",
        "serve_stdio",
        "tokio::io::stdin",
    ];
    for path in [
        "src/domain/consolidation.rs",
        "src/domain/episode.rs",
        "src/domain/evidence.rs",
        "src/domain/experience.rs",
        "src/domain/failure.rs",
        "src/service/consolidation.rs",
        "src/service/context.rs",
        "src/service/evidence.rs",
        "src/service/experience_assessment.rs",
        "src/service/experience_search.rs",
        "src/service/failure.rs",
    ] {
        assert_absent(&production_source(path), path, &forbidden);
    }
}

#[test]
fn context_uses_storage_services_and_repository_has_no_tool_specific_semantics() {
    let context = production_source("src/service/context.rs");
    assert_absent(&context, "src/service/context.rs", &["sqlx::", ".pool()"]);

    let repository = production_source("src/storage/repositories.rs");
    assert_absent(
        &repository,
        "src/storage/repositories.rs",
        &[
            "cortexweave.rust_compiler_result",
            "cortexweave.cargo_test_result",
            "E0308",
            "tree-sitter-rust",
        ],
    );
}

#[test]
fn adapters_do_not_reimplement_experience_outcome_or_ranking_policy() {
    let adapters = format!(
        "{}\n{}",
        production_source("src/adapters/mcp.rs"),
        production_source("src/main.rs")
    );
    assert_absent(
        &adapters,
        "adapter surfaces",
        &[
            "ExperienceOutcome::",
            "VerificationStatus::",
            "experience_lifecycle(",
            "ExperienceSearchScores {",
            "experience_rank(",
        ],
    );
}

#[test]
fn native_full_cycle_proof_imports_no_adapter_or_transport() {
    let contract = source("tests/native_experience_full_cycle.rs");
    assert!(contract.contains("CortexWeaveService"));
    assert_absent(
        &contract,
        "tests/native_experience_full_cycle.rs",
        &[
            "adapters::",
            "McpServer",
            "clap::",
            "jsonrpc",
            "serve_stdio",
        ],
    );
}
