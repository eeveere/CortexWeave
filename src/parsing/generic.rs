use std::path::Path;

use serde_json::json;

use crate::{
    Result,
    domain::{AnalyzedChunk, AnalyzerCapabilities},
};

use super::LanguageAnalyzer;

#[derive(Debug, Clone)]
pub struct GenericAnalyzer {
    target_chars: usize,
    overlap_chars: usize,
}

impl GenericAnalyzer {
    pub fn new(target_chars: usize, overlap_chars: usize) -> Self {
        Self {
            target_chars: target_chars.max(1),
            overlap_chars: overlap_chars.min(target_chars.saturating_sub(1)),
        }
    }
}

impl Default for GenericAnalyzer {
    fn default() -> Self {
        Self::new(3_000, 300)
    }
}

impl LanguageAnalyzer for GenericAnalyzer {
    fn language_id(&self) -> &'static str {
        "text"
    }
    fn analyzer_id(&self) -> &'static str {
        "generic"
    }
    fn analyzer_version(&self) -> String {
        format!(
            "2-target{}-overlap{}",
            self.target_chars, self.overlap_chars
        )
    }
    fn extensions(&self) -> &'static [&'static str] {
        &[]
    }
    fn capabilities(&self) -> AnalyzerCapabilities {
        AnalyzerCapabilities::default()
    }

    fn analyze(&self, path: &Path, source: &str) -> Result<Vec<AnalyzedChunk>> {
        if source.is_empty() {
            return Ok(Vec::new());
        }

        let boundaries = chunk_boundaries(source, self.target_chars, self.overlap_chars);
        let path = path.to_string_lossy().replace('\\', "/");
        Ok(boundaries
            .into_iter()
            .enumerate()
            .map(|(index, (start, end))| {
                let start_line = source[..start]
                    .bytes()
                    .filter(|byte| *byte == b'\n')
                    .count()
                    + 1;
                let end_line = start_line
                    + source[start..end]
                        .bytes()
                        .filter(|byte| *byte == b'\n')
                        .count();
                AnalyzedChunk {
                    stable_key: format!("{path}::text:{index}"),
                    language: "text".into(),
                    symbol: None,
                    qualified_symbol: None,
                    symbol_kind: None,
                    start_byte: start,
                    end_byte: end,
                    start_line,
                    end_line,
                    content: source[start..end].to_owned(),
                    metadata: json!({"chunk_index": index}),
                }
            })
            .collect())
    }
}

fn chunk_boundaries(source: &str, target: usize, overlap: usize) -> Vec<(usize, usize)> {
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < source.len() {
        let target_end = (start + target).min(source.len());
        let mut end = floor_char_boundary(source, target_end);
        if end < source.len()
            && let Some(heading_end) = markdown_heading_boundary(&source[start..end])
        {
            end = start + heading_end;
        }
        if end < source.len()
            && let Some(relative) = source[start..end].rfind("\n\n")
        {
            let paragraph_end = start + relative + 2;
            if paragraph_end > start {
                end = paragraph_end;
            }
        }
        if end <= start {
            end = next_char_boundary(source, start);
        }
        chunks.push((start, end));
        if end == source.len() {
            break;
        }
        let proposed = end.saturating_sub(overlap).max(start + 1);
        start = next_char_boundary_at_or_after(source, proposed).min(end);
    }

    chunks
}

fn markdown_heading_boundary(chunk: &str) -> Option<usize> {
    chunk
        .rmatch_indices("\n#")
        .map(|(index, _)| index + 1)
        .find(|index| *index > 0)
}

fn floor_char_boundary(source: &str, mut index: usize) -> usize {
    while index > 0 && !source.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn next_char_boundary(source: &str, index: usize) -> usize {
    source[index..]
        .char_indices()
        .nth(1)
        .map_or(source.len(), |(offset, _)| index + offset)
}

fn next_char_boundary_at_or_after(source: &str, mut index: usize) -> usize {
    index = index.min(source.len());
    while index < source.len() && !source.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn chunks_unicode_without_breaking_utf8() {
        let analyzer = GenericAnalyzer::new(7, 2);
        let chunks = analyzer
            .analyze(Path::new("notes.md"), "alpha βeta gamma")
            .unwrap();
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
        assert!(chunks.iter().all(|chunk| chunk.start_byte < chunk.end_byte));
    }

    #[test]
    fn always_progresses_when_overlap_lands_inside_utf8() {
        let chunks = chunk_boundaries("ββββ", 2, 1);
        assert_eq!(chunks, vec![(0, 2), (2, 4), (4, 6), (6, 8)]);
    }

    #[test]
    fn prefers_markdown_heading_boundaries() {
        let chunks = chunk_boundaries("intro words\n# Next\nbody", 18, 0);
        assert_eq!(chunks[0], (0, 12));
    }
}
