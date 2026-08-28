use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::{MemoryOrigin, MemoryRecord, MemoryTrust, SourceSegment};

const DUPLICATE_OVERLAP_THRESHOLD: f32 = 0.8;
const SEGMENT_SUPPORTED_TOKEN_THRESHOLD: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryTrustReviewRequest {
    pub workspace_id: String,
    pub memory_id: String,
    pub new_trust: MemoryTrust,
    pub reviewed_by: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySupersessionReviewRequest {
    pub workspace_id: String,
    pub superseded_memory_id: String,
    pub superseding_memory_id: String,
    pub reviewed_by: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryConsolidationRequest {
    pub workspace_id: String,
    pub memory_ids: Vec<String>,
}

impl MemoryConsolidationRequest {
    pub fn new(
        workspace_id: impl Into<String>,
        memory_ids: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            workspace_id: workspace_id.into(),
            memory_ids: memory_ids.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryContradiction {
    pub claim_key: String,
    pub older_value: Value,
    pub newer_value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedMemorySupersession {
    pub superseded_memory_id: String,
    pub superseding_memory_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryConsolidationProposal {
    pub older_memory_id: String,
    pub newer_memory_id: String,
    pub older_origin: MemoryOrigin,
    pub newer_origin: MemoryOrigin,
    pub older_trust: MemoryTrust,
    pub newer_trust: MemoryTrust,
    pub shared_token_count: usize,
    pub duplicate_token_ratio: f32,
    pub source_overlap_ratio: f32,
    pub contradiction: Option<MemoryContradiction>,
    pub proposed_supersession: Option<ProposedMemorySupersession>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryConsolidationReport {
    pub workspace_id: String,
    pub analyzed_memory_count: usize,
    pub analyzed_pair_count: usize,
    pub proposals: Vec<MemoryConsolidationProposal>,
}

pub(crate) fn analyze_memory_consolidation(
    workspace_id: &str,
    mut memories: Vec<MemoryRecord>,
) -> MemoryConsolidationReport {
    memories.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let analyzed_pair_count = memories
        .len()
        .saturating_mul(memories.len().saturating_sub(1))
        / 2;
    let mut proposals = Vec::new();
    for older_index in 0..memories.len() {
        for newer in &memories[older_index + 1..] {
            let older = &memories[older_index];
            if let Some(proposal) = consolidation_proposal(older, newer) {
                proposals.push(proposal);
            }
        }
    }
    MemoryConsolidationReport {
        workspace_id: workspace_id.into(),
        analyzed_memory_count: memories.len(),
        analyzed_pair_count,
        proposals,
    }
}

fn consolidation_proposal(
    older: &MemoryRecord,
    newer: &MemoryRecord,
) -> Option<MemoryConsolidationProposal> {
    let (shared_token_count, duplicate_token_ratio) = token_overlap(&older.content, &newer.content);
    let source_overlap_ratio = source_overlap(&older.source_segments, &newer.source_segments);
    let contradiction = match (&older.claim, &newer.claim) {
        (Some(older_claim), Some(newer_claim))
            if older_claim.key == newer_claim.key && older_claim.value != newer_claim.value =>
        {
            Some(MemoryContradiction {
                claim_key: older_claim.key.clone(),
                older_value: older_claim.value.clone(),
                newer_value: newer_claim.value.clone(),
            })
        }
        _ => None,
    };
    let duplicate = duplicate_token_ratio >= DUPLICATE_OVERLAP_THRESHOLD
        || (source_overlap_ratio >= DUPLICATE_OVERLAP_THRESHOLD
            && duplicate_token_ratio >= SEGMENT_SUPPORTED_TOKEN_THRESHOLD);
    if !duplicate && contradiction.is_none() {
        return None;
    }

    let mut reasons = Vec::new();
    if duplicate_token_ratio >= DUPLICATE_OVERLAP_THRESHOLD {
        reasons.push("normalized token overlap meets the duplicate threshold".into());
    }
    if source_overlap_ratio >= DUPLICATE_OVERLAP_THRESHOLD {
        reasons.push("source segment overlap meets the duplicate threshold".into());
    }
    if contradiction.is_some() {
        reasons.push("the same structured claim key has different values".into());
    }
    let proposed_supersession =
        (newer.trust == MemoryTrust::Trusted).then(|| ProposedMemorySupersession {
            superseded_memory_id: older.id.clone(),
            superseding_memory_id: newer.id.clone(),
        });
    if proposed_supersession.is_none() {
        reasons.push("the newer memory is not trusted, so supersession is not proposed".into());
    }

    Some(MemoryConsolidationProposal {
        older_memory_id: older.id.clone(),
        newer_memory_id: newer.id.clone(),
        older_origin: older.origin,
        newer_origin: newer.origin,
        older_trust: older.trust,
        newer_trust: newer.trust,
        shared_token_count,
        duplicate_token_ratio,
        source_overlap_ratio,
        contradiction,
        proposed_supersession,
        reasons,
    })
}

fn token_overlap(left: &str, right: &str) -> (usize, f32) {
    let left = token_counts(left);
    let right = token_counts(right);
    let left_count = left.values().sum::<usize>();
    let right_count = right.values().sum::<usize>();
    let shared = left
        .iter()
        .map(|(token, count)| count.min(right.get(token).unwrap_or(&0)))
        .sum::<usize>();
    let denominator = left_count.min(right_count);
    let ratio = if denominator == 0 {
        0.0
    } else {
        shared as f32 / denominator as f32
    };
    (shared, ratio)
}

fn token_counts(content: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for token in content
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter(|token| !token.is_empty())
    {
        *counts.entry(token.to_lowercase()).or_default() += 1;
    }
    counts
}

fn source_overlap(left: &[SourceSegment], right: &[SourceSegment]) -> f32 {
    let left_bytes = left
        .iter()
        .map(|segment| segment.end_byte.saturating_sub(segment.start_byte))
        .sum::<u64>();
    let right_bytes = right
        .iter()
        .map(|segment| segment.end_byte.saturating_sub(segment.start_byte))
        .sum::<u64>();
    let denominator = left_bytes.min(right_bytes);
    if denominator == 0 {
        return 0.0;
    }
    let overlap = left
        .iter()
        .flat_map(|left| {
            right
                .iter()
                .filter(move |right| left.source == right.source)
                .map(move |right| {
                    left.end_byte
                        .min(right.end_byte)
                        .saturating_sub(left.start_byte.max(right.start_byte))
                })
        })
        .sum::<u64>();
    overlap as f32 / denominator as f32
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, Utc};
    use serde_json::json;

    use super::*;
    use crate::domain::{MemoryClaim, MemoryKind};

    #[test]
    fn proposes_but_does_not_apply_claim_supersession() {
        let mut older = MemoryRecord::new("workspace", MemoryKind::Decision, "Use L2 distance");
        older.id = "older".into();
        older.claim = Some(MemoryClaim::new("embedding.metric", json!("l2")));
        older.created_at = Utc::now() - Duration::hours(1);
        let mut newer = MemoryRecord::new("workspace", MemoryKind::Decision, "Use cosine distance");
        newer.id = "newer".into();
        newer.claim = Some(MemoryClaim::new("embedding.metric", json!("cosine")));

        let report = analyze_memory_consolidation("workspace", vec![newer, older]);

        assert_eq!(report.analyzed_pair_count, 1);
        assert_eq!(report.proposals.len(), 1);
        assert_eq!(
            report.proposals[0]
                .contradiction
                .as_ref()
                .unwrap()
                .claim_key,
            "embedding.metric"
        );
        assert_eq!(
            report.proposals[0].proposed_supersession,
            Some(ProposedMemorySupersession {
                superseded_memory_id: "older".into(),
                superseding_memory_id: "newer".into(),
            })
        );
    }

    #[test]
    fn source_overlap_supports_duplicate_detection() {
        let older = MemoryRecord::imported(
            "workspace",
            MemoryKind::Observation,
            "alpha beta gamma delta",
            vec![SourceSegment::new("notes.txt", 0, 100)],
        );
        let newer = MemoryRecord::imported(
            "workspace",
            MemoryKind::Observation,
            "alpha beta gamma epsilon",
            vec![SourceSegment::new("notes.txt", 10, 100)],
        );

        let proposal = consolidation_proposal(&older, &newer).unwrap();

        assert_eq!(proposal.source_overlap_ratio, 1.0);
        assert_eq!(proposal.duplicate_token_ratio, 0.75);
        assert!(proposal.proposed_supersession.is_none());
    }
}
