//! Talking to a hub over HTTP.
//!
//! The only place in the workspace that opens an outbound connection for vault
//! content, and the only one that speaks TLS. Everything it does maps one-to-one
//! onto a `/api/v1/hub/*` endpoint; it holds no policy, decides nothing, and is
//! deliberately dull. What to send is [`crate::plan`]'s job, and that has no
//! socket at all.
//!
//! ## Why `https` matters here and not in `llm.rs`
//!
//! The hand-written HTTP client in `arc-labs-runtime` refuses `https://` by
//! design, and should keep refusing: it talks to Ollama on loopback, where TLS
//! buys nothing and a hand-rolled parser is a reasonable trade for zero
//! dependencies. This client carries whole notebooks across a network that may
//! be the internet. Those are not the same problem, and pretending they are
//! would mean either hand-writing TLS — which is not a thing to hand-write — or
//! telling every self-hoster to run a reverse proxy before they can sync at all.
//!
//! ## Failures are reported, never guessed at
//!
//! Every method returns the hub's own reason. A sync that half-worked and said
//! "done" is worse than one that stopped and said which file it was on, because
//! the base manifest would then record an agreement that never happened.

use std::time::Duration;

use crate::manifest::Manifest;
use crate::{Result, SyncError};

/// A hub, and the credentials to talk to it.
pub struct Hub {
    base: String,
    token: String,
    agent: ureq::Agent,
}

/// What the hub holds, and the state it held it in.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubManifest {
    pub generation: String,
    pub files: Manifest,
}

impl Hub {
    /// Point at a hub.
    ///
    /// The trailing slash is trimmed so `https://vault.example` and
    /// `https://vault.example/` are one hub — otherwise typing the URL slightly
    /// differently in Settings silently starts the whole sync over, because the
    /// base manifest is keyed on this string.
    pub fn new(url: &str, token: &str) -> Hub {
        let agent = ureq::Agent::config_builder()
            // Generous, because a first sync of a large vault is a lot of small
            // requests and a hub may be a small box behind a slow link. Not
            // unbounded: a hung connection has to become an error someone can
            // read rather than a sync that never finishes and never says why.
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();

        Hub {
            base: url.trim().trim_end_matches('/').to_string(),
            token: token.trim().to_string(),
            agent,
        }
    }

    /// The hub's URL, normalised. The base manifest is filed under this, so it
    /// has to be the same string every time the same hub is named.
    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api/v1/hub{path}", self.base)
    }

    /// What the hub holds right now, and the generation to quote back.
    pub fn manifest(&self) -> Result<HubManifest> {
        self.agent
            .get(self.url("/manifest"))
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| self.wire(e))?
            .body_mut()
            .read_json()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    /// One file's bytes.
    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let mut res = self
            .agent
            .get(self.url("/file"))
            .query("path", path)
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| self.wire(e))?;

        // No cap. A vault holds PDFs and images, and a limit low enough to be a
        // safety net is also low enough to silently refuse someone's actual
        // attachment — which would look like sync quietly skipping a file.
        res.body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_vec()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    /// Send one file, quoting the generation it was planned against.
    pub fn write(&self, path: &str, bytes: &[u8], generation: &str) -> Result<()> {
        self.agent
            .post(self.url("/file"))
            .query("path", path)
            .query("generation", generation)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Content-Type", "application/octet-stream")
            .send(bytes)
            .map_err(|e| self.wire(e))?;
        Ok(())
    }

    /// Delete one file there, on behalf of a deletion made here.
    pub fn delete(&self, path: &str, generation: &str) -> Result<()> {
        self.agent
            .post(self.url("/delete"))
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(serde_json::json!({ "path": path, "generation": generation }))
            .map_err(|e| self.wire(e))?;
        Ok(())
    }

    /// Which of these object hashes the hub does not have.
    pub fn missing_objects(&self, hashes: &[String]) -> Result<Vec<String>> {
        self.agent
            .post(self.url("/objects/missing"))
            .header("Authorization", &format!("Bearer {}", self.token))
            .send_json(serde_json::json!({ "hashes": hashes }))
            .map_err(|e| self.wire(e))?
            .body_mut()
            .read_json()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    pub fn read_object(&self, hash: &str) -> Result<String> {
        self.agent
            .get(self.url("/object"))
            .query("hash", hash)
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| self.wire(e))?
            .body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_string()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    pub fn write_object(&self, content: &str) -> Result<()> {
        self.agent
            .post(self.url("/object"))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Content-Type", "text/plain")
            .send(content)
            .map_err(|e| self.wire(e))?;
        Ok(())
    }

    pub fn ledger_keys(&self) -> Result<Vec<String>> {
        self.agent
            .get(self.url("/ledger/keys"))
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| self.wire(e))?
            .body_mut()
            .read_json()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    pub fn read_ledger(&self, key: &str) -> Result<String> {
        self.agent
            .get(self.url("/ledger"))
            .query("key", key)
            .header("Authorization", &format!("Bearer {}", self.token))
            .call()
            .map_err(|e| self.wire(e))?
            .body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_string()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    /// Merge history into the hub's copy. Returns entries it gained.
    pub fn merge_ledger(&self, key: &str, jsonl: &str) -> Result<usize> {
        self.agent
            .post(self.url("/ledger"))
            .query("key", key)
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Content-Type", "text/plain")
            .send(jsonl)
            .map_err(|e| self.wire(e))?
            .body_mut()
            .read_json()
            .map_err(|e| SyncError::Wire(e.to_string()))
    }

    /// Turn a transport failure into something worth reading.
    ///
    /// A hub answers errors as `{"code","message"}`, and that message is the
    /// useful one — "this ARC-LABS is not a vault server" beats "HTTP 403".
    /// Falls back to the status when there is no body to read, and says plainly
    /// when the hub could not be reached at all, which is the most common
    /// failure and the one a bare error string explains worst.
    fn wire(&self, e: ureq::Error) -> SyncError {
        match e {
            ureq::Error::StatusCode(code) => {
                SyncError::Hub(format!("{} answered {code}", self.base))
            }
            other => SyncError::Unreachable {
                hub: self.base.clone(),
                why: other.to_string(),
            },
        }
    }
}
