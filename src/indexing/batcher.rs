use std::{collections::VecDeque, time::Instant};

use crate::{
    CortexError,
    embedding::{EmbeddingFailureKind, EmbeddingProvider},
    instrumentation::RuntimeMetrics,
};

const MAX_CAPACITY_RETRY_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedBatch {
    pub indices: Vec<usize>,
    pub token_count: usize,
    pub largest_item_tokens: usize,
}

#[derive(Debug)]
pub(crate) enum BatchEmbeddingError {
    NeedsSmallerInput { measured_tokens: usize },
    Fatal(CortexError),
}

pub(crate) fn plan_batches(
    texts: &[String],
    provider: &dyn EmbeddingProvider,
) -> std::result::Result<Vec<PlannedBatch>, BatchEmbeddingError> {
    let limits = provider.limits();
    let max_items = limits.max_batch_items.max(1);
    let counts: Vec<usize> = texts
        .iter()
        .map(|text| {
            provider
                .count_tokens(&provider.prepare_document_input(text))
                .tokens
        })
        .collect();

    for count in counts.iter().copied() {
        if limits.input_budget().is_some_and(|budget| count > budget)
            || limits.max_batch_tokens.is_some_and(|budget| count > budget)
        {
            return Err(BatchEmbeddingError::NeedsSmallerInput {
                measured_tokens: count,
            });
        }
    }

    let mut batches = Vec::new();
    let mut current = PlannedBatch {
        indices: Vec::new(),
        token_count: 0,
        largest_item_tokens: 0,
    };
    for (index, tokens) in counts.into_iter().enumerate() {
        let exceeds_items = current.indices.len() == max_items;
        let exceeds_tokens = limits
            .max_batch_tokens
            .is_some_and(|limit| current.token_count.saturating_add(tokens) > limit);
        if !current.indices.is_empty() && (exceeds_items || exceeds_tokens) {
            batches.push(current);
            current = PlannedBatch {
                indices: Vec::new(),
                token_count: 0,
                largest_item_tokens: 0,
            };
        }
        current.indices.push(index);
        current.token_count = current.token_count.saturating_add(tokens);
        current.largest_item_tokens = current.largest_item_tokens.max(tokens);
    }
    if !current.indices.is_empty() {
        batches.push(current);
    }
    Ok(batches)
}

pub(crate) async fn embed_documents(
    texts: &[String],
    provider: &dyn EmbeddingProvider,
    metrics: &RuntimeMetrics,
) -> std::result::Result<Vec<Vec<f32>>, BatchEmbeddingError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let planned = plan_batches(texts, provider)?;
    let mut queue: VecDeque<(Vec<usize>, usize)> = planned
        .into_iter()
        .map(|batch| (batch.indices, 0))
        .collect();
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; texts.len()];

    while let Some((indices, depth)) = queue.pop_front() {
        let batch: Vec<String> = indices.iter().map(|index| texts[*index].clone()).collect();
        let started = Instant::now();
        let result = provider.embed_documents(&batch).await;
        metrics.record_embedding(batch.len(), None, started.elapsed());
        match result {
            Ok(batch_vectors) => {
                if batch_vectors.len() != indices.len() {
                    return Err(BatchEmbeddingError::Fatal(CortexError::Embedding(format!(
                        "provider returned {} vectors for {} inputs",
                        batch_vectors.len(),
                        indices.len()
                    ))));
                }
                for (index, vector) in indices.into_iter().zip(batch_vectors) {
                    vectors[index] = Some(vector);
                }
            }
            Err(CortexError::EmbeddingFailure(failure))
                if matches!(
                    failure.kind,
                    EmbeddingFailureKind::BatchTooLarge | EmbeddingFailureKind::InputTooLarge
                ) =>
            {
                if depth >= MAX_CAPACITY_RETRY_DEPTH {
                    metrics.record_terminal_capacity_failure();
                    return Err(BatchEmbeddingError::Fatal(CortexError::EmbeddingFailure(
                        failure,
                    )));
                }
                metrics.record_capacity_retry(indices.len() > 1);
                if indices.len() == 1 {
                    let index = indices[0];
                    let measured_tokens = provider
                        .count_tokens(&provider.prepare_document_input(&texts[index]))
                        .tokens;
                    return Err(BatchEmbeddingError::NeedsSmallerInput { measured_tokens });
                }
                let middle = indices.len() / 2;
                let left = indices[..middle].to_vec();
                let right = indices[middle..].to_vec();
                queue.push_front((right, depth + 1));
                queue.push_front((left, depth + 1));
            }
            Err(error) => return Err(BatchEmbeddingError::Fatal(error)),
        }
    }

    vectors
        .into_iter()
        .enumerate()
        .map(|(index, vector)| {
            vector.ok_or_else(|| {
                BatchEmbeddingError::Fatal(CortexError::Embedding(format!(
                    "provider returned no vector for input {index}"
                )))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::{Result, embedding::EmbeddingLimits};

    struct StrictProvider;

    #[async_trait]
    impl EmbeddingProvider for StrictProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let total: usize = texts.iter().map(String::len).sum();
            if total > 10 {
                return Err(crate::embedding::EmbeddingFailure::new(
                    EmbeddingFailureKind::BatchTooLarge,
                    "strict aggregate limit",
                )
                .into());
            }
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
        fn model_name(&self) -> &str {
            "strict"
        }
        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(8),
                max_batch_tokens: Some(12),
                max_batch_items: 3,
                reserved_tokens: 0,
            }
        }
    }

    #[test]
    fn packs_by_item_and_token_limits_in_input_order() {
        let texts = vec![
            "123456".into(),
            "123456".into(),
            "1".into(),
            "1".into(),
            "1".into(),
            "1".into(),
        ];
        let batches = plan_batches(&texts, &StrictProvider).unwrap();
        assert_eq!(batches[0].indices, vec![0, 1]);
        assert_eq!(batches[0].token_count, 12);
        assert_eq!(batches[1].indices, vec![2, 3, 4]);
        assert_eq!(batches[2].indices, vec![5]);
    }

    #[tokio::test]
    async fn repacks_provider_rejected_batches_boundedly() {
        let texts = vec!["123456".into(), "123456".into(), "12".into()];
        let metrics = RuntimeMetrics::default();
        let vectors = embed_documents(&texts, &StrictProvider, &metrics)
            .await
            .unwrap();
        assert_eq!(vectors.len(), 3);
        let mut snapshot = crate::instrumentation::InstrumentationSnapshot::default();
        metrics.apply_to(&mut snapshot);
        assert_eq!(snapshot.capacity_retries, 1);
        assert_eq!(snapshot.batch_repacks, 1);
    }
}
