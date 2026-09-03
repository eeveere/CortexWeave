use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use crate::{
    CortexError, Result,
    domain::{
        Experience, ExperienceEligibilityExplanation, ExperienceEligibilityInclusion,
        ExperienceExplanation, ExperienceLifecycle, ExperienceNormalContextEligibility,
        ExperienceNormalContextReason, ExperienceReferenceResolution, ExperienceSearchExplanation,
        ExperienceSearchHit, ExperienceSearchRequest, ExperienceSearchScores,
        HistoricalGraphReferenceStatus, HistoricalReferenceStatus,
        MAX_EXPERIENCE_SEARCH_CANDIDATES, MAX_EXPERIENCE_SEARCH_LIMIT,
    },
    storage::{ExperienceCandidateQuery, ExperienceSearchCandidates, SqliteStorage},
};

pub struct ExperienceSearchService {
    storage: Arc<SqliteStorage>,
}

impl ExperienceSearchService {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    pub async fn search(
        &self,
        request: &ExperienceSearchRequest,
    ) -> Result<Vec<ExperienceSearchHit>> {
        validate_request(request)?;
        let components = requested_components(request);
        let lexical_query = request.query.as_deref().and_then(fts_query);
        let candidates = self
            .storage
            .experience_search_candidates(ExperienceCandidateQuery {
                workspace_id: &request.workspace_id,
                exact_failure_key: request
                    .exact_failure_signature
                    .as_ref()
                    .filter(|value| value.is_exact_capable())
                    .map(|value| value.normalized_key.as_str()),
                components: &components,
                lexical_query: lexical_query.as_deref(),
                path: request.path.as_deref(),
                graph_stable_key: request.graph_stable_key.as_deref(),
                limit: MAX_EXPERIENCE_SEARCH_CANDIDATES,
            })
            .await?;
        let exact: BTreeSet<_> = candidates.exact_signature.iter().cloned().collect();
        let lexical: BTreeSet<_> = candidates.lexical.iter().cloned().collect();
        let path: BTreeSet<_> = candidates.path.iter().cloned().collect();
        let graph: BTreeSet<_> = candidates.graph_stable_key.iter().cloned().collect();
        let include_recent = request.exact_failure_signature.is_none()
            && components.is_empty()
            && lexical_query.is_none()
            && request.path.is_none()
            && request.graph_stable_key.is_none();
        let ids = merged_candidate_ids(&candidates, include_recent);

        let mut hits = Vec::new();
        for id in ids {
            let Some(record) = self.storage.experience(&request.workspace_id, &id).await? else {
                continue;
            };
            let lifecycle = self
                .storage
                .experience_lifecycle(&request.workspace_id, &id)
                .await?;
            if !matches_request(
                &record.experience,
                &record.code_snapshots,
                &record.graph_snapshots,
                lifecycle,
                request,
                &request.compatible_components,
            ) {
                continue;
            }
            let matched_components = matching_components(&record.experience, &components);
            let scores = score(
                exact.contains(&id),
                matched_components.len(),
                lexical.contains(&id),
                path.contains(&id),
                graph.contains(&id),
            );
            hits.push(ExperienceSearchHit {
                experience: record.experience,
                lifecycle,
                explanation: ExperienceSearchExplanation {
                    scores,
                    matched_components,
                    lexical_match: lexical.contains(&id),
                    eligibility: eligibility_explanation(lifecycle, request),
                    normal_context: normal_context_eligibility(lifecycle),
                },
            });
        }
        hits.sort_by(|left, right| {
            right
                .explanation
                .scores
                .exact_signature
                .cmp(&left.explanation.scores.exact_signature)
                .then_with(|| {
                    right
                        .explanation
                        .matched_components
                        .len()
                        .cmp(&left.explanation.matched_components.len())
                })
                .then_with(|| {
                    authority_rank(&right.experience).cmp(&authority_rank(&left.experience))
                })
                .then_with(|| {
                    right
                        .explanation
                        .scores
                        .total
                        .cmp(&left.explanation.scores.total)
                })
                .then_with(|| left.experience.id.cmp(&right.experience.id))
        });
        hits.truncate(request.limit);
        Ok(hits)
    }

    pub async fn get(
        &self,
        workspace_id: &str,
        experience_id: &str,
    ) -> Result<Option<ExperienceExplanation>> {
        let Some(record) = self.storage.experience(workspace_id, experience_id).await? else {
            return Ok(None);
        };
        let assessments = self
            .storage
            .experience_assessments(workspace_id, experience_id)
            .await?;
        let lifecycle = self
            .storage
            .experience_lifecycle(workspace_id, experience_id)
            .await?;
        let references = self.references(&record).await?;
        Ok(Some(ExperienceExplanation {
            record,
            assessments,
            lifecycle,
            normal_context: normal_context_eligibility(lifecycle),
            references,
            search: None,
        }))
    }

    async fn references(
        &self,
        record: &crate::domain::ExperienceRecord,
    ) -> Result<Vec<ExperienceReferenceResolution>> {
        let graph_is_current = self
            .storage
            .workspace_graph_revision(&record.experience.workspace_id)
            .await?
            .is_some_and(|revision| revision.is_current());
        let mut references = Vec::new();
        for snapshot in &record.code_snapshots {
            let current_document = self
                .storage
                .find_document(&record.experience.workspace_id, &snapshot.relative_path)
                .await?;
            let document_status = match &current_document {
                None => HistoricalReferenceStatus::Deleted,
                Some(document)
                    if document.content_revision == snapshot.document_content_revision
                        && document.content_hash == snapshot.document_content_hash =>
                {
                    HistoricalReferenceStatus::Current
                }
                Some(_) => HistoricalReferenceStatus::ContentChanged,
            };
            let graph_snapshots = record
                .graph_snapshots
                .iter()
                .filter(|graph| graph.code_snapshot_ordinal == snapshot.ordinal)
                .collect::<Vec<_>>();
            if graph_snapshots.is_empty() {
                references.push(ExperienceReferenceResolution {
                    snapshot_ordinal: snapshot.ordinal,
                    graph_snapshot_ordinal: None,
                    relative_path: snapshot.relative_path.clone(),
                    historical_document_content_revision: snapshot.document_content_revision,
                    historical_document_content_hash: snapshot.document_content_hash.clone(),
                    current_document_content_revision: current_document
                        .as_ref()
                        .map(|value| value.content_revision),
                    current_document_content_hash: current_document
                        .as_ref()
                        .map(|value| value.content_hash.clone()),
                    status: document_status,
                    historical_graph_stable_key: None,
                    graph_status: HistoricalGraphReferenceStatus::NotCaptured,
                    current_graph_node_id: None,
                });
                continue;
            }
            for graph_snapshot in graph_snapshots {
                let current_node = if graph_is_current {
                    self.storage
                        .graph_node_by_stable_key(
                            &record.experience.workspace_id,
                            &graph_snapshot.node_stable_key,
                        )
                        .await?
                } else {
                    None
                };
                let graph_status = if !graph_is_current {
                    HistoricalGraphReferenceStatus::Unavailable
                } else if current_node.is_some() {
                    HistoricalGraphReferenceStatus::Current
                } else {
                    HistoricalGraphReferenceStatus::Missing
                };
                references.push(ExperienceReferenceResolution {
                    snapshot_ordinal: snapshot.ordinal,
                    graph_snapshot_ordinal: Some(graph_snapshot.ordinal),
                    relative_path: snapshot.relative_path.clone(),
                    historical_document_content_revision: snapshot.document_content_revision,
                    historical_document_content_hash: snapshot.document_content_hash.clone(),
                    current_document_content_revision: current_document
                        .as_ref()
                        .map(|value| value.content_revision),
                    current_document_content_hash: current_document
                        .as_ref()
                        .map(|value| value.content_hash.clone()),
                    status: document_status,
                    historical_graph_stable_key: Some(graph_snapshot.node_stable_key.clone()),
                    graph_status,
                    current_graph_node_id: current_node.map(|value| value.id),
                });
            }
        }
        Ok(references)
    }
}

fn authority_rank(experience: &Experience) -> (u8, u8, u8) {
    let verification = match experience.verification.status {
        crate::domain::VerificationStatus::VerifiedPassed => 2,
        crate::domain::VerificationStatus::VerifiedFailed => 1,
        _ => 0,
    };
    let strength = match experience.evidence_strength.strength {
        crate::domain::EvidenceStrength::Strong => 2,
        crate::domain::EvidenceStrength::Moderate => 1,
        _ => 0,
    };
    let outcome = match experience.outcome {
        crate::domain::ExperienceOutcome::Success => 3,
        crate::domain::ExperienceOutcome::PartialSuccess => 2,
        crate::domain::ExperienceOutcome::Failure => 1,
        crate::domain::ExperienceOutcome::Inconclusive
        | crate::domain::ExperienceOutcome::Abandoned => 0,
    };
    (verification, strength, outcome)
}

fn merged_candidate_ids(
    candidates: &ExperienceSearchCandidates,
    include_recent: bool,
) -> BTreeSet<String> {
    candidates
        .exact_signature
        .iter()
        .chain(candidates.compatible_components.iter())
        .chain(candidates.lexical.iter())
        .chain(candidates.path.iter())
        .chain(candidates.graph_stable_key.iter())
        .chain(
            include_recent
                .then_some(&candidates.recent)
                .into_iter()
                .flatten(),
        )
        .cloned()
        .collect()
}

fn validate_request(request: &ExperienceSearchRequest) -> Result<()> {
    if request.workspace_id.trim().is_empty() || request.workspace_id.len() > 256 {
        return Err(CortexError::Analysis(
            "experience search requires a bounded workspace ID".into(),
        ));
    }
    if request.limit == 0 || request.limit > MAX_EXPERIENCE_SEARCH_LIMIT {
        return Err(CortexError::Analysis(format!(
            "experience search limit must be between 1 and {MAX_EXPERIENCE_SEARCH_LIMIT}"
        )));
    }
    if request
        .query
        .as_ref()
        .is_some_and(|value| value.len() > 1024 || fts_query(value).is_none())
        || request
            .path
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 512)
        || request
            .graph_stable_key
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.len() > 512)
    {
        return Err(CortexError::Analysis(
            "experience search text filters must be bounded and non-empty".into(),
        ));
    }
    if request.created_after.is_some()
        && request
            .created_before
            .is_some_and(|before| request.created_after.expect("checked").gt(&before))
    {
        return Err(CortexError::Analysis(
            "experience search time bounds are inverted".into(),
        ));
    }
    if request.compatible_components.len() > crate::domain::MAX_FAILURE_COMPONENTS {
        return Err(CortexError::Analysis(
            "experience search has too many compatible components".into(),
        ));
    }
    for (name, value) in &request.compatible_components {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            || value.trim().is_empty()
            || value.len() > crate::domain::MAX_FAILURE_COMPONENT_BYTES
        {
            return Err(CortexError::Analysis(
                "experience search components must use canonical bounded names and values".into(),
            ));
        }
    }
    if let Some(signature) = &request.exact_failure_signature {
        if signature.scope.workspace_id != request.workspace_id {
            return Err(CortexError::Analysis(
                "exact failure signature belongs to a different workspace".into(),
            ));
        }
        let rebuilt = crate::domain::FailureSignature::new(
            signature.domain,
            signature.identity_capability,
            signature.components.clone(),
            signature.normalizer_id.clone(),
            signature.normalizer_version.clone(),
            signature.scope.clone(),
        )?;
        if &rebuilt != signature {
            return Err(CortexError::Analysis(
                "exact failure signature is not canonically self-consistent".into(),
            ));
        }
        if request.compatible_components.iter().any(|(name, value)| {
            signature
                .components
                .get(name)
                .is_some_and(|canonical| canonical != value)
        }) {
            return Err(CortexError::Analysis(
                "compatible components conflict with the exact failure signature".into(),
            ));
        }
    }
    Ok(())
}

fn requested_components(request: &ExperienceSearchRequest) -> BTreeMap<String, String> {
    let mut components = request.compatible_components.clone();
    if let Some(signature) = &request.exact_failure_signature {
        for (name, value) in &signature.components {
            components
                .entry(name.clone())
                .or_insert_with(|| value.clone());
        }
    }
    components
}

fn matching_components(
    experience: &Experience,
    requested: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    experience
        .failure_signature
        .as_ref()
        .map(|signature| {
            requested
                .iter()
                .filter(|(name, value)| signature.components.get(*name) == Some(*value))
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn matches_request(
    experience: &Experience,
    snapshots: &[crate::domain::ExperienceCodeSnapshot],
    graph_snapshots: &[crate::domain::ExperienceGraphSnapshot],
    lifecycle: ExperienceLifecycle,
    request: &ExperienceSearchRequest,
    components: &BTreeMap<String, String>,
) -> bool {
    if !request.outcomes.is_empty() && !request.outcomes.contains(&experience.outcome)
        || !request.strengths.is_empty()
            && !request
                .strengths
                .contains(&experience.evidence_strength.strength)
        || !(if request.lifecycles.is_empty() {
            request.include_historical || lifecycle == ExperienceLifecycle::Active
        } else {
            request.lifecycles.contains(&lifecycle)
        })
        || request
            .created_after
            .is_some_and(|after| experience.created_at < after)
        || request
            .created_before
            .is_some_and(|before| experience.created_at > before)
    {
        return false;
    }
    if let Some(path) = &request.path
        && experience
            .failure_signature
            .as_ref()
            .and_then(|value| value.scope.path.as_ref())
            != Some(path)
        && !snapshots
            .iter()
            .any(|snapshot| &snapshot.relative_path == path)
    {
        return false;
    }
    if let Some(key) = &request.graph_stable_key
        && !graph_snapshots
            .iter()
            .any(|snapshot| &snapshot.node_stable_key == key)
    {
        return false;
    }
    if !components.is_empty()
        && matching_components(experience, components).len() != components.len()
    {
        return false;
    }
    true
}

fn eligibility_explanation(
    lifecycle: ExperienceLifecycle,
    request: &ExperienceSearchRequest,
) -> ExperienceEligibilityExplanation {
    let inclusion = if !request.lifecycles.is_empty() {
        ExperienceEligibilityInclusion::ExplicitLifecycle
    } else if request.include_historical {
        ExperienceEligibilityInclusion::HistoricalInspection
    } else {
        ExperienceEligibilityInclusion::ActiveDefault
    };
    ExperienceEligibilityExplanation {
        lifecycle,
        inclusion,
    }
}

fn normal_context_eligibility(
    lifecycle: ExperienceLifecycle,
) -> ExperienceNormalContextEligibility {
    let reason = match lifecycle {
        ExperienceLifecycle::Active => ExperienceNormalContextReason::ActiveLifecycle,
        ExperienceLifecycle::Disputed => ExperienceNormalContextReason::DisputedLifecycle,
        ExperienceLifecycle::Refuted => ExperienceNormalContextReason::RefutedLifecycle,
        ExperienceLifecycle::Superseded => ExperienceNormalContextReason::SupersededLifecycle,
    };
    ExperienceNormalContextEligibility {
        eligible: lifecycle == ExperienceLifecycle::Active,
        lifecycle,
        reason,
    }
}

fn score(
    exact: bool,
    component_matches: usize,
    lexical: bool,
    path: bool,
    graph: bool,
) -> ExperienceSearchScores {
    let mut scores = ExperienceSearchScores {
        exact_signature: u32::from(exact) * 1000,
        compatible_components: (component_matches.min(16) as u32) * 20,
        lexical: u32::from(lexical) * 40,
        exact_path: u32::from(path) * 60,
        graph_stable_key: u32::from(graph) * 60,
        recency: 0,
        total: 0,
    };
    scores.total = scores.exact_signature
        + scores.compatible_components
        + scores.lexical
        + scores.exact_path
        + scores.graph_stable_key
        + scores.recency;
    scores
}

fn fts_query(value: &str) -> Option<String> {
    let tokens = value
        .split(|character: char| {
            !character.is_alphanumeric() && character != '_' && character != ':'
        })
        .filter(|token| !token.is_empty())
        .take(16)
        .collect::<Vec<_>>();
    (!tokens.is_empty()).then(|| {
        tokens
            .into_iter()
            .map(|token| format!("\"{token}\""))
            .collect::<Vec<_>>()
            .join(" AND ")
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::domain::{ExperienceSearchRequest, FailureDomain, FailureScope, FailureSignature};
    use crate::storage::ExperienceSearchCandidates;

    use super::{fts_query, merged_candidate_ids, score, validate_request};

    #[test]
    fn exact_signature_score_dominates_all_weaker_sources() {
        assert!(
            score(true, 0, false, false, false).total > score(false, 16, true, true, true).total
        );
    }

    #[test]
    fn fts_query_removes_operators_and_keeps_bounded_terms() {
        assert_eq!(
            fts_query("observed:target OR \"unsafe\""),
            Some("\"observed:target\" AND \"OR\" AND \"unsafe\"".into())
        );
        assert_eq!(fts_query("***"), None);
    }

    #[test]
    fn merged_bound_never_discards_exact_candidates_by_id_order() {
        let candidates = ExperienceSearchCandidates {
            exact_signature: vec!["z-exact".into()],
            lexical: (0..200).map(|index| format!("a-{index:03}")).collect(),
            ..ExperienceSearchCandidates::default()
        };
        let merged = merged_candidate_ids(&candidates, false);
        assert_eq!(merged.len(), 201);
        assert!(merged.contains("z-exact"));
    }

    #[test]
    fn request_rejects_unsearchable_text_and_mutated_signature_identity() {
        let signature = FailureSignature::new(
            FailureDomain::RustCompiler,
            crate::domain::FailureIdentityCapability::Exact,
            BTreeMap::from([("diagnostic_code".into(), "E0308".into())]),
            "rust.compiler",
            "1",
            FailureScope {
                workspace_id: "workspace".into(),
                language: Some("rust".into()),
                tool: "rustc".into(),
                target: Some("core".into()),
                path: Some("src/lib.rs".into()),
                symbol_key: None,
            },
        )
        .unwrap();
        let mut request = ExperienceSearchRequest {
            workspace_id: "workspace".into(),
            query: Some("***".into()),
            exact_failure_signature: Some(signature),
            compatible_components: BTreeMap::new(),
            path: None,
            graph_stable_key: None,
            outcomes: Vec::new(),
            strengths: Vec::new(),
            lifecycles: Vec::new(),
            include_historical: false,
            created_after: None,
            created_before: None,
            limit: 10,
        };
        assert!(validate_request(&request).is_err());
        request.query = None;
        request
            .exact_failure_signature
            .as_mut()
            .unwrap()
            .normalized_key = "0".repeat(64);
        assert!(validate_request(&request).is_err());
    }
}
