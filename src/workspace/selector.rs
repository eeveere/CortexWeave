use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum WorkspaceSelector {
    Id(String),
    Name(String),
    RootPath(PathBuf),
    FileUri(String),
    Default,
}
