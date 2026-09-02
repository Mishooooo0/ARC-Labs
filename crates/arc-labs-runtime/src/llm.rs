//! Talking to a model.
//!
//! # Why this speaks HTTP by hand
//!
//! Ollama's generate endpoint is one POST returning newline-delimited JSON.
//! Pulling in a full HTTP client for that would add a hundred crates to the
//! dependency tree of a program whose headline constraint is that it works with
//! the cable pulled — and every one of them is a thing to audit, because this is
//! the crate that touches the network.
//!
//! Writing it directly also buys the two things this phase actually needs:
//! **streaming**, so tokens appear as they arrive rather than after the whole
//! response, and **exact cancellation**, because the read loop checks a flag
//! between chunks and a cancelled run stops mid-stream rather than when a future
//! happens to be polled.
//!
//! ## Plain HTTP only, on purpose
//!
//! `https://` endpoints are **refused**, not silently downgraded. Constraint 3
//! says models are local; a remote endpoint is a documented, ledgered exception
//! for a machine on your own network. Implementing TLS badly here — or pulling
//! in a stack to do it properly — would be the wrong answer to a case the design
//! already discourages. Refusing says so out loud.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, thiserror::Error)]
pub enum LlmError {
    #[error("the model endpoint is not reachable at {0}")]
    Unreachable(String),
    #[error("https endpoints are not supported; ARC-LABS speaks plain HTTP to a local or LAN model")]
    HttpsNotSupported,
    #[error("{0} is not a usable endpoint")]
    BadEndpoint(String),
    #[error("the model returned an error: {0}")]
    Model(String),
    #[error("model {0} is not installed on that endpoint")]
    NoSuchModel(String),
    #[error("the run was cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, LlmError>;

#[derive(Debug, Clone, PartialEq)]
pub struct GenerateRequest {
    pub model: String,
    pub prompt: String,
    pub temperature: f32,
    /// Context window, in tokens.
    ///
    /// **Set explicitly, and deliberately small.** Ollama defaults to whatever
    /// the model advertises, and a recent small model can advertise 262,144 —
    /// which was measured on this hardware allocating 5.5 GB and pinning the CPU
    /// before producing a single token. On a machine with no GPU and roughly
    /// 0.24 GB/s of memory bandwidth, the context window costs far more than the
    /// weights do. A notebook summarising a few notes does not need a quarter of
    /// a million tokens of it.
    pub context_tokens: usize,
    /// Cap on generated tokens, so a runaway model cannot occupy the machine
    /// indefinitely.
    pub max_tokens: usize,
}

impl GenerateRequest {
    /// Sensible on hardware without a GPU. Both numbers are overridable per node.
    pub const DEFAULT_CONTEXT: usize = 4096;
    /// Generous enough that a reasoning model can think *and* answer. A model
    /// that spends its whole budget reasoning returns nothing, which is the
    /// most confusing possible outcome.
    pub const DEFAULT_MAX_TOKENS: usize = 2048;

    pub fn new(model: impl Into<String>, prompt: impl Into<String>) -> GenerateRequest {
        GenerateRequest {
            model: model.into(),
            prompt: prompt.into(),
            temperature: 0.0,
            context_tokens: Self::DEFAULT_CONTEXT,
            max_tokens: Self::DEFAULT_MAX_TOKENS,
        }
    }
}

/// What a completed generation cost.
///
/// The spec is explicit that tok/s is not optional on constrained hardware:
/// knowing a run is producing three tokens a second is the difference between
/// waiting and giving up.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RunCost {
    pub tokens: usize,
    pub elapsed_ms: u128,
    pub tokens_per_sec: f64,
    /// Peak resident memory of this process during the run, in bytes, where the
    /// platform will say.
    pub peak_rss_bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Generated {
    pub text: String,
    pub cost: RunCost,
    /// Why generation stopped, as the model reported it. `length` means it hit
    /// the token budget rather than finishing — worth surfacing, because on a
    /// reasoning model that usually means it never got past thinking.
    pub stop_reason: Option<String>,
    /// True when the model produced reasoning but no answer.
    ///
    /// Reasoning models (qwen3.5 among them) stream their working in a separate
    /// `thinking` field and only then start filling `response`. Run out of
    /// budget first and the answer is empty — which looks exactly like a
    /// successful run that produced nothing. Saying so is the difference
    /// between a confusing blank card and an obvious "give it more tokens".
    pub thought_but_did_not_answer: bool,
}

/// A cancellation flag, shared with whoever can press stop.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Cancel {
        Cancel::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Somewhere text can be generated.
pub trait Llm: Send + Sync {
    fn name(&self) -> &str;
    /// Generate, calling `on_token` for each chunk as it arrives.
    fn generate(
        &self,
        req: &GenerateRequest,
        cancel: &Cancel,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Generated>;
}

// ── Endpoints ───────────────────────────────────────────────────────────────

/// Host and port from an endpoint string.
pub fn parse_endpoint(endpoint: &str) -> Result<(String, u16)> {
    let e = endpoint.trim();
    if e.starts_with("https://") {
        return Err(LlmError::HttpsNotSupported);
    }
    let rest = e.strip_prefix("http://").unwrap_or(e).trim_end_matches('/');
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| LlmError::BadEndpoint(e.into()))?),
        None => (rest, 11434),
    };
    if host.is_empty() {
        return Err(LlmError::BadEndpoint(e.into()));
    }
    Ok((host.to_string(), port))
}

/// Whether an endpoint stays on this machine.
///
/// This is the question that decides whether a run is egress. Anything that is
/// not unambiguously loopback counts as leaving — including a hostname that
/// might resolve to loopback, because "might" is not a basis for deciding
/// whether someone's notes left their computer.
pub fn is_local(endpoint: &str) -> bool {
    let Ok((host, _)) = parse_endpoint(endpoint) else { return false };
    matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "[::1]" | "0.0.0.0")
}

// ── Ollama ──────────────────────────────────────────────────────────────────

pub struct Ollama {
    pub endpoint: String,
    pub connect_timeout: Duration,
}

impl Ollama {
    pub fn new(endpoint: impl Into<String>) -> Ollama {
        Ollama { endpoint: endpoint.into(), connect_timeout: Duration::from_secs(5) }
    }

    fn connect(&self) -> Result<TcpStream> {
        use std::net::ToSocketAddrs;
        let (host, port) = parse_endpoint(&self.endpoint)?;
        let addr = (host.as_str(), port)
            .to_socket_addrs()
            .map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?
            .next()
            .ok_or_else(|| LlmError::Unreachable(self.endpoint.clone()))?;

        let stream = TcpStream::connect_timeout(&addr, self.connect_timeout)
            .map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?;
        // A read timeout, not none: a model that stops sending should surface as
        // an error rather than hanging the run for ever.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(300)));
        Ok(stream)
    }

    /// Models installed on the endpoint. Used by `doctor` and by the run
    /// pre-flight, so a missing model is reported before a pipeline starts.
    pub fn models(&self) -> Result<Vec<String>> {
        let (host, port) = parse_endpoint(&self.endpoint)?;
        let mut stream = self.connect()?;
        let req = format!(
            "GET /api/tags HTTP/1.1\r\nHost: {host}:{port}\r\nAccept: application/json\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?;

        let mut body = String::new();
        let mut reader = BufReader::new(stream);
        skip_headers(&mut reader)?;
        let _ = reader.read_to_string(&mut body);

        let parsed: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| LlmError::Model(e.to_string()))?;
        Ok(parsed["models"]
            .as_array()
            .map(|a| {
                a.iter().filter_map(|m| m["name"].as_str().map(str::to_string)).collect()
            })
            .unwrap_or_default())
    }
}

impl Llm for Ollama {
    fn name(&self) -> &str {
        "ollama"
    }

    fn generate(
        &self,
        req: &GenerateRequest,
        cancel: &Cancel,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Generated> {
        let (host, port) = parse_endpoint(&self.endpoint)?;
        let started = Instant::now();

        let body = serde_json::json!({
            "model": req.model,
            "prompt": req.prompt,
            "stream": true,
            "options": {
                "temperature": req.temperature,
                // See GenerateRequest::context_tokens. Left unset, Ollama uses
                // the model's advertised maximum, which on this hardware is the
                // difference between a few seconds and never finishing.
                "num_ctx": req.context_tokens,
                "num_predict": req.max_tokens,
            },
        })
        .to_string();

        let mut stream = self.connect()?;
        let head = format!(
            "POST /api/generate HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(body.as_bytes()))
            .map_err(|_| LlmError::Unreachable(self.endpoint.clone()))?;

        let mut reader = BufReader::new(stream);
        let status = skip_headers(&mut reader)?;
        if status == 404 {
            return Err(LlmError::NoSuchModel(req.model.clone()));
        }

        let mut text = String::new();
        let mut thinking = String::new();
        let mut tokens = 0usize;
        let mut stop_reason: Option<String> = None;
        let mut line = String::new();

        loop {
            // Checked between chunks, so cancelling stops mid-stream rather
            // than after the model finishes. The Phase 5 gate depends on this.
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {}
                Err(_) => break,
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Chunked transfer encoding puts hex length lines between payloads.
            if !trimmed.starts_with('{') {
                continue;
            }
            let Ok(chunk) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                continue;
            };
            if let Some(err) = chunk["error"].as_str() {
                return Err(if err.contains("not found") {
                    LlmError::NoSuchModel(req.model.clone())
                } else {
                    LlmError::Model(err.to_string())
                });
            }
            if let Some(piece) = chunk["response"].as_str() {
                if !piece.is_empty() {
                    text.push_str(piece);
                    on_token(piece);
                }
            }
            // A reasoning model streams its working here before it answers.
            // Counted and remembered, but never treated as the answer — the
            // note gets the conclusion, not the deliberation.
            if let Some(piece) = chunk["thinking"].as_str() {
                thinking.push_str(piece);
            }
            if chunk["done"].as_bool().unwrap_or(false) {
                tokens = chunk["eval_count"].as_u64().unwrap_or(0) as usize;
                stop_reason = chunk["done_reason"].as_str().map(str::to_string);
                break;
            }
        }

        let thought_but_did_not_answer = text.trim().is_empty() && !thinking.trim().is_empty();
        if thought_but_did_not_answer {
            tracing::warn!(
                model = %req.model,
                budget = req.max_tokens,
                "the model spent its whole token budget reasoning and produced no answer"
            );
        }
        Ok(Generated {
            text,
            cost: cost_of(tokens, started),
            stop_reason,
            thought_but_did_not_answer,
        })
    }
}

/// Read past the status line and headers. Returns the status code.
fn skip_headers<R: BufRead>(reader: &mut R) -> Result<u16> {
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| LlmError::Model(e.to_string()))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut line = String::new();
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

fn cost_of(tokens: usize, started: Instant) -> RunCost {
    let elapsed = started.elapsed();
    let secs = elapsed.as_secs_f64();
    RunCost {
        tokens,
        elapsed_ms: elapsed.as_millis(),
        tokens_per_sec: if secs > 0.0 { tokens as f64 / secs } else { 0.0 },
        peak_rss_bytes: peak_rss(),
    }
}

/// Peak resident memory, where the platform will say.
///
/// The gate asks every run to record it, and on constrained hardware it is the
/// number that explains why a larger model made everything else slow.
pub fn peak_rss() -> u64 {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/self/status") {
            for line in s.lines() {
                if let Some(rest) = line.strip_prefix("VmHWM:") {
                    if let Some(kb) = rest.split_whitespace().next().and_then(|v| v.parse::<u64>().ok())
                    {
                        return kb * 1024;
                    }
                }
            }
        }
        0
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Windows would need GetProcessMemoryInfo through a binding. Reporting
        // 0 is honest; guessing would not be, and a wrong number in a
        // performance record is worse than a missing one.
        0
    }
}

// ── Mock ────────────────────────────────────────────────────────────────────

/// A deterministic backend.
///
/// Not a pretend language model. It echoes a stable transform of the prompt, so
/// a test asserting on output is asserting on a rule rather than on plausible
/// prose — a mock that imitated fluency would invite tests that pass for the
/// wrong reason. Determinism is what lets the whole runtime be tested with
/// Ollama switched off, which is how Phase 5's gates run in CI.
#[derive(Debug, Clone, Default)]
pub struct MockLlm {
    /// Chunks to split the response into, so streaming and cancellation have
    /// something to happen between.
    pub chunks: usize,
    /// Milliseconds to wait between chunks, so a test can cancel mid-stream.
    pub delay_ms: u64,
}

impl MockLlm {
    pub fn response_for(prompt: &str) -> String {
        format!("[mock:{}] {}", &blake3::hash(prompt.as_bytes()).to_hex()[..8], prompt.trim())
    }
}

impl Llm for MockLlm {
    fn name(&self) -> &str {
        "mock"
    }

    fn generate(
        &self,
        req: &GenerateRequest,
        cancel: &Cancel,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<Generated> {
        let started = Instant::now();
        let full = MockLlm::response_for(&req.prompt);
        let n = self.chunks.max(1);
        let step = full.len().div_ceil(n).max(1);

        let mut text = String::new();
        // Split on character boundaries, not bytes: a mock that panics on a
        // multi-byte prompt is a mock that cannot test a real vault.
        let chars: Vec<char> = full.chars().collect();
        for chunk in chars.chunks(step.max(1)) {
            if cancel.is_cancelled() {
                return Err(LlmError::Cancelled);
            }
            if self.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(self.delay_ms));
            }
            let piece: String = chunk.iter().collect();
            text.push_str(&piece);
            on_token(&piece);
        }
        let tokens = text.split_whitespace().count();
        Ok(Generated {
            text,
            cost: cost_of(tokens, started),
            stop_reason: Some("stop".into()),
            thought_but_did_not_answer: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_context_window_is_small_on_purpose() {
        // Ollama defaults to the model's advertised maximum. A small model
        // advertising 262,144 tokens was measured taking 5.5 GB and pinning the
        // CPU on this hardware before emitting a token, so the default here is
        // explicit and modest.
        let r = GenerateRequest::new("m", "p");
        assert_eq!(r.context_tokens, 4096);
        assert_eq!(r.max_tokens, 2048);
    }

    #[test]
    fn endpoint_parsing_handles_what_people_write() {
        assert_eq!(parse_endpoint("http://localhost:11434").unwrap(), ("localhost".into(), 11434));
        assert_eq!(parse_endpoint("localhost").unwrap(), ("localhost".into(), 11434));
        assert_eq!(parse_endpoint("http://127.0.0.1:1234/").unwrap(), ("127.0.0.1".into(), 1234));
        assert_eq!(
            parse_endpoint("http://workstation.local:11434").unwrap(),
            ("workstation.local".into(), 11434)
        );
    }

    #[test]
    fn https_is_refused_rather_than_silently_downgraded() {
        assert!(matches!(
            parse_endpoint("https://api.example.com"),
            Err(LlmError::HttpsNotSupported)
        ));
        // And an https endpoint is never treated as local.
        assert!(!is_local("https://localhost:11434"));
    }

    /// The check that decides whether a run counts as egress.
    #[test]
    fn only_unambiguous_loopback_counts_as_local() {
        for local in ["http://localhost:11434", "127.0.0.1:11434", "http://[::1]:11434"] {
            assert!(is_local(local), "{local} should be local");
        }
        for remote in [
            "http://workstation.local:11434",
            "http://192.168.1.50:11434",
            "http://api.example.com",
            // A hostname that *might* resolve to loopback still counts as
            // leaving: "might" is not a basis for deciding whether someone's
            // notes left their machine.
            "http://my-loopback-alias:11434",
        ] {
            assert!(!is_local(remote), "{remote} should not be local");
        }
    }

    #[test]
    fn the_mock_is_deterministic_and_streams() {
        let m = MockLlm { chunks: 5, delay_ms: 0 };
        let req = GenerateRequest::new("mock", "summarise this note");

        let mut pieces = Vec::new();
        let a = m.generate(&req, &Cancel::new(), &mut |t| pieces.push(t.to_string())).unwrap();
        assert!(pieces.len() > 1, "should stream in pieces");
        assert_eq!(pieces.concat(), a.text);

        let b = m.generate(&req, &Cancel::new(), &mut |_| {}).unwrap();
        assert_eq!(a.text, b.text, "the same prompt must give the same text");
        assert!(a.cost.tokens > 0);
    }

    #[test]
    fn cancelling_stops_the_stream_and_reports_it() {
        let m = MockLlm { chunks: 40, delay_ms: 2 };
        let cancel = Cancel::new();
        let req = GenerateRequest::new("mock", "a fairly long prompt so there are many chunks to stream");

        let mut seen = 0;
        let result = m.generate(&req, &cancel, &mut |_| {
            seen += 1;
            if seen == 3 {
                cancel.cancel();
            }
        });
        assert!(matches!(result, Err(LlmError::Cancelled)));
        assert!((3..40).contains(&seen), "should stop early, saw {seen} chunks");
    }

    #[test]
    fn the_mock_handles_multibyte_prompts() {
        let m = MockLlm { chunks: 4, delay_ms: 0 };
        let req = GenerateRequest::new("mock", "café ☕ 日本語 naïve");
        let out = m.generate(&req, &Cancel::new(), &mut |_| {}).unwrap();
        assert!(out.text.contains("café"));
    }

    #[test]
    fn an_unreachable_endpoint_fails_promptly_and_says_where() {
        let o = Ollama { endpoint: "http://127.0.0.1:1".into(), connect_timeout: Duration::from_millis(300) };
        let err = o
            .generate(
                &GenerateRequest::new("m", "p"),
                &Cancel::new(),
                &mut |_| {},
            )
            .unwrap_err();
        assert!(matches!(err, LlmError::Unreachable(_)));
    }
}
