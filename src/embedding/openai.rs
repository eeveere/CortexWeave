use std::time::Duration;

use async_trait::async_trait;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

use crate::{
    CortexError, Result,
    config::{EmbeddingConfig, TokenizerKind},
};

use super::{
    ConservativeByteCounter, EmbeddingFailure, EmbeddingFailureKind, EmbeddingLimits,
    EmbeddingProvider, HuggingFaceTokenCounter, TokenCount, TokenCountAccuracy, TokenCounter,
};

pub struct OpenAiCompatibleEmbeddingProvider {
    client: Client,
    endpoint: Url,
    model: String,
    embedding_space: String,
    document_transformation: String,
    dimension: Option<usize>,
    batch_size: usize,
    document_prefix: String,
    query_prefix: String,
    limits: EmbeddingLimits,
    token_counter: Box<dyn TokenCounter>,
}

impl OpenAiCompatibleEmbeddingProvider {
    pub fn new(config: &EmbeddingConfig) -> Result<Self> {
        config.validate()?;
        let base = Url::parse(&config.base_url)
            .map_err(|error| CortexError::Configuration(error.to_string()))?;
        let endpoint = base
            .join(config.endpoint.trim_start_matches('/'))
            .map_err(|error| CortexError::Configuration(error.to_string()))?;
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_seconds))
            .build()
            .map_err(|error| CortexError::Embedding(error.to_string()))?;
        let transformation = format!(
            "cortexweave-embedding-space-v1\0{}\0{}\0{}",
            config.model, config.document_prefix, config.query_prefix
        );
        let transformation_hash = blake3::hash(transformation.as_bytes()).to_hex();
        let document_transformation = format!(
            "document-prefix-v1:{}",
            &blake3::hash(config.document_prefix.as_bytes()).to_hex()[..16]
        );
        let token_counter: Box<dyn TokenCounter> = match config.limits.tokenizer {
            TokenizerKind::ConservativeBytes => Box::new(ConservativeByteCounter),
            TokenizerKind::HuggingFace => {
                Box::new(HuggingFaceTokenCounter::from_file(std::path::Path::new(
                    config
                        .limits
                        .tokenizer_path
                        .as_deref()
                        .expect("validated Hugging Face tokenizer path"),
                ))?)
            }
        };
        Ok(Self {
            client,
            endpoint,
            model: config.model.clone(),
            embedding_space: format!("{}@{}", config.model, &transformation_hash[..16]),
            document_transformation,
            dimension: config.dimension,
            batch_size: config.batch_size,
            document_prefix: config.document_prefix.clone(),
            query_prefix: config.query_prefix.clone(),
            limits: EmbeddingLimits {
                max_input_tokens: config.limits.max_input_tokens,
                max_batch_tokens: config.limits.max_batch_tokens,
                max_batch_items: config.batch_size,
                reserved_tokens: config.limits.reserved_tokens,
            },
            token_counter,
        })
    }

    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let response = self
            .client
            .post(self.endpoint.clone())
            .json(&EmbeddingRequest {
                model: &self.model,
                input: texts,
            })
            .send()
            .await
            .map_err(request_failure)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(classify_http_failure(status.as_u16(), &body).into());
        }
        let payload: EmbeddingResponse = response.json().await.map_err(|error| {
            EmbeddingFailure::new(
                EmbeddingFailureKind::InvalidResponse,
                format!("invalid embedding response: {error}"),
            )
        })?;
        Ok(validate_response(
            payload,
            texts.len(),
            &self.model,
            self.dimension,
        )?)
    }

    async fn embed_prepared(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.batch_size) {
            vectors.extend(self.embed_batch(batch).await?);
        }
        Ok(vectors)
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleEmbeddingProvider {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_documents(texts).await
    }

    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_prepared(
            texts
                .iter()
                .map(|text| self.prepare_document_input(text))
                .collect(),
        )
        .await
    }

    async fn embed_queries(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_prepared(
            texts
                .iter()
                .map(|text| self.prepare_query_input(text))
                .collect(),
        )
        .await
    }

    fn model_name(&self) -> &str {
        &self.embedding_space
    }

    fn request_model_name(&self) -> &str {
        &self.model
    }

    fn embedding_space_id(&self) -> &str {
        &self.embedding_space
    }

    fn prepare_document_input(&self, text: &str) -> String {
        format!("{}{text}", self.document_prefix)
    }

    fn prepare_query_input(&self, text: &str) -> String {
        format!("{}{text}", self.query_prefix)
    }

    fn document_transformation_id(&self) -> &str {
        &self.document_transformation
    }

    fn count_tokens(&self, text: &str) -> TokenCount {
        self.token_counter.count(text)
    }

    fn token_counter_id(&self) -> &str {
        self.token_counter.identity()
    }

    fn token_counter_accuracy(&self) -> TokenCountAccuracy {
        self.token_counter.accuracy()
    }

    fn limits(&self) -> EmbeddingLimits {
        self.limits
    }

    fn dimension(&self) -> Option<usize> {
        self.dimension
    }

    fn batch_size(&self) -> Option<usize> {
        Some(self.batch_size)
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingDatum {
    index: usize,
    embedding: Vec<f32>,
}

fn validate_response(
    mut response: EmbeddingResponse,
    expected_count: usize,
    expected_model: &str,
    expected_dimension: Option<usize>,
) -> std::result::Result<Vec<Vec<f32>>, EmbeddingFailure> {
    if let Some(model) = &response.model
        && model != expected_model
    {
        return Err(EmbeddingFailure::new(
            EmbeddingFailureKind::InvalidResponse,
            format!("embedding model mismatch: requested {expected_model}, received {model}"),
        ));
    }
    if response.data.len() != expected_count {
        return Err(EmbeddingFailure::new(
            EmbeddingFailureKind::InvalidResponse,
            format!(
                "embedding count mismatch: expected {expected_count}, received {}",
                response.data.len()
            ),
        ));
    }
    response.data.sort_unstable_by_key(|datum| datum.index);
    let mut dimension = None;
    for (expected_index, datum) in response.data.iter().enumerate() {
        if datum.index != expected_index {
            return Err(EmbeddingFailure::new(
                EmbeddingFailureKind::InvalidResponse,
                format!("embedding response has missing or duplicate index {expected_index}"),
            ));
        }
        if datum.embedding.is_empty() {
            return Err(EmbeddingFailure::new(
                EmbeddingFailureKind::InvalidResponse,
                "embedding vector is empty",
            ));
        }
        if expected_dimension.is_some_and(|expected| expected != datum.embedding.len()) {
            return Err(EmbeddingFailure::new(
                EmbeddingFailureKind::InvalidResponse,
                format!(
                    "embedding dimension mismatch: expected {}, received {}",
                    expected_dimension.unwrap_or_default(),
                    datum.embedding.len()
                ),
            ));
        }
        match dimension {
            Some(value) if value != datum.embedding.len() => {
                return Err(EmbeddingFailure::new(
                    EmbeddingFailureKind::InvalidResponse,
                    "embedding response contains inconsistent dimensions",
                ));
            }
            None => dimension = Some(datum.embedding.len()),
            _ => {}
        }
        if datum.embedding.iter().any(|value| !value.is_finite()) {
            return Err(EmbeddingFailure::new(
                EmbeddingFailureKind::InvalidResponse,
                "embedding response contains a non-finite value",
            ));
        }
    }
    Ok(response
        .data
        .into_iter()
        .map(|datum| datum.embedding)
        .collect())
}

fn request_failure(error: reqwest::Error) -> CortexError {
    let kind = if error.is_timeout() {
        EmbeddingFailureKind::Timeout
    } else if error.is_connect() {
        EmbeddingFailureKind::Unavailable
    } else {
        EmbeddingFailureKind::Protocol
    };
    EmbeddingFailure::new(kind, error.to_string()).into()
}

fn classify_http_failure(status: u16, body: &str) -> EmbeddingFailure {
    let normalized = body.to_ascii_lowercase();
    let kind = if normalized.contains("physical batch size")
        || normalized.contains("batch") && normalized.contains("too large")
    {
        EmbeddingFailureKind::BatchTooLarge
    } else if status == 413
        || normalized.contains("larger than the max context size")
        || normalized.contains("input") && normalized.contains("too large")
    {
        EmbeddingFailureKind::InputTooLarge
    } else if status == 408 || status == 504 {
        EmbeddingFailureKind::Timeout
    } else if status >= 500 {
        EmbeddingFailureKind::Unavailable
    } else {
        EmbeddingFailureKind::Unknown
    };
    EmbeddingFailure::new(kind, format!("embedding endpoint rejected request: {body}"))
        .with_status(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(vectors: Vec<(usize, Vec<f32>)>) -> EmbeddingResponse {
        EmbeddingResponse {
            data: vectors
                .into_iter()
                .map(|(index, embedding)| EmbeddingDatum { index, embedding })
                .collect(),
            model: Some("model-a".into()),
        }
    }

    #[test]
    fn reorders_indexed_batch_results() {
        let vectors = validate_response(
            response(vec![(1, vec![0.0, 1.0]), (0, vec![1.0, 0.0])]),
            2,
            "model-a",
            None,
        )
        .unwrap();
        assert_eq!(vectors, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
    }

    #[test]
    fn rejects_malformed_responses() {
        assert!(validate_response(response(vec![]), 1, "model-a", None).is_err());
        assert!(validate_response(response(vec![(0, vec![])]), 1, "model-a", None).is_err());
        assert!(
            validate_response(response(vec![(0, vec![f32::NAN])]), 1, "model-a", None).is_err()
        );
        assert!(
            validate_response(
                response(vec![(0, vec![1.0]), (1, vec![1.0, 2.0])]),
                2,
                "model-a",
                None
            )
            .is_err()
        );
        assert!(validate_response(response(vec![(0, vec![1.0])]), 1, "model-b", None).is_err());
        assert!(validate_response(response(vec![(0, vec![1.0])]), 1, "model-a", Some(2)).is_err());
    }

    #[test]
    fn classifies_llama_cpp_capacity_failures() {
        let input = classify_http_failure(
            400,
            "input (2330 tokens) is larger than the max context size (2048 tokens)",
        );
        assert_eq!(input.kind, EmbeddingFailureKind::InputTooLarge);
        assert_eq!(input.status, Some(400));

        let batch = classify_http_failure(
            400,
            "input (4590 tokens) is too large to process; increase the physical batch size",
        );
        assert_eq!(batch.kind, EmbeddingFailureKind::BatchTooLarge);

        let unknown = classify_http_failure(400, "invalid model parameter");
        assert_eq!(unknown.kind, EmbeddingFailureKind::Unknown);

        let payload_too_large = classify_http_failure(413, "");
        assert_eq!(payload_too_large.kind, EmbeddingFailureKind::InputTooLarge);
    }

    #[test]
    fn exposes_distinct_model_space_transforms_and_limits() {
        let config = EmbeddingConfig {
            model: "nomic-embed-text-v1.5".into(),
            document_prefix: "search_document: ".into(),
            query_prefix: "search_query: ".into(),
            batch_size: 4,
            limits: crate::config::EmbeddingLimitConfig {
                max_input_tokens: Some(2_048),
                max_batch_tokens: Some(4_096),
                reserved_tokens: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let provider = OpenAiCompatibleEmbeddingProvider::new(&config).unwrap();
        assert_eq!(provider.request_model_name(), "nomic-embed-text-v1.5");
        assert_ne!(provider.embedding_space_id(), provider.request_model_name());
        assert_eq!(
            provider.prepare_document_input("alpha"),
            "search_document: alpha"
        );
        assert_eq!(provider.prepare_query_input("alpha"), "search_query: alpha");
        assert_eq!(provider.limits().input_budget(), Some(2_016));
        assert_eq!(provider.limits().max_batch_items, 4);
        assert_eq!(provider.token_counter_id(), "conservative-bytes-v1");

        let changed = OpenAiCompatibleEmbeddingProvider::new(&EmbeddingConfig {
            document_prefix: "different: ".into(),
            ..config
        })
        .unwrap();
        assert_ne!(provider.embedding_space_id(), changed.embedding_space_id());
    }
}
