use std::{cell::RefCell, path::Path};

use anyhow::{Context, Result, bail};
use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};
use sha2::{Digest, Sha256};

pub trait EmbeddingProvider {
    fn name(&self) -> &str;
    fn model(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
    fn embed_query(&self, input: &str) -> Result<Vec<f32>>;
}

#[derive(Debug, Clone)]
pub struct HashEmbedding {
    dimensions: usize,
}

impl HashEmbedding {
    pub fn new(dimensions: usize) -> Self {
        Self { dimensions }
    }

    fn embed_one(&self, input: &str) -> Vec<f32> {
        let mut vector = vec![0.0f32; self.dimensions];
        for token in input
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|token| !token.is_empty())
        {
            let token = token.to_ascii_lowercase();
            let digest = Sha256::digest(token.as_bytes());
            let index = u64::from_le_bytes(digest[..8].try_into().expect("sha prefix")) as usize
                % self.dimensions;
            let sign = if digest[8] & 1 == 0 { 1.0 } else { -1.0 };
            vector[index] += sign;
        }
        normalize(&mut vector);
        vector
    }
}

impl EmbeddingProvider for HashEmbedding {
    fn name(&self) -> &str {
        "hash"
    }

    fn model(&self) -> &str {
        "moon-hash-v1"
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(inputs.iter().map(|input| self.embed_one(input)).collect())
    }

    fn embed_query(&self, input: &str) -> Result<Vec<f32>> {
        Ok(self.embed_one(input))
    }
}

const LOCAL_DIMENSIONS: usize = 384;
const LOCAL_MAX_TOKENS: usize = 512;
const LOCAL_BATCH_SIZE: usize = 64;

/// In-process multilingual embeddings. The model is downloaded once into Moon's
/// private cache and then runs fully locally through ONNX Runtime.
pub struct LocalEmbedding {
    model: RefCell<TextEmbedding>,
}

impl LocalEmbedding {
    pub fn new(cache_dir: &Path, dimensions: usize) -> Result<Self> {
        if dimensions != LOCAL_DIMENSIONS {
            bail!("local embedding model requires {LOCAL_DIMENSIONS} dimensions, got {dimensions}");
        }

        std::fs::create_dir_all(cache_dir)
            .with_context(|| format!("create model cache {}", cache_dir.display()))?;
        let threads = std::thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(2)
            .min(4);
        let options = TextInitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_cache_dir(cache_dir.to_path_buf())
            .with_show_download_progress(false)
            .with_max_length(LOCAL_MAX_TOKENS)
            .with_intra_threads(threads);
        let model = TextEmbedding::try_new(options)
            .context("load intfloat/multilingual-e5-small local embedding model")?;
        Ok(Self {
            model: RefCell::new(model),
        })
    }

    fn embed_prefixed(&self, prefix: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }

        let mut model = self.model.borrow_mut();
        let mut flattened = Vec::new();
        let mut segment_counts = Vec::with_capacity(inputs.len());
        for input in inputs {
            let segments = split_to_token_limit(&model, prefix, input)?;
            segment_counts.push(segments.len());
            flattened.extend(segments);
        }

        let raw = model
            .embed(flattened, Some(LOCAL_BATCH_SIZE))
            .context("run local embedding inference")?;
        let mut cursor = 0;
        let mut pooled = Vec::with_capacity(inputs.len());
        for count in segment_counts {
            let mut vector = vec![0.0f32; LOCAL_DIMENSIONS];
            for segment in &raw[cursor..cursor + count] {
                for (target, value) in vector.iter_mut().zip(segment) {
                    *target += *value;
                }
            }
            cursor += count;
            normalize(&mut vector);
            pooled.push(vector);
        }
        Ok(pooled)
    }
}

impl EmbeddingProvider for LocalEmbedding {
    fn name(&self) -> &str {
        "local"
    }

    fn model(&self) -> &str {
        "intfloat/multilingual-e5-small@fastembed-5.17.3:query-passage-v1:max512"
    }

    fn dimensions(&self) -> usize {
        LOCAL_DIMENSIONS
    }

    fn embed_documents(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_prefixed("passage: ", inputs)
    }

    fn embed_query(&self, input: &str) -> Result<Vec<f32>> {
        let mut vectors = self.embed_prefixed("query: ", &[input.to_owned()])?;
        Ok(vectors.remove(0))
    }
}

fn split_to_token_limit(model: &TextEmbedding, prefix: &str, input: &str) -> Result<Vec<String>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(vec![prefix.trim_end().to_owned()]);
    }

    let whole = format!("{prefix}{input}");
    if token_count(model, &whole)? <= LOCAL_MAX_TOKENS {
        return Ok(vec![whole]);
    }

    let boundaries = input
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(input.len()))
        .collect::<Vec<_>>();
    let mut start = 0;
    let mut result = Vec::new();
    while start < input.len() {
        let start_position = boundaries.partition_point(|boundary| *boundary <= start);
        let mut low = start_position;
        let mut high = boundaries.len() - 1;
        let mut best = start;
        while low <= high {
            let middle = low + (high - low) / 2;
            let end = boundaries[middle];
            let candidate = format!("{prefix}{}", &input[start..end]);
            if token_count(model, &candidate)? <= LOCAL_MAX_TOKENS {
                best = end;
                low = middle + 1;
            } else if middle == 0 {
                break;
            } else {
                high = middle - 1;
            }
        }
        if best == start {
            bail!("embedding tokenizer could not fit one character within the token limit");
        }

        let minimum_natural_break = start + ((best - start) * 4 / 5);
        let natural_end = input[start..best]
            .char_indices()
            .rev()
            .find_map(|(offset, character)| {
                let end = start + offset + character.len_utf8();
                (end >= minimum_natural_break && character.is_whitespace()).then_some(end)
            })
            .unwrap_or(best);
        let segment = input[start..natural_end].trim();
        if !segment.is_empty() {
            result.push(format!("{prefix}{segment}"));
        }
        start = natural_end;
        while start < input.len() {
            let character = input[start..]
                .chars()
                .next()
                .expect("valid character boundary");
            if !character.is_whitespace() {
                break;
            }
            start += character.len_utf8();
        }
    }
    Ok(result)
}

fn token_count(model: &TextEmbedding, input: &str) -> Result<usize> {
    model
        .tokenizer
        .encode(input, true)
        .map(|encoding| encoding.len())
        .map_err(|error| anyhow::anyhow!("tokenize embedding input: {error}"))
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for value in vector {
            *value /= norm;
        }
    }
}

pub fn vector_to_blob(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(vector));
    for value in vector {
        bytes.extend_from_slice(&value.to_ne_bytes());
    }
    bytes
}
