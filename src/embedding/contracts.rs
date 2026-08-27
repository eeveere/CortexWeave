use std::{
    fmt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingLimits {
    pub max_input_tokens: Option<usize>,
    pub max_batch_tokens: Option<usize>,
    pub max_batch_items: usize,
    pub reserved_tokens: usize,
}

impl EmbeddingLimits {
    pub fn input_budget(self) -> Option<usize> {
        self.max_input_tokens
            .map(|limit| limit.saturating_sub(self.reserved_tokens))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenCountAccuracy {
    Exact,
    #[default]
    ConservativeEstimate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCount {
    pub tokens: usize,
    pub accuracy: TokenCountAccuracy,
}

pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str) -> TokenCount;
    fn identity(&self) -> &str;

    fn accuracy(&self) -> TokenCountAccuracy {
        self.count("").accuracy
    }
}

#[derive(Debug, Default)]
pub struct ConservativeByteCounter;

impl TokenCounter for ConservativeByteCounter {
    fn count(&self, text: &str) -> TokenCount {
        TokenCount {
            tokens: text.len(),
            accuracy: TokenCountAccuracy::ConservativeEstimate,
        }
    }

    fn identity(&self) -> &str {
        "conservative-bytes-v1"
    }

    fn accuracy(&self) -> TokenCountAccuracy {
        TokenCountAccuracy::ConservativeEstimate
    }
}

pub struct HuggingFaceTokenCounter {
    tokenizer: tokenizers::Tokenizer,
    identity: String,
    fallback_used: AtomicBool,
}

impl HuggingFaceTokenCounter {
    pub fn from_file(path: &Path) -> crate::Result<Self> {
        let bytes = std::fs::read(path).map_err(|source| crate::CortexError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let tokenizer = tokenizers::Tokenizer::from_bytes(&bytes).map_err(|error| {
            crate::CortexError::Configuration(format!(
                "failed to load Hugging Face tokenizer {}: {error}",
                path.display()
            ))
        })?;
        let hash = blake3::hash(&bytes).to_hex();
        Ok(Self {
            tokenizer,
            identity: format!("hugging-face-tokenizer-v1:{}", &hash[..16]),
            fallback_used: AtomicBool::new(false),
        })
    }
}

impl TokenCounter for HuggingFaceTokenCounter {
    fn count(&self, text: &str) -> TokenCount {
        match self.tokenizer.encode(text, true) {
            Ok(encoding) => TokenCount {
                tokens: encoding.len(),
                accuracy: TokenCountAccuracy::Exact,
            },
            Err(_) => {
                self.fallback_used.store(true, Ordering::Relaxed);
                ConservativeByteCounter.count(text)
            }
        }
    }

    fn identity(&self) -> &str {
        &self.identity
    }

    fn accuracy(&self) -> TokenCountAccuracy {
        if self.fallback_used.load(Ordering::Relaxed) {
            TokenCountAccuracy::ConservativeEstimate
        } else {
            TokenCountAccuracy::Exact
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingFailureKind {
    InputTooLarge,
    BatchTooLarge,
    Unavailable,
    Timeout,
    Protocol,
    InvalidResponse,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingFailure {
    pub kind: EmbeddingFailureKind,
    pub message: String,
    pub status: Option<u16>,
}

impl EmbeddingFailure {
    pub fn new(kind: EmbeddingFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status: None,
        }
    }

    pub fn with_status(mut self, status: u16) -> Self {
        self.status = Some(status);
        self
    }
}

impl fmt::Display for EmbeddingFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(formatter, "{} (HTTP {status})", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for EmbeddingFailure {}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn conservative_counter_is_utf8_safe_and_never_understates_bytes() {
        let counter = ConservativeByteCounter;
        let count = counter.count("alpha β");
        assert_eq!(count.tokens, "alpha β".len());
        assert_eq!(count.accuracy, TokenCountAccuracy::ConservativeEstimate);
    }

    #[test]
    fn input_budget_reserves_provider_overhead() {
        let limits = EmbeddingLimits {
            max_input_tokens: Some(2_048),
            max_batch_tokens: Some(4_096),
            max_batch_items: 16,
            reserved_tokens: 32,
        };
        assert_eq!(limits.input_budget(), Some(2_016));
    }

    #[test]
    fn loads_exact_hugging_face_tokenizer_with_content_identity() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("tokenizer.json");
        let vocabulary_path = directory.path().join("vocab.json");
        std::fs::write(&vocabulary_path, r#"{"[UNK]":0,"hello":1,"world":2}"#).unwrap();
        let model = tokenizers::models::wordlevel::WordLevel::from_file(
            vocabulary_path.to_str().unwrap(),
            "[UNK]".into(),
        )
        .unwrap();
        let mut tokenizer = tokenizers::Tokenizer::new(model);
        tokenizer.with_pre_tokenizer(Some(tokenizers::pre_tokenizers::whitespace::Whitespace));
        tokenizer.save(&path, false).unwrap();

        let counter = HuggingFaceTokenCounter::from_file(&path).unwrap();
        assert_eq!(counter.count("hello world").tokens, 2);
        assert_eq!(
            counter.count("hello world").accuracy,
            TokenCountAccuracy::Exact
        );
        assert!(counter.identity().starts_with("hugging-face-tokenizer-v1:"));
    }
}
