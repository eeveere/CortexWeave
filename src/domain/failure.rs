use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{CortexError, Result};

use super::EvidenceContract;

pub const FAILURE_SIGNATURE_ENCODING_VERSION: u16 = 2;
pub const MAX_FAILURE_COMPONENTS: usize = 16;
pub const MAX_FAILURE_COMPONENT_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureDomain {
    RustCompiler,
    CargoTest,
    RegisteredTool,
}

impl FailureDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RustCompiler => "rust_compiler",
            Self::CargoTest => "cargo_test",
            Self::RegisteredTool => "registered_tool",
        }
    }
}

/// States what kind of equality a normalized failure identity can support.
///
/// `CompatibleOnly` is deliberately the serde default so signatures written
/// before this distinction existed can never acquire exact authority merely
/// by being deserialized by newer code.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureIdentityCapability {
    Exact,
    #[default]
    CompatibleOnly,
}

impl FailureIdentityCapability {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::CompatibleOnly => "compatible_only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureScope {
    pub workspace_id: String,
    pub language: Option<String>,
    pub tool: String,
    pub target: Option<String>,
    pub path: Option<String>,
    /// Reserved for deterministic structural provenance. Phase 3 does not
    /// populate it from a producer payload.
    pub symbol_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureSignature {
    pub domain: FailureDomain,
    #[serde(default)]
    pub identity_capability: FailureIdentityCapability,
    pub components: BTreeMap<String, String>,
    pub normalizer_id: String,
    pub normalizer_version: String,
    pub scope: FailureScope,
    pub encoding_version: u16,
    /// Length-delimited canonical v1 representation, suitable for exact
    /// comparison and future durable storage.
    pub canonical_encoding: String,
    /// Domain-separated BLAKE3 digest of canonical_encoding.
    pub normalized_key: String,
}

impl FailureSignature {
    pub fn new(
        domain: FailureDomain,
        identity_capability: FailureIdentityCapability,
        components: BTreeMap<String, String>,
        normalizer_id: impl Into<String>,
        normalizer_version: impl Into<String>,
        scope: FailureScope,
    ) -> Result<Self> {
        let normalizer_id = normalizer_id.into();
        let normalizer_version = normalizer_version.into();
        validate_identifier(&normalizer_id, "normalizer ID")?;
        validate_identifier(&normalizer_version, "normalizer version")?;
        validate_scope(&scope)?;
        if components.is_empty() || components.len() > MAX_FAILURE_COMPONENTS {
            return Err(CortexError::Analysis(format!(
                "failure signatures require between 1 and {MAX_FAILURE_COMPONENTS} components"
            )));
        }
        for (name, value) in &components {
            if !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            {
                return Err(CortexError::Analysis(
                    "failure component names use lowercase ASCII letters, digits, and underscores"
                        .into(),
                ));
            }
            validate_component(value, "failure component")?;
        }
        let canonical_encoding = canonical_encoding(
            domain,
            identity_capability,
            &components,
            &normalizer_id,
            &normalizer_version,
            &scope,
        );
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"cortexweave.failure_signature.key.v2\0");
        hasher.update(canonical_encoding.as_bytes());
        Ok(Self {
            domain,
            identity_capability,
            components,
            normalizer_id,
            normalizer_version,
            scope,
            encoding_version: FAILURE_SIGNATURE_ENCODING_VERSION,
            normalized_key: hasher.finalize().to_hex().to_string(),
            canonical_encoding,
        })
    }

    pub const fn is_exact_capable(&self) -> bool {
        matches!(self.identity_capability, FailureIdentityCapability::Exact)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationSubjectKind {
    Workspace,
    Package,
    Target,
    Test,
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct VerificationSubject {
    pub kind: VerificationSubjectKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifierRule {
    pub id: String,
    pub version: String,
    pub evidence_contract: EvidenceContract,
    pub declared_verifier_id: String,
    pub declared_verifier_version: String,
    pub required_tool: String,
    pub required_operation: String,
    pub allowed_subject_kinds: BTreeSet<VerificationSubjectKind>,
}

impl VerifierRule {
    pub fn cargo_check_v1() -> Self {
        Self {
            id: "cortexweave.cargo_check".into(),
            version: "1".into(),
            evidence_contract: EvidenceContract::GenericVerifierResult,
            declared_verifier_id: "cargo.check".into(),
            declared_verifier_version: "1".into(),
            required_tool: "cargo".into(),
            required_operation: "check".into(),
            allowed_subject_kinds: [
                VerificationSubjectKind::Workspace,
                VerificationSubjectKind::Package,
                VerificationSubjectKind::Target,
            ]
            .into_iter()
            .collect(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        for (label, value) in [
            ("verifier rule ID", &self.id),
            ("verifier rule version", &self.version),
            ("declared verifier ID", &self.declared_verifier_id),
            ("declared verifier version", &self.declared_verifier_version),
            ("required tool", &self.required_tool),
            ("required operation", &self.required_operation),
        ] {
            validate_identifier(value, label)?;
        }
        if self.evidence_contract != EvidenceContract::GenericVerifierResult {
            return Err(CortexError::Configuration(
                "v1 verifier rules support generic verifier evidence only".into(),
            ));
        }
        if self.allowed_subject_kinds.is_empty() {
            return Err(CortexError::Configuration(
                "verifier rules require at least one subject kind".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureNormalizationDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FailureNormalization {
    pub signature: FailureSignature,
    pub diagnostics: Vec<FailureNormalizationDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedFailureNormalization {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FailureNormalizationResult {
    Normalized {
        normalization: Box<FailureNormalization>,
    },
    Unsupported {
        reason: UnsupportedFailureNormalization,
    },
}

fn canonical_encoding(
    domain: FailureDomain,
    identity_capability: FailureIdentityCapability,
    components: &BTreeMap<String, String>,
    normalizer_id: &str,
    normalizer_version: &str,
    scope: &FailureScope,
) -> String {
    let mut encoded =
        format!("cortexweave.failure_signature.v{FAILURE_SIGNATURE_ENCODING_VERSION};");
    append_field(&mut encoded, "domain", domain.as_str());
    append_field(
        &mut encoded,
        "identity_capability",
        identity_capability.as_str(),
    );
    append_field(&mut encoded, "normalizer_id", normalizer_id);
    append_field(&mut encoded, "normalizer_version", normalizer_version);
    append_field(&mut encoded, "workspace_id", &scope.workspace_id);
    append_optional_field(&mut encoded, "language", scope.language.as_deref());
    append_field(&mut encoded, "tool", &scope.tool);
    append_optional_field(&mut encoded, "target", scope.target.as_deref());
    append_optional_field(&mut encoded, "path", scope.path.as_deref());
    append_optional_field(&mut encoded, "symbol_key", scope.symbol_key.as_deref());
    for (name, value) in components {
        append_field(&mut encoded, &format!("component.{name}"), value);
    }
    encoded
}

fn append_field(encoded: &mut String, name: &str, value: &str) {
    encoded.push_str(name);
    encoded.push('=');
    encoded.push_str(&value.len().to_string());
    encoded.push(':');
    encoded.push_str(value);
    encoded.push(';');
}

fn append_optional_field(encoded: &mut String, name: &str, value: Option<&str>) {
    match value {
        Some(value) => append_field(encoded, name, value),
        None => append_field(encoded, name, ""),
    }
}

fn validate_scope(scope: &FailureScope) -> Result<()> {
    validate_identifier(&scope.workspace_id, "workspace ID")?;
    validate_identifier(&scope.tool, "tool")?;
    for (label, value) in [
        ("language", scope.language.as_deref()),
        ("target", scope.target.as_deref()),
        ("symbol key", scope.symbol_key.as_deref()),
    ] {
        if let Some(value) = value {
            validate_component(value, label)?;
        }
    }
    if let Some(path) = scope.path.as_deref() {
        validate_workspace_relative_path(path)?;
    }
    Ok(())
}

fn validate_workspace_relative_path(path: &str) -> Result<()> {
    if path.trim().is_empty()
        || path.len() > MAX_FAILURE_COMPONENT_BYTES
        || path.contains('\0')
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains(':')
        || path.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(CortexError::Analysis(
            "failure signature paths must be bounded workspace-relative paths".into(),
        ));
    }
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > MAX_FAILURE_COMPONENT_BYTES || value.contains('\0')
    {
        return Err(CortexError::Analysis(format!(
            "{label} must be non-empty, contain no NUL, and be at most {MAX_FAILURE_COMPONENT_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_component(value: &str, label: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > MAX_FAILURE_COMPONENT_BYTES || value.contains('\0')
    {
        return Err(CortexError::Analysis(format!(
            "{label} must be non-empty, contain no NUL, and be at most {MAX_FAILURE_COMPONENT_BYTES} bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> FailureScope {
        FailureScope {
            workspace_id: "workspace-a".into(),
            language: Some("rust".into()),
            tool: "rustc".into(),
            target: Some("core".into()),
            path: Some("src/lib.rs".into()),
            symbol_key: None,
        }
    }

    #[test]
    fn canonical_encoding_sorts_components_and_preserves_field_boundaries() {
        let first = FailureSignature::new(
            FailureDomain::RustCompiler,
            FailureIdentityCapability::Exact,
            BTreeMap::from([("a".into(), "bc".into()), ("d".into(), "e".into())]),
            "normalizer",
            "1",
            scope(),
        )
        .unwrap();
        let reordered = FailureSignature::new(
            FailureDomain::RustCompiler,
            FailureIdentityCapability::Exact,
            BTreeMap::from([("d".into(), "e".into()), ("a".into(), "bc".into())]),
            "normalizer",
            "1",
            scope(),
        )
        .unwrap();
        let distinct = FailureSignature::new(
            FailureDomain::RustCompiler,
            FailureIdentityCapability::Exact,
            BTreeMap::from([("ab".into(), "c".into()), ("d".into(), "e".into())]),
            "normalizer",
            "1",
            scope(),
        )
        .unwrap();
        assert_eq!(first.canonical_encoding, reordered.canonical_encoding);
        assert_eq!(first.normalized_key, reordered.normalized_key);
        assert_ne!(first.normalized_key, distinct.normalized_key);
    }

    #[test]
    fn signatures_reject_absolute_scope_paths() {
        let mut unsafe_scope = scope();
        unsafe_scope.path = Some("C:/temporary/lib.rs".into());
        assert!(
            FailureSignature::new(
                FailureDomain::RustCompiler,
                FailureIdentityCapability::Exact,
                BTreeMap::from([("diagnostic_code".into(), "E0308".into())]),
                "normalizer",
                "1",
                unsafe_scope,
            )
            .is_err()
        );
    }

    #[test]
    fn identity_capability_is_canonical_and_legacy_json_is_compatible_only() {
        let components = BTreeMap::from([("diagnostic_code".into(), "E0308".into())]);
        let exact = FailureSignature::new(
            FailureDomain::RustCompiler,
            FailureIdentityCapability::Exact,
            components.clone(),
            "normalizer",
            "2",
            scope(),
        )
        .unwrap();
        let compatible = FailureSignature::new(
            FailureDomain::RustCompiler,
            FailureIdentityCapability::CompatibleOnly,
            components,
            "normalizer",
            "2",
            scope(),
        )
        .unwrap();
        assert!(exact.is_exact_capable());
        assert!(!compatible.is_exact_capable());
        assert_ne!(exact.normalized_key, compatible.normalized_key);

        let mut legacy = serde_json::to_value(&exact).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("identity_capability");
        let decoded: FailureSignature = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            decoded.identity_capability,
            FailureIdentityCapability::CompatibleOnly
        );
        assert!(!decoded.is_exact_capable());
    }
}
