use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};

use crate::domain::{ContextPacket, ContextSourceType};

#[derive(Debug, Clone)]
pub struct ContextEvaluationCase {
    pub id: String,
    pub packet: ContextPacket,
    pub expected_source_ids: Vec<String>,
    pub expected_current_paths: Vec<String>,
    pub expected_resume_task_id: Option<String>,
    pub selected_resume_task_id: Option<String>,
    pub selection_latency: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextEvaluationMetrics {
    pub case_count: usize,
    pub context_recall: f64,
    pub context_precision: f64,
    pub mean_reciprocal_rank: f64,
    pub token_utilization: f64,
    pub duplicate_token_ratio: f64,
    pub current_source_accuracy: f64,
    pub resume_context_accuracy: f64,
    pub average_selection_latency: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplementationEvidence {
    pub path: String,
    pub symbol: String,
}

#[derive(Debug, Clone)]
pub struct ImplementationCoverageCase {
    pub id: String,
    pub packet: ContextPacket,
    pub expected: Vec<ImplementationEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplementationEvidenceResult {
    pub expected: ImplementationEvidence,
    pub matched_source_id: Option<String>,
    pub mention_only_source_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplementationCoverageResult {
    pub id: String,
    pub expected_count: usize,
    pub matched_count: usize,
    pub coverage: f64,
    pub passed: bool,
    pub evidence: Vec<ImplementationEvidenceResult>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImplementationCoverageMetrics {
    pub case_count: usize,
    pub passed_cases: usize,
    pub pass_rate: f64,
    pub mean_coverage: f64,
    pub cases: Vec<ImplementationCoverageResult>,
}

pub fn evaluate_context_packets(cases: &[ContextEvaluationCase]) -> ContextEvaluationMetrics {
    if cases.is_empty() {
        return ContextEvaluationMetrics {
            case_count: 0,
            context_recall: 0.0,
            context_precision: 0.0,
            mean_reciprocal_rank: 0.0,
            token_utilization: 0.0,
            duplicate_token_ratio: 0.0,
            current_source_accuracy: 0.0,
            resume_context_accuracy: 0.0,
            average_selection_latency: Duration::ZERO,
        };
    }

    let mut recall = 0.0;
    let mut precision = 0.0;
    let mut reciprocal_rank = 0.0;
    let mut utilization = 0.0;
    let mut duplicate_tokens = 0_usize;
    let mut selected_tokens = 0_usize;
    let mut current_source_accuracy = 0.0;
    let mut resume_context_accuracy = 0.0;
    let mut latency = Duration::ZERO;

    for case in cases {
        let selected_ids: Vec<_> = case
            .packet
            .items
            .iter()
            .map(|item| item.source_id.as_str())
            .collect();
        let matching = case
            .expected_source_ids
            .iter()
            .filter(|source_id| selected_ids.contains(&source_id.as_str()))
            .count();
        if !case.expected_source_ids.is_empty() {
            recall += matching as f64 / case.expected_source_ids.len() as f64;
            if let Some(index) = selected_ids.iter().position(|source_id| {
                case.expected_source_ids
                    .iter()
                    .any(|expected| expected == source_id)
            }) {
                reciprocal_rank += 1.0 / (index + 1) as f64;
            }
        }
        if !case.packet.items.is_empty() {
            precision += matching as f64 / case.packet.items.len() as f64;
        }
        if case.packet.token_budget > 0 {
            utilization += case.packet.estimated_tokens as f64 / case.packet.token_budget as f64;
        }
        selected_tokens += case.packet.estimated_tokens;
        duplicate_tokens += duplicate_item_tokens(&case.packet);

        if !case.expected_current_paths.is_empty() {
            let matched_paths = case
                .expected_current_paths
                .iter()
                .filter(|path| {
                    case.packet
                        .items
                        .iter()
                        .any(|item| item.path.as_deref() == Some(path.as_str()))
                })
                .count();
            current_source_accuracy +=
                matched_paths as f64 / case.expected_current_paths.len() as f64;
        }
        if let Some(expected) = case.expected_resume_task_id.as_deref() {
            resume_context_accuracy +=
                f64::from(case.selected_resume_task_id.as_deref() == Some(expected));
        }
        latency += case.selection_latency;
    }

    let count = cases.len() as f64;
    ContextEvaluationMetrics {
        case_count: cases.len(),
        context_recall: recall / count,
        context_precision: precision / count,
        mean_reciprocal_rank: reciprocal_rank / count,
        token_utilization: utilization / count,
        duplicate_token_ratio: duplicate_tokens as f64 / selected_tokens.max(1) as f64,
        current_source_accuracy: current_source_accuracy / count,
        resume_context_accuracy: resume_context_accuracy / count,
        average_selection_latency: latency / u32::try_from(cases.len()).unwrap_or(u32::MAX),
    }
}

pub fn evaluate_implementation_coverage(
    cases: &[ImplementationCoverageCase],
) -> ImplementationCoverageMetrics {
    let results: Vec<_> = cases
        .iter()
        .map(|case| {
            let evidence: Vec<_> = case
                .expected
                .iter()
                .map(|expected| {
                    let matched_source_id = case
                        .packet
                        .items
                        .iter()
                        .find(|item| {
                            item.source_type == ContextSourceType::Code
                                && item.path.as_deref() == Some(expected.path.as_str())
                                && item.symbol.as_deref() == Some(expected.symbol.as_str())
                        })
                        .map(|item| item.source_id.clone());
                    let mention_only_source_ids = case
                        .packet
                        .items
                        .iter()
                        .filter(|item| {
                            matched_source_id.as_deref() != Some(item.source_id.as_str())
                                && (item.content.contains(&expected.symbol)
                                    || item.content.contains(&expected.path))
                        })
                        .map(|item| item.source_id.clone())
                        .collect();
                    ImplementationEvidenceResult {
                        expected: expected.clone(),
                        matched_source_id,
                        mention_only_source_ids,
                    }
                })
                .collect();
            let matched_count = evidence
                .iter()
                .filter(|result| result.matched_source_id.is_some())
                .count();
            let expected_count = evidence.len();
            ImplementationCoverageResult {
                id: case.id.clone(),
                expected_count,
                matched_count,
                coverage: if expected_count == 0 {
                    1.0
                } else {
                    matched_count as f64 / expected_count as f64
                },
                passed: matched_count == expected_count,
                evidence,
            }
        })
        .collect();
    let passed_cases = results.iter().filter(|result| result.passed).count();
    let case_count = results.len();
    ImplementationCoverageMetrics {
        case_count,
        passed_cases,
        pass_rate: if case_count == 0 {
            0.0
        } else {
            passed_cases as f64 / case_count as f64
        },
        mean_coverage: if case_count == 0 {
            0.0
        } else {
            results.iter().map(|result| result.coverage).sum::<f64>() / case_count as f64
        },
        cases: results,
    }
}

fn duplicate_item_tokens(packet: &ContextPacket) -> usize {
    let mut covered = BTreeMap::<String, Vec<(u64, u64)>>::new();
    let mut selected_source_ids = HashSet::new();
    let mut duplicate_tokens = 0_usize;
    for item in &packet.items {
        if item.source_segments.is_empty() {
            if !selected_source_ids
                .insert((item.source_type.storage_name(), item.source_id.clone()))
            {
                duplicate_tokens = duplicate_tokens.saturating_add(item.estimated_tokens);
            }
            continue;
        }
        let total_bytes = item
            .source_segments
            .iter()
            .map(|segment| segment.end_byte.saturating_sub(segment.start_byte))
            .sum::<u64>();
        let duplicate_bytes = item
            .source_segments
            .iter()
            .map(|segment| {
                covered
                    .get(&segment.source)
                    .map(|intervals| {
                        interval_overlap(segment.start_byte, segment.end_byte, intervals)
                    })
                    .unwrap_or_default()
            })
            .sum::<u64>();
        if total_bytes > 0 {
            let ratio = duplicate_bytes as f64 / total_bytes as f64;
            duplicate_tokens = duplicate_tokens
                .saturating_add((item.estimated_tokens as f64 * ratio).round() as usize);
        }
        for segment in &item.source_segments {
            insert_interval(
                covered.entry(segment.source.clone()).or_default(),
                segment.start_byte,
                segment.end_byte,
            );
        }
        selected_source_ids.insert((item.source_type.storage_name(), item.source_id.clone()));
    }
    duplicate_tokens
}

fn interval_overlap(start: u64, end: u64, covered: &[(u64, u64)]) -> u64 {
    covered
        .iter()
        .map(|(covered_start, covered_end)| {
            end.min(*covered_end)
                .saturating_sub(start.max(*covered_start))
        })
        .sum()
}

fn insert_interval(intervals: &mut Vec<(u64, u64)>, start: u64, end: u64) {
    if start >= end {
        return;
    }
    intervals.push((start, end));
    intervals.sort_unstable();
    let mut merged = Vec::with_capacity(intervals.len());
    for (start, end) in intervals.drain(..) {
        if let Some((_, previous_end)) = merged.last_mut()
            && start <= *previous_end
        {
            *previous_end = (*previous_end).max(end);
        } else {
            merged.push((start, end));
        }
    }
    *intervals = merged;
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::domain::{ContextFreshness, ContextItem, ContextScores, SourceSegment};

    fn packet(items: Vec<ContextItem>) -> ContextPacket {
        ContextPacket {
            workspace_id: "workspace".into(),
            session_id: None,
            task_id: None,
            summary: None,
            estimated_tokens: items.iter().map(|item| item.estimated_tokens).sum(),
            token_budget: 100,
            items,
            generated_at: Utc::now(),
            explanation: None,
        }
    }

    fn item(id: &str, path: Option<&str>, tokens: usize) -> ContextItem {
        ContextItem {
            source_id: id.into(),
            source_type: ContextSourceType::Code,
            content: format!("fn {id}() {{}}"),
            path: path.map(Into::into),
            symbol: None,
            language: Some("rust".into()),
            source_segments: Vec::new(),
            freshness: ContextFreshness::Current,
            scores: ContextScores::default(),
            reasons: Vec::new(),
            structural_evidence: Vec::new(),
            estimated_tokens: tokens,
            truncated: false,
        }
    }

    #[test]
    fn duplicate_tokens_are_proportional_to_source_range_overlap() {
        let mut first = item("first", Some("src/lib.rs"), 100);
        first.source_segments = vec![SourceSegment::new("src/lib.rs", 0, 100)];
        let mut overlapping = item("overlapping", Some("src/lib.rs"), 100);
        overlapping.source_segments = vec![SourceSegment::new("src/lib.rs", 50, 150)];
        let mut separate = item("separate", Some("src/lib.rs"), 100);
        separate.content = first.content.clone();
        separate.source_segments = vec![SourceSegment::new("src/lib.rs", 200, 300)];

        let metrics = evaluate_context_packets(&[ContextEvaluationCase {
            id: "overlap".into(),
            packet: packet(vec![first, overlapping, separate]),
            expected_source_ids: Vec::new(),
            expected_current_paths: Vec::new(),
            expected_resume_task_id: None,
            selected_resume_task_id: None,
            selection_latency: Duration::ZERO,
        }]);

        assert_eq!(metrics.duplicate_token_ratio, 50.0 / 300.0);
    }

    #[test]
    fn measures_packet_selection_and_resume_accuracy() {
        let metrics = evaluate_context_packets(&[ContextEvaluationCase {
            id: "resume".into(),
            packet: packet(vec![
                item("target", Some("src/lib.rs"), 20),
                item("other", None, 10),
            ]),
            expected_source_ids: vec!["target".into()],
            expected_current_paths: vec!["src/lib.rs".into()],
            expected_resume_task_id: Some("task".into()),
            selected_resume_task_id: Some("task".into()),
            selection_latency: Duration::from_micros(50),
        }]);
        assert_eq!(metrics.case_count, 1);
        assert_eq!(metrics.context_recall, 1.0);
        assert_eq!(metrics.context_precision, 0.5);
        assert_eq!(metrics.mean_reciprocal_rank, 1.0);
        assert_eq!(metrics.current_source_accuracy, 1.0);
        assert_eq!(metrics.resume_context_accuracy, 1.0);
        assert_eq!(metrics.token_utilization, 0.3);
        assert_eq!(metrics.average_selection_latency, Duration::from_micros(50));
    }

    #[test]
    fn implementation_coverage_requires_code_path_and_symbol_not_mentions() {
        let mut implementation = item("implementation", Some("src/process_manager.py"), 20);
        implementation.symbol = Some("ProcessManager.ensure_ready".into());
        implementation.content = "def ensure_ready(self): return True".into();
        implementation.language = Some("python".into());
        let mut plan = item("plan", Some("docs/restart-plan.md"), 10);
        plan.source_type = ContextSourceType::Document;
        plan.symbol = None;
        plan.content = "Call ProcessManager.restart after ensure_ready.".into();
        let metrics = evaluate_implementation_coverage(&[ImplementationCoverageCase {
            id: "llama-server-lifecycle".into(),
            packet: packet(vec![implementation, plan]),
            expected: vec![
                ImplementationEvidence {
                    path: "src/process_manager.py".into(),
                    symbol: "ProcessManager.ensure_ready".into(),
                },
                ImplementationEvidence {
                    path: "src/process_manager.py".into(),
                    symbol: "ProcessManager.restart".into(),
                },
            ],
        }]);

        assert_eq!(metrics.case_count, 1);
        assert_eq!(metrics.passed_cases, 0);
        assert_eq!(metrics.mean_coverage, 0.5);
        assert_eq!(metrics.cases[0].matched_count, 1);
        assert_eq!(
            metrics.cases[0].evidence[1].mention_only_source_ids,
            vec!["plan"]
        );
    }
}
