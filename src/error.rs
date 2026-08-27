use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CortexError {
    #[error("configuration error: {0}")]
    Configuration(String),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("analysis error: {0}")]
    Analysis(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("embedding error: {0}")]
    EmbeddingFailure(#[from] crate::embedding::EmbeddingFailure),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("ambiguous: {0}")]
    Ambiguous(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<std::io::Error> for CortexError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            path: PathBuf::new(),
            source,
        }
    }
}

pub type Result<T, E = CortexError> = std::result::Result<T, E>;
