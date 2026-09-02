//! Seams for the two external systems ARC-LABS will talk to, and deterministic
//! mocks for both.
//!
//! These land in Phase 0 unused, on purpose. ARC-Cortex proved the pattern: with
//! a `MockEngine` in place from the start, the scheduler, memory and security
//! layers were built and tested before any weights existed. The same trick pays
//! off harder here, because Phase 5 and Phase 6 have acceptance criteria that
//! are *impossible* to test against a real model:
//!
//! - "the same graph run twice produces identical `transform` output" — a real
//!   LLM is not reproducible, so the gate needs a backend that is.
//! - "cancel mid-stream leaves the target note byte-identical" — needs a stream
//!   that can be paused exactly where the test wants it.
//! - "typing latency is unchanged with Weave running" — needs embeddings that
//!   cost a known, controllable amount of time.
//!
//! Keeping the traits here, in a crate with no network dependency, is also what
//! enforces the egress boundary: `arc-labs-core` *cannot* reach the network,
//! because nothing in its dependency tree can.

use std::future::Future;
use std::pin::Pin;

/// A chunk of generated text, or the end of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Text(String),
    Done { tokens: usize },
}

pub type TokenStream = Pin<Box<dyn futures_core::Stream<Item = Token> + Send>>;

/// A borrowed, boxed future. Both seams return one so the traits stay
/// object-safe without requiring `async_trait`'s allocation on every call.
pub type SeamFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SeamError>> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
    /// Temperature 0 means "be reproducible if you can". The mock always is.
    pub temperature: f32,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SeamError {
    #[error("model backend unavailable: {0}")]
    Unavailable(String),
    #[error("model {0} is not installed")]
    NoSuchModel(String),
    #[error("run was cancelled")]
    Cancelled,
}

/// Text generation. Implemented by Ollama in Phase 5 and by [`MockLlm`] in tests.
pub trait LlmBackend: Send + Sync {
    fn name(&self) -> &str;
    fn generate(&self, req: GenerateRequest) -> SeamFuture<'_, TokenStream>;
}

/// Embedding. Implemented by Ollama's `nomic-embed-text` in Phase 6 and by
/// [`MockEmbedder`] in tests.
pub trait Embedder: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> usize;
    fn embed(&self, texts: Vec<String>) -> SeamFuture<'_, Vec<Vec<f32>>>;
}

/// A backend whose output is a pure function of its input.
///
/// It echoes a deterministic transform of the prompt rather than pretending to
/// be a language model. Tests that need particular text should assert on the
/// transform, not on plausible prose — a mock that imitates fluency invites
/// tests that pass for the wrong reason.
#[derive(Debug, Clone)]
pub struct MockLlm {
    /// Chunks the response into this many tokens, so streaming and cancellation
    /// have something to happen between.
    pub chunk_size: usize,
}

impl Default for MockLlm {
    fn default() -> Self {
        MockLlm { chunk_size: 8 }
    }
}

impl MockLlm {
    /// The transform, exposed so tests can state the expected output without
    /// duplicating the rule.
    pub fn response_for(prompt: &str) -> String {
        format!("[mock:{}] {}", &blake3::hash(prompt.as_bytes()).to_hex()[..8], prompt.trim())
    }
}

/// An embedder that maps text to a stable unit vector by hashing.
///
/// Nearby text does **not** produce nearby vectors, which is the point: a test
/// asserting that Weave proposes a link must set up the vectors it wants rather
/// than relying on a fake model to have opinions. Determinism is the feature;
/// semantics are not simulated.
#[derive(Debug, Clone)]
pub struct MockEmbedder {
    pub dims: usize,
}

impl Default for MockEmbedder {
    fn default() -> Self {
        // Matches nomic-embed-text, so index schema and vector maths are
        // exercised at the real width.
        MockEmbedder { dims: 768 }
    }
}

impl MockEmbedder {
    pub fn vector_for(text: &str, dims: usize) -> Vec<f32> {
        let seed = blake3::hash(text.as_bytes());
        let bytes = seed.as_bytes();
        let mut v: Vec<f32> = (0..dims)
            .map(|i| {
                let b = bytes[i % bytes.len()] as f32;
                let j = bytes[(i * 7 + 3) % bytes.len()] as f32;
                (b - 128.0) / 128.0 + (j - 128.0) / 4096.0
            })
            .collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_llm_is_reproducible() {
        // The property Phase 5's "same graph run twice" gate depends on.
        let a = MockLlm::response_for("summarise this note");
        let b = MockLlm::response_for("summarise this note");
        assert_eq!(a, b);
        assert_ne!(a, MockLlm::response_for("summarise that note"));
    }

    #[test]
    fn mock_embedder_is_reproducible_and_normalised() {
        let v = MockEmbedder::vector_for("hello", 768);
        assert_eq!(v.len(), 768);
        assert_eq!(v, MockEmbedder::vector_for("hello", 768));
        assert_ne!(v, MockEmbedder::vector_for("goodbye", 768));

        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "not a unit vector: {norm}");
    }

    #[test]
    fn mock_embedder_handles_degenerate_input() {
        let v = MockEmbedder::vector_for("", 768);
        assert_eq!(v.len(), 768);
        assert!(v.iter().all(|x| x.is_finite()));
    }
}
