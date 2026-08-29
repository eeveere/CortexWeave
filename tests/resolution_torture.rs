use std::sync::Arc;

use cortexweave::{
    domain::{
        AnalyzedSymbol, Document, ResolutionAlias, ResolutionBasis, ResolutionOutcome,
        ResolutionRequest, SymbolKind, Workspace,
    },
    graph::{SymbolRegistry, SymbolResolver},
    storage::SqliteStorage,
};

fn symbol(
    stable_key: &str,
    name: &str,
    qualified_name: &str,
    symbol_kind: SymbolKind,
) -> AnalyzedSymbol {
    AnalyzedSymbol {
        stable_key: stable_key.into(),
        name: name.into(),
        qualified_name: Some(qualified_name.into()),
        symbol_kind,
        parent_key: None,
        start_byte: 0,
        end_byte: 10,
        start_line: 1,
        end_line: 1,
        metadata: serde_json::json!({}),
    }
}

async fn workspace(storage: &SqliteStorage, name: &str) -> Workspace {
    let workspace = Workspace::new(format!("C:/fixtures/{name}"), name);
    storage.insert_workspace(&workspace).await.unwrap();
    workspace
}

async fn document(storage: &SqliteStorage, workspace: &Workspace, path: &str) -> Document {
    let mut document = Document::new(&workspace.id, path);
    document.language = "rust".into();
    document.analyzer_id = "rust-tree-sitter".into();
    document.analyzer_version = "1".into();
    document.segmentation_id = "logical-v1".into();
    document.content_revision = 1;
    document.size_bytes = 100;
    storage.insert_document(&document).await.unwrap();
    document
}

fn assert_resolved(
    outcome: ResolutionOutcome,
    qualified_name: &str,
    expected_basis: ResolutionBasis,
) -> String {
    match outcome {
        ResolutionOutcome::Resolved { node, basis } => {
            assert_eq!(basis, expected_basis);
            assert_eq!(node.qualified_name.as_deref(), Some(qualified_name));
            node.id
        }
        other => panic!("expected {qualified_name} to resolve, got {other:?}"),
    }
}

fn assert_ambiguous(outcome: ResolutionOutcome, qualified_names: &[&str]) {
    match outcome {
        ResolutionOutcome::Ambiguous {
            candidates,
            external_targets,
            ..
        } => {
            assert!(external_targets.is_empty());
            let mut actual: Vec<_> = candidates
                .iter()
                .filter_map(|node| node.qualified_name.as_deref())
                .collect();
            actual.sort_unstable();
            let mut expected = qualified_names.to_vec();
            expected.sort_unstable();
            assert_eq!(actual, expected);
        }
        other => panic!("expected ambiguity, got {other:?}"),
    }
}

#[tokio::test]
async fn resolution_torture_test() {
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    let registry = SymbolRegistry::new(Arc::clone(&storage));
    let resolver = SymbolResolver::new(Arc::clone(&storage));

    // Same function name in two modules: a workspace-wide lookup is ambiguous,
    // while explicit source-document context deterministically narrows it.
    let functions = workspace(&storage, "same-functions").await;
    let alpha_document = document(&storage, &functions, "src/alpha.rs").await;
    let beta_document = document(&storage, &functions, "src/beta.rs").await;
    registry
        .reconcile_document(
            &alpha_document,
            "rust-structure-v1",
            &[symbol(
                "alpha::run",
                "run",
                "alpha::run",
                SymbolKind::Function,
            )],
        )
        .await
        .unwrap();
    registry
        .reconcile_document(
            &beta_document,
            "rust-structure-v1",
            &[symbol(
                "beta::run",
                "run",
                "beta::run",
                SymbolKind::Function,
            )],
        )
        .await
        .unwrap();
    let request = ResolutionRequest::new(
        &functions.id,
        cortexweave::domain::RelationshipTarget::QualifiedSymbol("run".into()),
    );
    assert_ambiguous(
        resolver.resolve(&request).await.unwrap(),
        &["alpha::run", "beta::run"],
    );
    let mut local_request = request.clone();
    local_request.source_document_id = Some(alpha_document.id.clone());
    assert_resolved(
        resolver.resolve(&local_request).await.unwrap(),
        "alpha::run",
        ResolutionBasis::SourceDocument,
    );

    // Same method name in different classes: the caller's enclosing container
    // wins before document or workspace name lookup.
    let methods = workspace(&storage, "same-methods").await;
    let methods_document = document(&storage, &methods, "src/types.rs").await;
    let registered = registry
        .reconcile_document(
            &methods_document,
            "rust-structure-v1",
            &[
                symbol("a::caller", "caller", "A::caller", SymbolKind::Method),
                symbol("a::run", "run", "A::run", SymbolKind::Method),
                symbol("b::run", "run", "B::run", SymbolKind::Method),
            ],
        )
        .await
        .unwrap();
    let caller = registered
        .nodes
        .iter()
        .find(|node| node.qualified_name.as_deref() == Some("A::caller"))
        .unwrap();
    let mut method_request = ResolutionRequest::new(
        &methods.id,
        cortexweave::domain::RelationshipTarget::QualifiedSymbol("run".into()),
    );
    method_request.source_node_id = Some(caller.id.clone());
    assert_resolved(
        resolver.resolve(&method_request).await.unwrap(),
        "A::run",
        ResolutionBasis::EnclosingContainer,
    );

    // Import renames and duplicate aliases are data, not resolver guesses.
    let aliases = workspace(&storage, "aliases").await;
    let aliases_document = document(&storage, &aliases, "src/apis.rs").await;
    registry
        .reconcile_document(
            &aliases_document,
            "rust-structure-v1",
            &[
                symbol(
                    "alpha::api::run",
                    "run",
                    "alpha::api::run",
                    SymbolKind::Function,
                ),
                symbol(
                    "beta::api::run",
                    "run",
                    "beta::api::run",
                    SymbolKind::Function,
                ),
            ],
        )
        .await
        .unwrap();
    let mut alias_request = ResolutionRequest::new(
        &aliases.id,
        cortexweave::domain::RelationshipTarget::QualifiedSymbol("api::run".into()),
    );
    alias_request
        .aliases
        .push(ResolutionAlias::new("api", "alpha::api"));
    assert_resolved(
        resolver.resolve(&alias_request).await.unwrap(),
        "alpha::api::run",
        ResolutionBasis::Alias,
    );
    alias_request
        .aliases
        .push(ResolutionAlias::new("api", "beta::api"));
    assert_ambiguous(
        resolver.resolve(&alias_request).await.unwrap(),
        &["alpha::api::run", "beta::api::run"],
    );

    // Nested modules resolve exactly, without suffix matching.
    let nested = workspace(&storage, "nested-modules").await;
    let nested_document = document(&storage, &nested, "src/outer/inner.rs").await;
    registry
        .reconcile_document(
            &nested_document,
            "rust-structure-v1",
            &[
                symbol("outer::inner", "inner", "outer::inner", SymbolKind::Module),
                symbol(
                    "outer::inner::run",
                    "run",
                    "outer::inner::run",
                    SymbolKind::Function,
                ),
            ],
        )
        .await
        .unwrap();
    let nested_request = ResolutionRequest::new(
        &nested.id,
        cortexweave::domain::RelationshipTarget::ModulePath("outer::inner".into()),
    );
    assert_resolved(
        resolver.resolve(&nested_request).await.unwrap(),
        "outer::inner",
        ResolutionBasis::Module,
    );
    let local_key_request = ResolutionRequest::new(
        &nested.id,
        cortexweave::domain::RelationshipTarget::LocalStableKey("outer::inner::run".into()),
    );
    assert_resolved(
        resolver.resolve(&local_key_request).await.unwrap(),
        "outer::inner::run",
        ResolutionBasis::LocalStableKey,
    );

    // External targets require an explicit external root. A missing target never
    // becomes external merely because no local symbol exists.
    let external = workspace(&storage, "external-and-missing").await;
    let mut external_request = ResolutionRequest::new(
        &external.id,
        cortexweave::domain::RelationshipTarget::ModulePath("serde::de".into()),
    );
    external_request.external_module_roots.push("serde".into());
    assert_eq!(
        resolver.resolve(&external_request).await.unwrap(),
        ResolutionOutcome::External {
            target: "serde::de".into()
        }
    );
    let mut ambiguous_external = ResolutionRequest::new(
        &external.id,
        cortexweave::domain::RelationshipTarget::ModulePath("codec::read".into()),
    );
    ambiguous_external.aliases = vec![
        ResolutionAlias::new("codec", "serde::de"),
        ResolutionAlias::new("codec", "borsh::de"),
    ];
    ambiguous_external.external_module_roots = vec!["serde".into(), "borsh".into()];
    assert!(matches!(
        resolver.resolve(&ambiguous_external).await.unwrap(),
        ResolutionOutcome::Ambiguous {
            candidates,
            external_targets,
            basis: ResolutionBasis::Alias,
        } if candidates.is_empty() && external_targets == ["borsh::de::read", "serde::de::read"]
    ));
    let missing_request = ResolutionRequest::new(
        &external.id,
        cortexweave::domain::RelationshipTarget::QualifiedSymbol("missing::target".into()),
    );
    assert_eq!(
        resolver.resolve(&missing_request).await.unwrap(),
        ResolutionOutcome::Unresolved {
            target: "missing::target".into()
        }
    );

    // Stable registry identity survives body/offset changes, and removing a
    // target makes a later lookup unresolved instead of retaining a ghost node.
    let deletion = workspace(&storage, "deleted-target").await;
    let mut deletion_document = document(&storage, &deletion, "src/target.rs").await;
    let first = registry
        .reconcile_document(
            &deletion_document,
            "rust-structure-v1",
            &[symbol(
                "target::run",
                "run",
                "target::run",
                SymbolKind::Function,
            )],
        )
        .await
        .unwrap();
    let first_id = first
        .nodes
        .iter()
        .find(|node| node.qualified_name.as_deref() == Some("target::run"))
        .unwrap()
        .id
        .clone();
    deletion_document.content_revision = 2;
    let mut moved_symbol = symbol("target::run", "run", "target::run", SymbolKind::Function);
    moved_symbol.start_byte = 30;
    moved_symbol.end_byte = 45;
    let second = registry
        .reconcile_document(&deletion_document, "rust-structure-v1", &[moved_symbol])
        .await
        .unwrap();
    assert_eq!(
        second
            .nodes
            .iter()
            .find(|node| node.qualified_name.as_deref() == Some("target::run"))
            .unwrap()
            .id,
        first_id
    );
    let deletion_request = ResolutionRequest::new(
        &deletion.id,
        cortexweave::domain::RelationshipTarget::QualifiedSymbol("target::run".into()),
    );
    assert_resolved(
        resolver.resolve(&deletion_request).await.unwrap(),
        "target::run",
        ResolutionBasis::QualifiedName,
    );
    deletion_document.content_revision = 3;
    let removed = registry
        .reconcile_document(&deletion_document, "rust-structure-v1", &[])
        .await
        .unwrap();
    assert_eq!(removed.removed, 1);
    assert!(matches!(
        resolver.resolve(&deletion_request).await.unwrap(),
        ResolutionOutcome::Unresolved { .. }
    ));

    // A same-named symbol in another workspace cannot become a candidate, and
    // a cross-workspace source node ID is rejected rather than used as context.
    let isolated_left = workspace(&storage, "isolated-left").await;
    let isolated_right = workspace(&storage, "isolated-right").await;
    let right_document = document(&storage, &isolated_right, "src/only.rs").await;
    let right = registry
        .reconcile_document(
            &right_document,
            "rust-structure-v1",
            &[symbol(
                "only::run",
                "run",
                "only::run",
                SymbolKind::Function,
            )],
        )
        .await
        .unwrap();
    let right_node = right
        .nodes
        .iter()
        .find(|node| node.qualified_name.as_deref() == Some("only::run"))
        .unwrap();
    let isolated_request = ResolutionRequest::new(
        &isolated_left.id,
        cortexweave::domain::RelationshipTarget::QualifiedSymbol("only::run".into()),
    );
    assert!(matches!(
        resolver.resolve(&isolated_request).await.unwrap(),
        ResolutionOutcome::Unresolved { .. }
    ));
    let mut invalid_context = isolated_request;
    invalid_context.source_node_id = Some(right_node.id.clone());
    assert!(resolver.resolve(&invalid_context).await.is_err());
}

#[tokio::test]
async fn registry_rejects_cross_document_identity_collisions() {
    let storage = Arc::new(SqliteStorage::in_memory().await.unwrap());
    let registry = SymbolRegistry::new(Arc::clone(&storage));
    let workspace = workspace(&storage, "identity-collision").await;
    let first_document = document(&storage, &workspace, "src/first.rs").await;
    let second_document = document(&storage, &workspace, "src/second.rs").await;
    let colliding_symbol = symbol(
        "incorrectly-global-key",
        "run",
        "module::run",
        SymbolKind::Function,
    );
    registry
        .reconcile_document(
            &first_document,
            "rust-structure-v1",
            std::slice::from_ref(&colliding_symbol),
        )
        .await
        .unwrap();
    let error = registry
        .reconcile_document(&second_document, "rust-structure-v1", &[colliding_symbol])
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already owned"));
    assert_eq!(
        storage
            .graph_nodes_for_document(&workspace.id, &second_document.id)
            .await
            .unwrap(),
        Vec::new()
    );
}
