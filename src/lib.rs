pub mod adapters;
pub mod config;
pub mod domain;
pub mod embedding;
pub mod error;
pub mod indexing;
pub mod instrumentation;
pub mod parsing;
pub mod retrieval;
pub mod service;
pub mod storage;
pub mod workspace;

pub use config::AppConfig;
pub use error::{CortexError, Result};
pub use service::{ContextService, CortexWeaveService};
