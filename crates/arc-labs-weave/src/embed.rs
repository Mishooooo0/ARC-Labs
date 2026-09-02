//! Embedding notes.
//!
//! Same shape as the generation client in `arc-labs-runtime`: plain HTTP by
//! hand, no dependency, and a deterministic mock so everything above it is
//! testable with Ollama switched off.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use arc_labs_runtime::llm::{parse_endpoint, LlmError};

pub type Result<T> = std::result::Result<T, LlmError>;

/// nomic-embed-text's width. The index's vector column is declared with it, so
/// a model of a different width is a schema mismatch rather than a silent
/// dimension error at query time.
pub const DIMENSIONS: usize = 768;

pub trait Embedder: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    /// Embed a batch. One call per batch, because the per-request overhead
    /// dominates for short notes.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
}

pub struct OllamaEmbedder {
    pub endpoint: String,
    pub model: String,
    pub timeout: Duration,
}

impl OllamaEmbedder {
    pub fn new(endpoint: impl Into<String>, model: impl Into<String>) -> OllamaEmbedder {
        OllamaEmbedder {
            endpoint: endpoint.into(),
            model: model.into(),
            timeout: Duration::from_secs(120),
        }
    }
}

impl Embedder for OllamaEmbedder {
    fn name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        DIMENSIONS
    }

    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let (host, port) = parse_endpoint(&self.endpoint)?;
        let body = serde_json::json!({ "model": self.model, "input": texts }).to_string();

        let addr = {
            use std::net::ToSocketAddrs;
            (host.as_str(), port)
                .to_socket_addrs()
                .map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?
                .next()
                .ok_or_else(|| LlmError::Unreachable(self.endpoint.clone()))?
        };
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(5))
            .map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?;
        let _ = stream.set_read_timeout(Some(self.timeout));

        let head = format!(
            "POST /api/embed HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(body.as_bytes()))
            .map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?;

        let mut reader = BufReader::new(stream);
        let status = read_status(&mut reader)?;
        let mut payload = String::new();
        let _ = reader.read_to_string(&mut payload);

        if status == 404 {
            return Err(LlmError::NoSuchModel(self.model.clone()));
        }
        // Chunked responses put hex length markers around the JSON.
        let json = payload
            .find('{')
            .map(|i| &payload[i..])
            .and_then(|s| s.rfind('}').map(|j| &s[..=j]))
            .unwrap_or(&payload);

        let parsed: serde_json::Value =
            serde_json::from_str(json).map_err(|e| LlmError::Model(e.to_string()))?;
        if let Some(err) = parsed["error"].as_str() {
            return Err(LlmError::Model(err.to_string()));
        }

        let rows = parsed["embeddings"]
            .as_array()
            .ok_or_else(|| LlmError::Model("no embeddings in the response".into()))?;
        Ok(rows
            .iter()
            .map(|row| {
                row.as_array()
                    .map(|v| {
                        v.iter()
                            .filter_map(|x| x.as_f64())
                            .map(|x| x as f32)
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect())
    }
}

fn read_status<R: BufRead>(reader: &mut R) -> Result<u16> {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .map_err(|e| LlmError::Model(e.to_string()))?;
    let status = line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) if line.trim().is_empty() => break,
            Ok(_) => {}
            Err(e) => return Err(LlmError::Model(e.to_string())),
        }
    }
    Ok(status)
}

/// A deterministic embedder.
///
/// Hashes text to a stable unit vector. Nearby text does **not** produce nearby
/// vectors, and that is the point: a test asserting that Weave proposes a link
/// must construct the situation it wants rather than relying on a fake model to
/// have opinions. Determinism is the feature; semantics are not simulated.
#[derive(Debug, Clone, Default)]
pub struct MockEmbedder;

impl MockEmbedder {
    pub fn vector_for(text: &str) -> Vec<f32> {
        let seed = blake3::hash(text.as_bytes());
        let bytes = seed.as_bytes();
        let mut v: Vec<f32> = (0..DIMENSIONS)
            .map(|i| {
                let a = bytes[i % bytes.len()] as f32;
                let b = bytes[(i * 7 + 3) % bytes.len()] as f32;
                (a - 128.0) / 128.0 + (b - 128.0) / 4096.0
            })
            .collect();
        normalise(&mut v);
        v
    }
}

impl Embedder for MockEmbedder {
    fn name(&self) -> &str {
        "mock-embed"
    }
    fn dimensions(&self) -> usize {
        DIMENSIONS
    }
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| MockEmbedder::vector_for(t)).collect())
    }
}

/// Scale to unit length, so cosine similarity is a dot product.
pub fn normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Little-endian bytes, the layout `sqlite-vec` stores.
pub fn to_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// What actually gets embedded for a note.
///
/// Title plus a prefix of the body, not the whole note. Two reasons: an
/// embedding model has a context limit and truncates silently past it, and the
/// opening of a note is what it is *about* — the tail of a long note is usually
/// detail that dilutes the signal rather than sharpening it.
pub fn text_for_note(title: &str, body: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(max_chars.min(body.len()) + title.len() + 2);
    out.push_str(title);
    out.push_str("\n\n");
    for ch in body.chars() {
        if out.len() >= max_chars {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mock_is_deterministic_and_normalised() {
        let a = MockEmbedder::vector_for("a note about provenance");
        assert_eq!(a.len(), DIMENSIONS);
        assert_eq!(a, MockEmbedder::vector_for("a note about provenance"));
        assert_ne!(a, MockEmbedder::vector_for("a different note"));

        let norm = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "not a unit vector: {norm}");
    }

    #[test]
    fn embedding_a_batch_returns_one_vector_per_input() {
        let texts: Vec<String> = ["one", "two", "three"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = MockEmbedder.embed(&texts).unwrap();
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|v| v.len() == DIMENSIONS));
        assert!(MockEmbedder.embed(&[]).unwrap().is_empty());
    }

    #[test]
    fn note_text_is_title_plus_a_bounded_prefix() {
        let body = "x".repeat(10_000);
        let t = text_for_note("Title", &body, 500);
        assert!(t.starts_with("Title\n\n"));
        assert!(t.len() <= 520, "got {} chars", t.len());
    }

    #[test]
    fn note_text_truncation_never_splits_a_character() {
        // Byte-based truncation on multi-byte text would panic or corrupt.
        let body = "日本語のテキストがここにあります。".repeat(50);
        let t = text_for_note("タイトル", &body, 100);
        assert!(t.chars().count() > 0);
        // Round-trips, i.e. it is still valid UTF-8 with whole characters.
        assert_eq!(t, String::from_utf8(t.clone().into_bytes()).unwrap());
    }

    #[test]
    fn bytes_are_little_endian_and_the_right_length() {
        let v = vec![1.0f32, -2.0, 0.5];
        let b = to_bytes(&v);
        assert_eq!(b.len(), 12);
        assert_eq!(&b[..4], &1.0f32.to_le_bytes());
    }

    #[test]
    fn an_unreachable_endpoint_fails_rather_than_hanging() {
        let e = OllamaEmbedder::new("http://127.0.0.1:1", "nomic-embed-text");
        let err = e.embed(&["x".to_string()]).unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)), "got {err:?}");
    }
}
