use std::path::Path;

use cortexweave::{
    domain::{AnalysisResult, GraphEdgeType, RelationshipTarget},
    parsing::{
        LanguageAnalyzer,
        languages::{
            CSharpAnalyzer, JavaScriptAnalyzer, PythonAnalyzer, RustAnalyzer, TypeScriptAnalyzer,
        },
    },
};

fn has_target(result: &AnalysisResult, edge: GraphEdgeType, target: RelationshipTarget) -> bool {
    result
        .relationships
        .iter()
        .any(|relationship| relationship.relationship == edge && relationship.target == target)
}

#[test]
fn mixed_language_module_graph_has_exact_import_dependencies() {
    let cases: [(&dyn LanguageAnalyzer, &str, &str, &[&str]); 4] = [
        (
            &RustAnalyzer,
            "src/service.rs",
            "use crate::shared::Engine;\npub use crate::exports::Thing;\npub fn run() {}",
            &["crate::shared::Engine", "crate::exports::Thing"],
        ),
        (
            &PythonAnalyzer,
            "service.py",
            "from package.worker import run\nimport requests as req\ndef service():\n    pass\n",
            &["package.worker", "requests"],
        ),
        (
            &TypeScriptAnalyzer,
            "service.ts",
            "import { run } from './shared';\nexport function service() {}\n",
            &["./shared"],
        ),
        (
            &CSharpAnalyzer,
            "Service.cs",
            "using Company.Shared;\nclass Service { void Run() {} }",
            &["Company.Shared"],
        ),
    ];

    for (analyzer, path, source, imports) in cases {
        let result = analyzer.analyze(Path::new(path), source).unwrap();
        for import in imports {
            let target = RelationshipTarget::ModulePath((*import).into());
            assert!(
                has_target(&result, GraphEdgeType::Imports, target.clone()),
                "{path} did not import {import}: {:#?}",
                result.relationships
            );
            assert!(
                has_target(&result, GraphEdgeType::DependsOn, target),
                "{path} did not depend on {import}: {:#?}",
                result.relationships
            );
        }
        assert!(
            result
                .relationships
                .iter()
                .any(|relationship| relationship.relationship == GraphEdgeType::Contains)
        );
        assert!(
            result
                .relationships
                .iter()
                .any(|relationship| relationship.relationship == GraphEdgeType::DeclaredIn)
        );
    }
}

#[test]
fn calls_and_references_are_direct_and_do_not_invent_member_dispatch() {
    let result = RustAnalyzer
        .analyze(
            Path::new("src/worker.rs"),
            "fn helper() {}\nfn worker() { helper(); object.helper(); }",
        )
        .unwrap();
    let direct = RelationshipTarget::QualifiedSymbol("helper".into());
    assert!(has_target(&result, GraphEdgeType::Calls, direct.clone()));
    assert!(has_target(&result, GraphEdgeType::References, direct));
    assert!(!result.relationships.iter().any(|relationship| {
        relationship.relationship == GraphEdgeType::Calls
            && relationship.target == RelationshipTarget::QualifiedSymbol("object.helper".into())
    }));
}

#[test]
fn inheritance_and_implementation_edges_are_limited_to_proven_syntax() {
    let rust = RustAnalyzer
        .analyze(
            Path::new("src/model.rs"),
            "trait Render {}\nstruct Widget;\nimpl Render for Widget {}",
        )
        .unwrap();
    assert!(has_target(
        &rust,
        GraphEdgeType::Implements,
        RelationshipTarget::QualifiedSymbol("Render".into()),
    ));

    let python = PythonAnalyzer
        .analyze(Path::new("model.py"), "class Child(Base):\n    pass\n")
        .unwrap();
    assert!(has_target(
        &python,
        GraphEdgeType::Extends,
        RelationshipTarget::QualifiedSymbol("Base".into()),
    ));

    let typescript = TypeScriptAnalyzer
        .analyze(
            Path::new("model.ts"),
            "class Child extends Base implements Printable, Serializable {}",
        )
        .unwrap();
    for (edge, target) in [
        (GraphEdgeType::Extends, "Base"),
        (GraphEdgeType::Implements, "Printable"),
        (GraphEdgeType::Implements, "Serializable"),
    ] {
        assert!(has_target(
            &typescript,
            edge,
            RelationshipTarget::QualifiedSymbol(target.into()),
        ));
    }

    let csharp = CSharpAnalyzer
        .analyze(Path::new("Model.cs"), "class Child : Base, IPrintable {}")
        .unwrap();
    for target in ["Base", "IPrintable"] {
        assert!(has_target(
            &csharp,
            GraphEdgeType::UsesType,
            RelationshipTarget::QualifiedSymbol(target.into()),
        ));
    }
    assert!(
        !csharp
            .relationships
            .iter()
            .any(|relationship| relationship.relationship == GraphEdgeType::Implements)
    );
}

#[test]
fn test_nodes_only_claim_direct_calls_with_high_confidence() {
    let rust = RustAnalyzer
        .analyze(
            Path::new("src/lib.rs"),
            "fn target() {}\n#[test]\nfn test_target() { target(); }",
        )
        .unwrap();
    let test_symbol = rust
        .symbols
        .iter()
        .find(|symbol| symbol.name == "test_target")
        .unwrap();
    assert_eq!(
        test_symbol
            .metadata
            .get("is_test")
            .and_then(serde_json::Value::as_bool),
        Some(true)
    );
    let test_edge = rust
        .relationships
        .iter()
        .find(|relationship| relationship.relationship == GraphEdgeType::Tests)
        .unwrap();
    assert_eq!(test_edge.from_key, test_symbol.stable_key);
    assert_eq!(test_edge.confidence, 0.9);
    assert_eq!(
        test_edge.target,
        RelationshipTarget::QualifiedSymbol("target".into())
    );
    assert_eq!(
        test_edge
            .metadata
            .get("test_relationship")
            .and_then(serde_json::Value::as_str),
        Some("direct_call_association")
    );
    assert_eq!(
        test_edge
            .metadata
            .get("test_certainty")
            .and_then(serde_json::Value::as_str),
        Some("likely")
    );

    let javascript = JavaScriptAnalyzer
        .analyze(
            Path::new("widget.test.js"),
            "function target() {}\ntest('runs target', () => { target(); });",
        )
        .unwrap();
    assert!(javascript.symbols.iter().any(|symbol| {
        symbol.name == "runs target"
            && symbol
                .metadata
                .get("is_test")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
    }));
    assert!(has_target(
        &javascript,
        GraphEdgeType::Tests,
        RelationshipTarget::QualifiedSymbol("target".into()),
    ));
}

#[test]
fn module_relationships_emit_analyzer_owned_local_candidates() {
    let typescript = TypeScriptAnalyzer
        .analyze(
            std::path::Path::new("src/service.ts"),
            "import { run } from './shared';\nexport function service() { run(); }",
        )
        .unwrap();
    let dependency = typescript
        .relationships
        .iter()
        .find(|relationship| relationship.relationship == GraphEdgeType::DependsOn)
        .unwrap();
    let aliases = dependency
        .metadata
        .get("resolution_aliases")
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert!(aliases.iter().any(|alias| {
        alias.get("alias").and_then(serde_json::Value::as_str) == Some("./shared")
            && alias.get("target").and_then(serde_json::Value::as_str) == Some("src/shared.ts")
    }));

    let compiled_extension = TypeScriptAnalyzer
        .analyze(
            std::path::Path::new("src/cli/main.ts"),
            "import { app } from '../app.js';\nexport { app };",
        )
        .unwrap();
    let dependency = compiled_extension
        .relationships
        .iter()
        .find(|relationship| relationship.relationship == GraphEdgeType::DependsOn)
        .unwrap();
    let aliases = dependency
        .metadata
        .get("resolution_aliases")
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert!(aliases.iter().any(|alias| {
        alias.get("target").and_then(serde_json::Value::as_str) == Some("src/app.ts")
    }));

    let python = PythonAnalyzer
        .analyze(
            std::path::Path::new("package/service.py"),
            "from .worker import run\ndef service():\n    return run()\n",
        )
        .unwrap();
    let dependency = python
        .relationships
        .iter()
        .find(|relationship| relationship.relationship == GraphEdgeType::DependsOn)
        .unwrap();
    let aliases = dependency
        .metadata
        .get("resolution_aliases")
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert!(aliases.iter().any(|alias| {
        alias.get("alias").and_then(serde_json::Value::as_str) == Some(".worker")
            && alias.get("target").and_then(serde_json::Value::as_str) == Some("package/worker.py")
    }));
}

#[test]
fn csharp_test_attributes_bind_only_to_their_declaration() {
    let result = CSharpAnalyzer
        .analyze(
            std::path::Path::new("Tests.cs"),
            "class Tests {\n  [Fact]\n  void ActualTest() {}\n  void Helper() {}\n}\n",
        )
        .unwrap();
    let is_test = |name: &str| {
        result
            .symbols
            .iter()
            .find(|symbol| symbol.name == name)
            .and_then(|symbol| symbol.metadata.get("is_test"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
    };
    assert!(is_test("ActualTest"));
    assert!(!is_test("Helper"));
}
