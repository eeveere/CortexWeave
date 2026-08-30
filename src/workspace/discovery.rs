use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::UNIX_EPOCH,
};

use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;

use crate::{CortexError, Result, parsing::AnalyzerRegistry};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    pub absolute_path: PathBuf,
    pub relative_path: PathBuf,
    pub size_bytes: u64,
    pub modified_at_ns: Option<i64>,
    pub content_hash: String,
    pub language: String,
    pub analyzer_id: String,
    pub analyzer_version: String,
}

#[derive(Debug, Default)]
pub struct WorkspaceScan {
    pub files: Vec<DiscoveredFile>,
    pub failed_relative_paths: HashSet<String>,
}

pub struct WorkspaceScanner {
    analyzers: Arc<AnalyzerRegistry>,
    max_file_bytes: u64,
    include_patterns: Vec<String>,
    exclude_patterns: Vec<String>,
}

impl WorkspaceScanner {
    pub fn new(analyzers: Arc<AnalyzerRegistry>, max_file_bytes: u64) -> Self {
        Self {
            analyzers,
            max_file_bytes,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
        }
    }

    pub fn with_patterns(
        analyzers: Arc<AnalyzerRegistry>,
        max_file_bytes: u64,
        include_patterns: Vec<String>,
        exclude_patterns: Vec<String>,
    ) -> Self {
        Self {
            analyzers,
            max_file_bytes,
            include_patterns,
            exclude_patterns,
        }
    }

    pub fn scan(&self, root: &Path) -> Result<WorkspaceScan> {
        if !root.is_dir() {
            return Err(CortexError::Configuration(format!(
                "workspace is not a directory: {}",
                root.display()
            )));
        }
        let mut scan = WorkspaceScan::default();
        let overrides = build_overrides(root, &self.include_patterns, &self.exclude_patterns)?;
        let walker = WalkBuilder::new(root)
            .standard_filters(true)
            .require_git(false)
            .hidden(false)
            .max_filesize(Some(self.max_file_bytes))
            .overrides(overrides)
            .filter_entry(|entry| entry.path().file_name().is_none_or(|name| name != ".git"))
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    tracing::warn!(%error, "skipping unreadable workspace entry");
                    continue;
                }
            };
            let file_type = entry.file_type();
            if !file_type.is_some_and(|kind| kind.is_file()) {
                continue;
            }
            let path = entry.path();
            let binary = match is_binary(path) {
                Ok(binary) => binary,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "skipping unreadable workspace file");
                    remember_failed_path(&mut scan, root, path);
                    continue;
                }
            };
            if binary {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "skipping workspace file without metadata");
                    remember_failed_path(&mut scan, root, path);
                    continue;
                }
            };
            if metadata.len() > self.max_file_bytes {
                continue;
            }
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "skipping unreadable workspace file");
                    remember_failed_path(&mut scan, root, path);
                    continue;
                }
            };
            if std::str::from_utf8(&bytes).is_err() {
                continue;
            }
            let relative_path = path
                .strip_prefix(root)
                .map_err(|error| CortexError::Analysis(error.to_string()))?
                .to_path_buf();
            let analyzer = self.analyzers.for_path(&relative_path);
            let modified_at_ns = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .and_then(|duration| i64::try_from(duration.as_nanos()).ok());
            scan.files.push(DiscoveredFile {
                absolute_path: path.to_path_buf(),
                relative_path,
                size_bytes: metadata.len(),
                modified_at_ns,
                content_hash: blake3::hash(&bytes).to_hex().to_string(),
                language: analyzer.language_id().into(),
                analyzer_id: analyzer.analyzer_id().into(),
                analyzer_version: analyzer.analyzer_version(),
            });
        }
        scan.files
            .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(scan)
    }
}

fn remember_failed_path(scan: &mut WorkspaceScan, root: &Path, path: &Path) {
    if let Ok(relative) = path.strip_prefix(root) {
        scan.failed_relative_paths
            .insert(relative.to_string_lossy().replace('\\', "/"));
    }
}

fn build_overrides(
    root: &Path,
    includes: &[String],
    excludes: &[String],
) -> Result<ignore::overrides::Override> {
    let mut builder = OverrideBuilder::new(root);
    for pattern in includes {
        builder.add(pattern).map_err(|error| {
            CortexError::Configuration(format!("invalid include pattern {pattern:?}: {error}"))
        })?;
    }
    for pattern in excludes {
        builder.add(&format!("!{pattern}")).map_err(|error| {
            CortexError::Configuration(format!("invalid exclude pattern {pattern:?}: {error}"))
        })?;
    }
    builder
        .build()
        .map_err(|error| CortexError::Configuration(error.to_string()))
}

fn is_binary(path: &Path) -> Result<bool> {
    let mut file = File::open(path).map_err(|source| CortexError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    let mut prefix = [0_u8; 8_192];
    let count = file.read(&mut prefix).map_err(|source| CortexError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(prefix[..count].contains(&0))
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn scan_honors_gitignore_and_skips_binary_files() {
        let directory = tempdir().unwrap();
        fs::create_dir(directory.path().join(".git")).unwrap();
        fs::write(directory.path().join(".gitignore"), "ignored.txt\n").unwrap();
        fs::write(directory.path().join(".git/config"), "[core]\n").unwrap();
        fs::write(directory.path().join("visible.rs"), "fn main() {}\n").unwrap();
        fs::write(directory.path().join("ignored.txt"), "not indexed\n").unwrap();
        fs::write(directory.path().join("binary.bin"), [1_u8, 0, 2]).unwrap();
        let scanner = WorkspaceScanner::new(Arc::new(AnalyzerRegistry::default()), 1_024);

        let scan = scanner.scan(directory.path()).unwrap();
        let files = scan.files;
        assert!(
            files
                .iter()
                .any(|file| file.relative_path == Path::new("visible.rs"))
        );
        assert!(
            !files
                .iter()
                .any(|file| file.relative_path == Path::new("ignored.txt"))
        );
        assert!(
            !files
                .iter()
                .any(|file| file.relative_path == Path::new("binary.bin"))
        );
        assert!(
            !files
                .iter()
                .any(|file| file.relative_path == Path::new(".git/config"))
        );
    }

    #[test]
    fn scan_applies_explicit_include_and_exclude_patterns() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("keep.rs"), "fn keep() {}\n").unwrap();
        fs::write(directory.path().join("skip.rs"), "fn skip() {}\n").unwrap();
        fs::write(directory.path().join("notes.txt"), "notes\n").unwrap();
        let scanner = WorkspaceScanner::with_patterns(
            Arc::new(AnalyzerRegistry::default()),
            1_024,
            vec!["*.rs".into()],
            vec!["skip.rs".into()],
        );

        let paths: Vec<_> = scanner
            .scan(directory.path())
            .unwrap()
            .files
            .into_iter()
            .map(|file| file.relative_path)
            .collect();
        assert_eq!(paths, vec![PathBuf::from("keep.rs")]);
    }
}
