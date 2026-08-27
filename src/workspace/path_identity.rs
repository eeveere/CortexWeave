use std::path::{Path, PathBuf};

use crate::{CortexError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PathIdentity {
    display_path: PathBuf,
    comparison_key: String,
    canonicalized: bool,
}

impl PathIdentity {
    pub(crate) fn existing_directory(path: &Path) -> Result<Self> {
        let identity = Self::from_path(path)?;
        if !identity.canonicalized || !identity.display_path.is_dir() {
            return Err(CortexError::Configuration(format!(
                "workspace is not a directory: {}",
                path.display()
            )));
        }
        Ok(identity)
    }

    pub(crate) fn from_path(path: &Path) -> Result<Self> {
        if path.as_os_str().is_empty() {
            return Err(CortexError::Configuration(
                "workspace path cannot be empty".into(),
            ));
        }
        let lexical_key = lexical_comparison_key(&path.to_string_lossy())?;
        if !is_absolute_key(&lexical_key) {
            return Err(CortexError::Configuration(format!(
                "workspace path must be absolute: {}",
                path.display()
            )));
        }
        let platform_path = PathBuf::from(platform_path_text(&lexical_key));
        match std::fs::canonicalize(&platform_path) {
            Ok(canonical) => {
                let display_path = clean_display_path(&canonical);
                Ok(Self {
                    comparison_key: lexical_comparison_key(&display_path.to_string_lossy())?,
                    display_path,
                    canonicalized: true,
                })
            }
            Err(_) => Ok(Self {
                display_path: platform_path,
                comparison_key: lexical_key,
                canonicalized: false,
            }),
        }
    }

    pub(crate) fn from_file_uri(uri: &str) -> Result<Self> {
        let url = url::Url::parse(uri).map_err(|error| {
            CortexError::Configuration(format!("invalid workspace file URI: {error}"))
        })?;
        if url.scheme() != "file" {
            return Err(CortexError::Configuration(format!(
                "workspace URI must use the file scheme: {uri}"
            )));
        }
        let path = url.to_file_path().map_err(|_| {
            CortexError::Configuration(format!("workspace file URI is not a local path: {uri}"))
        })?;
        Self::from_path(&path)
    }

    pub(crate) fn display_path(&self) -> &Path {
        &self.display_path
    }

    pub(crate) fn comparison_key(&self) -> &str {
        &self.comparison_key
    }

    pub(crate) fn contains(&self, candidate: &Self) -> bool {
        if self.comparison_key == candidate.comparison_key {
            return true;
        }
        if self.comparison_key.ends_with('/') {
            candidate.comparison_key.starts_with(&self.comparison_key)
        } else {
            candidate
                .comparison_key
                .strip_prefix(&self.comparison_key)
                .is_some_and(|remainder| remainder.starts_with('/'))
        }
    }
}

fn clean_display_path(path: &Path) -> PathBuf {
    let value = path.to_string_lossy();
    if let Some(rest) = value.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = value.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

fn platform_path_text(key: &str) -> String {
    #[cfg(windows)]
    {
        key.replace('/', r"\")
    }
    #[cfg(not(windows))]
    {
        key.to_owned()
    }
}

fn lexical_comparison_key(raw: &str) -> Result<String> {
    let mut normalized = raw.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{rest}");
    } else if let Some(rest) = normalized.strip_prefix("//?/") {
        normalized = rest.to_owned();
    }
    if cfg!(windows) && normalized.len() >= 4 {
        let bytes = normalized.as_bytes();
        if bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b'/'
            && bytes[3] != b'/'
        {
            normalized = format!("{}:{}", bytes[1] as char, &normalized[2..]);
        }
    }

    let (prefix, remainder, protected_components) =
        if let Some(rest) = normalized.strip_prefix("//") {
            ("//".to_owned(), rest, 2)
        } else if normalized.as_bytes().get(1) == Some(&b':')
            && normalized.as_bytes()[0].is_ascii_alphabetic()
            && normalized.as_bytes().get(2) == Some(&b'/')
        {
            (
                format!("{}:/", normalized[..1].to_ascii_lowercase()),
                normalized[2..].trim_start_matches('/'),
                0,
            )
        } else if let Some(rest) = normalized.strip_prefix('/') {
            ("/".to_owned(), rest, 0)
        } else {
            (String::new(), normalized.as_str(), 0)
        };

    let mut components: Vec<&str> = Vec::new();
    for component in remainder.split('/') {
        match component {
            "" | "." => {}
            ".." if components.len() > protected_components => {
                components.pop();
            }
            ".." if prefix.is_empty() => components.push(component),
            ".." => {}
            value => components.push(value),
        }
    }
    if prefix == "//" && components.len() < 2 {
        return Err(CortexError::Configuration(format!(
            "UNC workspace path must include server and share: {raw}"
        )));
    }
    let mut key = match prefix.as_str() {
        "//" => format!("//{}", components.join("/")),
        "/" => format!("/{}", components.join("/")),
        value if value.ends_with("/") => format!("{value}{}", components.join("/")),
        _ => components.join("/"),
    };
    while key.len() > 1 && key.ends_with('/') && !is_drive_root(&key) {
        key.pop();
    }
    if cfg!(windows) || looks_like_windows_path(&key) {
        key.make_ascii_lowercase();
    }
    Ok(key)
}

fn is_absolute_key(key: &str) -> bool {
    key.starts_with('/')
        || key.starts_with("//")
        || (key.as_bytes().get(1) == Some(&b':')
            && key.as_bytes()[0].is_ascii_alphabetic()
            && key.as_bytes().get(2) == Some(&b'/'))
}

fn is_drive_root(key: &str) -> bool {
    key.len() == 3 && key.as_bytes()[1] == b':' && key.ends_with('/')
}

fn looks_like_windows_path(key: &str) -> bool {
    key.starts_with("//")
        || (key.as_bytes().get(1) == Some(&b':') && key.as_bytes()[0].is_ascii_alphabetic())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn normalizes_windows_verbatim_slashes_case_and_dot_segments() {
        let expected = "c:/dev/project/src";
        for value in [
            r"C:\dev\project\.\src",
            r"c:/DEV/project/src",
            r"\\?\C:\dev\project\src",
        ] {
            assert_eq!(lexical_comparison_key(value).unwrap(), expected);
        }
        #[cfg(windows)]
        assert_eq!(
            lexical_comparison_key("/c/dev/project/src").unwrap(),
            expected
        );
        assert!(!is_absolute_key(
            &lexical_comparison_key(r"C:relative\path").unwrap()
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn preserves_posix_paths_beneath_single_letter_roots() {
        assert_eq!(
            lexical_comparison_key("/c/dev/project/src").unwrap(),
            "/c/dev/project/src"
        );
    }

    #[test]
    fn normalizes_unc_without_losing_server_or_share() {
        assert_eq!(
            lexical_comparison_key(r"\\?\UNC\Server\Share\repo\..\src").unwrap(),
            "//server/share/src"
        );
        assert!(lexical_comparison_key(r"\\server").is_err());
    }

    #[test]
    fn containment_observes_component_boundaries() {
        let root = PathIdentity {
            display_path: PathBuf::from("C:/dev/repo"),
            comparison_key: "c:/dev/repo".into(),
            canonicalized: false,
        };
        let child = PathIdentity {
            display_path: PathBuf::from("C:/dev/repo/src"),
            comparison_key: "c:/dev/repo/src".into(),
            canonicalized: false,
        };
        let sibling = PathIdentity {
            display_path: PathBuf::from("C:/dev/repository"),
            comparison_key: "c:/dev/repository".into(),
            canonicalized: false,
        };
        assert!(root.contains(&child));
        assert!(!root.contains(&sibling));
    }

    #[test]
    fn file_uri_decodes_to_the_same_existing_path_identity() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("workspace with spaces");
        std::fs::create_dir(&root).unwrap();
        let uri = url::Url::from_directory_path(&root).unwrap();

        let from_path = PathIdentity::from_path(&root).unwrap();
        let from_uri = PathIdentity::from_file_uri(uri.as_str()).unwrap();

        assert_eq!(from_uri.comparison_key(), from_path.comparison_key());
        assert!(from_uri.canonicalized);
    }

    #[test]
    fn rejects_non_file_uris() {
        let error = PathIdentity::from_file_uri("https://example.test/repo").unwrap_err();
        assert!(error.to_string().contains("file scheme"));
    }
}
