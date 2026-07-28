use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub ordinal: usize,
    pub content: String,
    pub content_hash: String,
    pub start_byte: usize,
    pub end_byte: usize,
}

pub fn sha256_hex(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    format!("{digest:x}")
}

pub fn chunk_text(input: &str, max_chars: usize, overlap_chars: usize) -> Vec<TextChunk> {
    let trimmed = input.trim();
    if trimmed.is_empty() || max_chars == 0 {
        return Vec::new();
    }
    let input_offset = input.len() - input.trim_start().len();

    let boundaries = trimmed
        .char_indices()
        .map(|(idx, _)| idx)
        .chain(std::iter::once(trimmed.len()))
        .collect::<Vec<_>>();
    let char_count = boundaries.len().saturating_sub(1);
    let overlap = overlap_chars.min(max_chars.saturating_sub(1));
    let mut chunks = Vec::new();
    let mut start_char = 0usize;

    while start_char < char_count {
        let hard_end = (start_char + max_chars).min(char_count);
        let mut end_char = hard_end;
        if hard_end < char_count {
            let preferred_start = start_char + (max_chars * 3 / 5);
            for candidate in (preferred_start..hard_end).rev() {
                let byte = boundaries[candidate];
                let next = trimmed[byte..].chars().next().unwrap_or(' ');
                if next.is_whitespace() {
                    end_char = candidate;
                    break;
                }
            }
        }
        if end_char <= start_char {
            end_char = hard_end;
        }

        let raw_start_byte = boundaries[start_char];
        let raw_end_byte = boundaries[end_char];
        let raw_content = &trimmed[raw_start_byte..raw_end_byte];
        let content = raw_content.trim();
        if !content.is_empty() {
            let content_offset = raw_content
                .find(content)
                .expect("trimmed content must be inside its source slice");
            let start_byte = input_offset + raw_start_byte + content_offset;
            let end_byte = start_byte + content.len();
            chunks.push(TextChunk {
                ordinal: chunks.len(),
                content_hash: sha256_hex(content),
                content: content.to_string(),
                start_byte,
                end_byte,
            });
        }
        if end_char >= char_count {
            break;
        }
        start_char = end_char.saturating_sub(overlap);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::chunk_text;

    #[test]
    fn chunks_utf8_without_breaking_characters() {
        let input = "Moon remembers carefully. 月亮会记住重要的决定。 ".repeat(20);
        let chunks = chunk_text(&input, 80, 10);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| !chunk.content.is_empty()));
        assert!(chunks.iter().all(|chunk| chunk.content.len() <= 400));
    }

    #[test]
    fn empty_input_has_no_chunks() {
        assert!(chunk_text("   ", 100, 10).is_empty());
    }

    #[test]
    fn chunk_offsets_select_the_exact_stored_content() {
        let input = "\n  Alpha memory. 月亮 remembers carefully.  \n";
        let chunks = chunk_text(input, 18, 4);
        assert!(chunks.len() > 1);
        for chunk in chunks {
            assert_eq!(
                &input.as_bytes()[chunk.start_byte..chunk.end_byte],
                chunk.content.as_bytes()
            );
        }
    }
}
