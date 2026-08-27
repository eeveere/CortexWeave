use crate::{CortexError, Result, domain::AnalyzedChunk, embedding::EmbeddingProvider};

const SEGMENTER_VERSION: &str = "embedding-segmenter-v1";

#[derive(Debug)]
pub(crate) struct SegmentationOutput {
    pub chunks: Vec<AnalyzedChunk>,
    pub identity: String,
    pub split_count: usize,
}

pub(crate) fn policy_identity(
    provider: &dyn EmbeddingProvider,
    overlap_tokens: usize,
    effective_max_input_tokens: Option<usize>,
) -> String {
    let limits = provider.limits();
    let policy = format!(
        "{SEGMENTER_VERSION}\0{:?}\0{}\0{}\0{}\0{}",
        effective_max_input_tokens,
        limits.reserved_tokens,
        overlap_tokens,
        provider.token_counter_id(),
        provider.document_transformation_id()
    );
    let hash = blake3::hash(policy.as_bytes()).to_hex();
    format!("{SEGMENTER_VERSION}:{}", &hash[..16])
}

pub(crate) fn segment_chunks(
    logical_chunks: &[AnalyzedChunk],
    provider: &dyn EmbeddingProvider,
    overlap_tokens: usize,
    effective_max_input_tokens: Option<usize>,
) -> Result<SegmentationOutput> {
    let identity = policy_identity(provider, overlap_tokens, effective_max_input_tokens);
    let Some(max_input_tokens) = effective_max_input_tokens else {
        return Ok(SegmentationOutput {
            chunks: logical_chunks.to_vec(),
            identity,
            split_count: 0,
        });
    };
    let reserved = provider.limits().reserved_tokens;
    let budget = max_input_tokens.checked_sub(reserved).ok_or_else(|| {
        CortexError::Configuration(format!(
            "embedding input limit {max_input_tokens} cannot accommodate {reserved} reserved tokens"
        ))
    })?;
    if budget == 0 {
        return Err(CortexError::Configuration(
            "embedding input budget must leave at least one token for source".into(),
        ));
    }

    let mut chunks = Vec::new();
    let mut split_count = 0;
    for chunk in logical_chunks {
        let prepared = provider.prepare_document_input(&chunk.content);
        if provider.count_tokens(&prepared).tokens <= budget {
            chunks.push(chunk.clone());
            continue;
        }
        let segments = split_chunk(chunk, provider, budget, overlap_tokens)?;
        split_count += segments.len().saturating_sub(1);
        chunks.extend(segments);
    }
    Ok(SegmentationOutput {
        chunks,
        identity,
        split_count,
    })
}

fn split_chunk(
    chunk: &AnalyzedChunk,
    provider: &dyn EmbeddingProvider,
    budget: usize,
    overlap_tokens: usize,
) -> Result<Vec<AnalyzedChunk>> {
    if chunk.content.is_empty() {
        return Err(CortexError::Analysis(format!(
            "oversized logical chunk {} has no source content",
            chunk.stable_key
        )));
    }
    let prefix_tokens = provider
        .count_tokens(&provider.prepare_document_input(""))
        .tokens;
    if prefix_tokens >= budget {
        return Err(CortexError::Configuration(format!(
            "document transformation consumes {prefix_tokens} tokens, leaving no usable input budget of {budget}"
        )));
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    let effective_overlap_tokens = overlap_tokens.min((budget - prefix_tokens) / 4);
    while start < chunk.content.len() {
        let max_end = furthest_fitting_end(&chunk.content, start, provider, budget)?;
        if max_end <= start {
            return Err(CortexError::Analysis(format!(
                "embedding input budget {budget} cannot fit one Unicode scalar from {}",
                chunk.stable_key
            )));
        }
        let end = preferred_boundary(&chunk.content, start, max_end);
        ranges.push((start, end));
        if end == chunk.content.len() {
            break;
        }
        let overlap_start = overlap_start(
            &chunk.content,
            start,
            end,
            provider,
            effective_overlap_tokens,
        );
        start = if overlap_start > start {
            overlap_start
        } else {
            next_char_boundary(&chunk.content, start)
        };
    }

    let part_count = ranges.len();
    Ok(ranges
        .into_iter()
        .enumerate()
        .map(|(part_index, (start, end))| {
            let mut segment = chunk.clone();
            segment.stable_key = format!("{}::segment:{part_index}", chunk.stable_key);
            segment.start_byte = chunk.start_byte + start;
            segment.end_byte = chunk.start_byte + end;
            segment.start_line = chunk.start_line + newline_count(&chunk.content[..start]);
            segment.end_line = segment.start_line + newline_count(&chunk.content[start..end]);
            segment.content = chunk.content[start..end].to_owned();
            let mut metadata = match chunk.metadata.clone() {
                serde_json::Value::Object(metadata) => metadata,
                original => {
                    let mut metadata = serde_json::Map::new();
                    metadata.insert("logical_metadata".into(), original);
                    metadata
                }
            };
            metadata.insert(
                "parent_logical_stable_key".into(),
                serde_json::Value::String(chunk.stable_key.clone()),
            );
            metadata.insert("segment_part_index".into(), part_index.into());
            metadata.insert("segment_part_count".into(), part_count.into());
            segment.metadata = serde_json::Value::Object(metadata);
            segment
        })
        .collect())
}

fn furthest_fitting_end(
    source: &str,
    start: usize,
    provider: &dyn EmbeddingProvider,
    budget: usize,
) -> Result<usize> {
    let boundaries: Vec<usize> = source[start..]
        .char_indices()
        .skip(1)
        .map(|(offset, _)| start + offset)
        .chain(std::iter::once(source.len()))
        .collect();
    let mut low = 0;
    let mut high = boundaries.len();
    while low < high {
        let middle = low + (high - low) / 2;
        let prepared = provider.prepare_document_input(&source[start..boundaries[middle]]);
        if provider.count_tokens(&prepared).tokens <= budget {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    if low == 0 {
        let end = boundaries[0];
        let prepared = provider.prepare_document_input(&source[start..end]);
        if provider.count_tokens(&prepared).tokens > budget {
            return Ok(start);
        }
    }
    Ok(boundaries[low.saturating_sub(1)])
}

fn preferred_boundary(source: &str, start: usize, max_end: usize) -> usize {
    if max_end == source.len() {
        return max_end;
    }
    let minimum = start + (max_end - start) / 2;
    let candidate = &source[start..max_end];
    let after = |relative: usize, width: usize| start + relative + width;

    if let Some((relative, width)) = ["\n\n", "\r\n\r\n"]
        .into_iter()
        .filter_map(|boundary| {
            candidate
                .rfind(boundary)
                .map(|index| (index, boundary.len()))
        })
        .filter(|(index, width)| after(*index, *width) >= minimum)
        .max_by_key(|(index, width)| after(*index, *width))
    {
        return after(relative, width);
    }
    if let Some(relative) = candidate
        .rmatch_indices('\n')
        .map(|(index, _)| index)
        .find(|index| after(*index, 1) >= minimum)
    {
        return after(relative, 1);
    }
    if let Some((relative, character)) =
        candidate.char_indices().rev().find(|(index, character)| {
            character.is_whitespace() && after(*index, character.len_utf8()) >= minimum
        })
    {
        return after(relative, character.len_utf8());
    }
    max_end
}

fn overlap_start(
    source: &str,
    segment_start: usize,
    end: usize,
    provider: &dyn EmbeddingProvider,
    overlap_tokens: usize,
) -> usize {
    if overlap_tokens == 0 {
        return end;
    }
    let mut result = end;
    for (offset, _) in source[segment_start..end].char_indices().rev() {
        let candidate = segment_start + offset;
        if provider.count_tokens(&source[candidate..end]).tokens > overlap_tokens {
            break;
        }
        result = candidate;
    }
    result
}

fn next_char_boundary(source: &str, start: usize) -> usize {
    start + source[start..].chars().next().map_or(0, char::len_utf8)
}

fn newline_count(source: &str) -> usize {
    source.bytes().filter(|byte| *byte == b'\n').count()
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use serde_json::json;

    use super::*;
    use crate::{
        domain::SymbolKind,
        embedding::{EmbeddingLimits, TokenCount},
    };

    struct FixtureProvider {
        max_input: usize,
        prefix: &'static str,
    }

    #[async_trait]
    impl EmbeddingProvider for FixtureProvider {
        async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![vec![1.0]; texts.len()])
        }
        fn model_name(&self) -> &str {
            "fixture"
        }
        fn prepare_document_input(&self, text: &str) -> String {
            format!("{}{text}", self.prefix)
        }
        fn count_tokens(&self, text: &str) -> TokenCount {
            TokenCount {
                tokens: text.chars().count(),
                accuracy: crate::embedding::TokenCountAccuracy::Exact,
            }
        }
        fn token_counter_id(&self) -> &str {
            "fixture-char-v1"
        }
        fn limits(&self) -> EmbeddingLimits {
            EmbeddingLimits {
                max_input_tokens: Some(self.max_input),
                max_batch_tokens: None,
                max_batch_items: 8,
                reserved_tokens: 0,
            }
        }
    }

    fn logical(content: &str) -> AnalyzedChunk {
        AnalyzedChunk {
            stable_key: "src/lib.rs::function:large".into(),
            language: "rust".into(),
            symbol: Some("large".into()),
            qualified_symbol: Some("large".into()),
            symbol_kind: Some(SymbolKind::Function),
            start_byte: 10,
            end_byte: 10 + content.len(),
            start_line: 3,
            end_line: 3 + newline_count(content),
            content: content.into(),
            metadata: json!({"node_kind": "function_item"}),
        }
    }

    #[test]
    fn preserves_exact_fit_and_splits_one_over_with_prefix() {
        let provider = FixtureProvider {
            max_input: 8,
            prefix: "p:",
        };
        let exact = segment_chunks(&[logical("123456")], &provider, 0, Some(8)).unwrap();
        assert_eq!(exact.chunks[0].stable_key, "src/lib.rs::function:large");

        let split = segment_chunks(&[logical("1234567")], &provider, 0, Some(8)).unwrap();
        assert_eq!(split.chunks.len(), 2);
        assert_eq!(
            split.chunks[0].stable_key,
            "src/lib.rs::function:large::segment:0"
        );
        assert_eq!(split.chunks.concat_content(), "1234567");
    }

    #[test]
    fn uses_utf8_safe_preferred_boundaries_and_bounded_overlap() {
        let provider = FixtureProvider {
            max_input: 12,
            prefix: "",
        };
        let content = "alpha βeta\n\nsecond line and tail";
        let output = segment_chunks(&[logical(content)], &provider, 2, Some(12)).unwrap();
        assert!(output.chunks.len() > 2);
        assert!(output.chunks[0].content.ends_with("\n\n"));
        assert!(output.chunks.iter().all(|chunk| {
            provider
                .count_tokens(&provider.prepare_document_input(&chunk.content))
                .tokens
                <= 12
        }));
        assert_eq!(output.chunks[0].start_byte, 10);
    }

    trait ChunkContents {
        fn concat_content(&self) -> String;
    }

    impl ChunkContents for [AnalyzedChunk] {
        fn concat_content(&self) -> String {
            self.iter().map(|chunk| chunk.content.as_str()).collect()
        }
    }
}
