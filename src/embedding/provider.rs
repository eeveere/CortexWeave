use async_trait::async_trait;

use crate::Result;

use super::{
    ConservativeByteCounter, EmbeddingLimits, TokenCount, TokenCountAccuracy, TokenCounter,
};

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    fn model_name(&self) -> &str;

    fn request_model_name(&self) -> &str {
        self.model_name()
    }

    fn embedding_space_id(&self) -> &str {
        self.model_name()
    }

    fn prepare_document_input(&self, text: &str) -> String {
        text.to_owned()
    }

    fn prepare_query_input(&self, text: &str) -> String {
        text.to_owned()
    }

    fn document_transformation_id(&self) -> &str {
        self.embedding_space_id()
    }

    fn count_tokens(&self, text: &str) -> TokenCount {
        ConservativeByteCounter.count(text)
    }

    fn token_counter_id(&self) -> &str {
        ConservativeByteCounter.identity()
    }

    fn token_counter_accuracy(&self) -> TokenCountAccuracy {
        self.count_tokens("").accuracy
    }

    fn limits(&self) -> EmbeddingLimits {
        EmbeddingLimits {
            max_input_tokens: None,
            max_batch_tokens: None,
            max_batch_items: self.batch_size().unwrap_or(usize::MAX),
            reserved_tokens: 0,
        }
    }

    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts).await
    }

    async fn embed_queries(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts).await
    }

    fn dimension(&self) -> Option<usize> {
        None
    }

    fn batch_size(&self) -> Option<usize> {
        None
    }
}

#[cfg(test)]
pub(crate) struct MockEmbeddingProvider {
    model: String,
    dimension: usize,
    calls: std::sync::atomic::AtomicUsize,
    embedded_texts: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl MockEmbeddingProvider {
    pub(crate) fn new(model: impl Into<String>, dimension: usize) -> Self {
        Self {
            model: model.into(),
            dimension,
            calls: std::sync::atomic::AtomicUsize::new(0),
            embedded_texts: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    pub(crate) fn call_count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn embedded_text_count(&self) -> usize {
        self.embedded_texts
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(test)]
#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.embedded_texts
            .fetch_add(texts.len(), std::sync::atomic::Ordering::Relaxed);
        Ok(texts
            .iter()
            .map(|text| {
                let mut vector = vec![0.0; self.dimension];
                for (index, byte) in text.bytes().enumerate() {
                    if self.dimension > 0 {
                        vector[index % self.dimension] += f32::from(byte) / 255.0;
                    }
                }
                vector
            })
            .collect())
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimension(&self) -> Option<usize> {
        Some(self.dimension)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::{EmbeddingFailure, EmbeddingFailureKind};

    #[tokio::test]
    async fn mock_provider_is_deterministic_and_tracks_batches() {
        let provider = MockEmbeddingProvider::new("test", 4);
        let first = provider.embed(&["alpha".into()]).await.unwrap();
        let second = provider.embed(&["alpha".into()]).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].len(), 4);
        assert_eq!(provider.call_count(), 2);
        assert_eq!(provider.embedded_text_count(), 2);
    }

    struct StrictProvider {
        limits: EmbeddingLimits,
    }

    #[async_trait]
    impl EmbeddingProvider for StrictProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let counts: Vec<_> = texts.iter().map(|text| self.count_tokens(text)).collect();
            if counts.iter().any(|count| {
                self.limits
                    .input_budget()
                    .is_some_and(|budget| count.tokens > budget)
            }) {
                return Err(EmbeddingFailure::new(
                    EmbeddingFailureKind::InputTooLarge,
                    "strict provider rejected oversized input",
                )
                .into());
            }
            if self.limits.max_batch_tokens.is_some_and(|budget| {
                counts.iter().map(|count| count.tokens).sum::<usize>() > budget
            }) {
                return Err(EmbeddingFailure::new(
                    EmbeddingFailureKind::BatchTooLarge,
                    "strict provider rejected oversized batch",
                )
                .into());
            }
            Ok(vec![vec![1.0]; texts.len()])
        }

        fn model_name(&self) -> &str {
            "strict"
        }

        fn limits(&self) -> EmbeddingLimits {
            self.limits
        }
    }

    #[tokio::test]
    async fn strict_provider_exposes_input_and_batch_failures() {
        let provider = StrictProvider {
            limits: EmbeddingLimits {
                max_input_tokens: Some(8),
                max_batch_tokens: Some(10),
                max_batch_items: 4,
                reserved_tokens: 0,
            },
        };
        let input_error = provider.embed(&["123456789".into()]).await.unwrap_err();
        assert!(matches!(
            input_error,
            crate::CortexError::EmbeddingFailure(EmbeddingFailure {
                kind: EmbeddingFailureKind::InputTooLarge,
                ..
            })
        ));
        let batch_error = provider
            .embed(&["123456".into(), "123456".into()])
            .await
            .unwrap_err();
        assert!(matches!(
            batch_error,
            crate::CortexError::EmbeddingFailure(EmbeddingFailure {
                kind: EmbeddingFailureKind::BatchTooLarge,
                ..
            })
        ));
    }
}
