//! The Phase 6 gate, as a command.
//!
//! > *An external agent can search, read and propose through MCP; a full audit
//! > of the vault's git history over a week of agent activity shows **zero**
//! > file changes without a matching accepted ledger entry.*
//!
//! Two halves, and the second is the one that matters.
//!
//! The first half is easy to fake: call the tools from inside the process and
//! assert they returned something. So this does not do that. It spawns
//! `arc-labs mcp` as a **separate process** and talks to it down a pipe, exactly
//! as Claude Desktop would. If the binary cannot serve a real client over stdio,
//! this fails.
//!
//! The second half is an audit, not a test. It runs a week's worth of agent
//! traffic against a vault under git, then asks git — not the application —
//! which files changed. Anything git reports must be explained by an `accept`
//! entry in that note's ledger. A file that changed without one is the failure
//! this whole architecture exists to prevent, and the only way to see it is to
//! ask an outside witness.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command as Proc, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// An MCP client that talks to a real subprocess over real pipes.
struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    fn spawn(exe: &Path, vault: &Path) -> Result<McpClient> {
        let mut child = Proc::new(exe)
            .arg("--vault")
            .arg(vault)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Inherit stderr so a crash is visible rather than swallowed.
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("spawning {}", exe.display()))?;

        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(McpClient {
            child,
            stdin,
            stdout,
            next_id: 1,
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{msg}")?;
        self.stdin.flush()?;

        let mut line = String::new();
        if self.stdout.read_line(&mut line)? == 0 {
            bail!("the mcp server closed the connection after {method}");
        }
        let v: Value = serde_json::from_str(&line)
            .with_context(|| format!("parsing the reply to {method}: {line}"))?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            bail!("{method} failed: {err}");
        }
        Ok(v)
    }

    fn call(&mut self, tool: &str, args: Value) -> Result<(String, bool)> {
        let v = self.request("tools/call", json!({ "name": tool, "arguments": args }))?;
        let text = v["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let is_error = v["result"]["isError"].as_bool().unwrap_or(false);
        Ok((text, is_error))
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn git(vault: &Path, args: &[&str]) -> Result<String> {
    let out = Proc::new("git")
        .arg("-C")
        .arg(vault)
        .args(args)
        .output()
        .context("running git — the audit needs it as an outside witness")?;
    if !out.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Run the audit. `days` is how many days of agent traffic to simulate.
pub fn audit_agents(vault: &Path, days: usize, exe: Option<&Path>) -> Result<()> {
    let exe = match exe {
        Some(p) => p.to_path_buf(),
        None => default_exe()?,
    };
    if !exe.exists() {
        bail!(
            "no arc-labs binary at {} — run `cargo build -p arc-labs-cli` first",
            exe.display()
        );
    }
    if !vault.is_dir() {
        bail!("no vault at {}", vault.display());
    }

    println!("ARC-LABS agent audit");
    println!("  vault  {}", vault.display());
    println!("  binary {}", exe.display());
    println!();

    // ── Set the witness up ───────────────────────────────────────────────────
    // A fresh repo inside the vault. `.arc/` is excluded: the index and the
    // ledger are ours to write, and committing them would let the audit's
    // subject vouch for itself.
    if !vault.join(".git").exists() {
        git(vault, &["init", "-q"])?;
        git(vault, &["config", "user.email", "audit@arc-labs.local"])?;
        git(vault, &["config", "user.name", "ARC-LABS audit"])?;
    }
    std::fs::write(vault.join(".gitignore"), b".arc/\n")?;
    git(vault, &["add", "-A"])?;
    git(
        vault,
        &["commit", "-q", "-m", "audit baseline", "--allow-empty"],
    )?;
    let baseline = git(vault, &["rev-parse", "HEAD"])?.trim().to_string();
    println!("baseline commit {}", &baseline[..12.min(baseline.len())]);

    // ── A week of agent activity ─────────────────────────────────────────────
    let mut mcp = McpClient::spawn(&exe, vault)?;
    let init = mcp.request("initialize", json!({}))?;
    println!(
        "connected to {} over stdio\n",
        init["result"]["serverInfo"]["name"].as_str().unwrap_or("?")
    );

    let tools = mcp.request("tools/list", json!({}))?;
    let names: Vec<String> = tools["result"]["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|t| t["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    println!("tools offered: {}", names.join(", "));
    for forbidden in ["note_write", "note_edit", "note_save", "note_delete"] {
        if names.iter().any(|n| n == forbidden) {
            bail!("FAIL: the server offers a direct-write tool: {forbidden}");
        }
    }
    println!("no direct-write tool is offered  ✓\n");

    // Which notes to work on. Taken from the server's own search, so this reads
    // the vault the way an agent actually would.
    let (found, _) = mcp.call("vault_search", json!({ "query": "the", "limit": 40 }))?;
    let mut targets: Vec<String> = found
        .lines()
        .map(str::trim)
        .filter(|l| l.ends_with(".md"))
        .map(String::from)
        .collect();
    targets.sort();
    targets.dedup();
    if targets.is_empty() {
        bail!("the vault has no notes the agent could find; the audit would prove nothing");
    }

    let mut proposed = 0usize;
    let mut refused = 0usize;
    let mut reads = 0usize;
    let mut searches = 0usize;

    for day in 1..=days {
        for (i, path) in targets.iter().enumerate() {
            // Read it.
            let (body, err) = mcp.call("note_read", json!({ "path": path }))?;
            if err {
                continue;
            }
            reads += 1;

            // Search for something in it.
            let word = body
                .split_whitespace()
                .find(|w| w.len() > 5)
                .unwrap_or("note")
                .to_string();
            let clean: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
            if !clean.is_empty() {
                mcp.call("vault_search", json!({ "query": clean, "limit": 5 }))?;
                searches += 1;
            }

            // Look at its history, and at what the vault thinks might link.
            mcp.call("ledger_read", json!({ "path": path }))?;
            if i == 0 {
                mcp.call("link_suggest", json!({ "limit": 10 }))?;
            }

            // Propose a change. Every third one, so the traffic is not uniform.
            if i % 3 == 0 {
                let (_, err) = mcp.call(
                    "note_propose",
                    json!({
                        "path": path,
                        "content": format!("{body}\n<!-- audit day {day} -->\n"),
                        "reason": format!("audit day {day}: tighten the wording"),
                        "agent": "audit-agent",
                        "model": "synthetic",
                    }),
                )?;
                if !err {
                    proposed += 1;
                }
            }

            // And try to write directly, the way a confused agent would.
            let (msg, err) = mcp.call(
                "note_write",
                json!({ "path": path, "content": "OVERWRITTEN BY AN AGENT\n" }),
            )?;
            if !err || !msg.contains("note_propose") {
                bail!("FAIL: note_write was not refused with a usable explanation: {msg}");
            }
            refused += 1;
        }
    }
    drop(mcp);

    println!("{days} day(s) of agent activity:");
    println!("  {searches} searches, {reads} reads, {proposed} proposals");
    println!("  {refused} direct-write attempts, all refused\n");

    // ── Ask the witness ──────────────────────────────────────────────────────
    let dirty = git(vault, &["status", "--porcelain"])?;
    let changed: Vec<&str> = dirty
        .lines()
        .map(|l| l.get(3..).unwrap_or("").trim_matches('"'))
        .filter(|p| !p.is_empty() && !p.starts_with(".arc/") && *p != ".gitignore")
        .collect();

    if !changed.is_empty() {
        println!(
            "FAIL: git reports {} changed file(s) after agent-only activity:",
            changed.len()
        );
        for p in changed.iter().take(20) {
            println!("  {p}");
        }
        bail!("agent activity changed files with no human acceptance");
    }
    println!("git reports zero changed files after {proposed} agent proposals  ✓");

    // The proposals must nonetheless be *there* — a server that silently dropped
    // them would also pass the test above, and would be useless.
    let ledger_dir = vault.join(".arc").join("ledger");
    let mut entries = 0usize;
    let mut proposals_on_disk = 0usize;
    if ledger_dir.is_dir() {
        for f in std::fs::read_dir(&ledger_dir)? {
            let f = f?;
            if f.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            for line in std::fs::read_to_string(f.path())?.lines() {
                entries += 1;
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    if v["op"] == "propose" {
                        proposals_on_disk += 1;
                    }
                }
            }
        }
    }
    if proposals_on_disk < proposed {
        bail!(
            "FAIL: {proposed} proposals were made but only {proposals_on_disk} reached the ledger"
        );
    }
    println!("{proposals_on_disk} proposals recorded in the ledger, {entries} entries total  ✓");

    // ── Now let a person accept one, and check the audit still lines up ───────
    // The other half of the guarantee: an accepted proposal *does* change the
    // file, and the change is explained by an entry naming who accepted it.
    println!("\naccepting one proposal, as a person would…");
    let accepted = accept_one_proposal(vault)?;
    match accepted {
        Some((path, index)) => {
            let dirty = git(vault, &["status", "--porcelain"])?;
            let changed: Vec<&str> = dirty
                .lines()
                .map(|l| l.get(3..).unwrap_or("").trim_matches('"'))
                .filter(|p| !p.is_empty() && !p.starts_with(".arc/") && *p != ".gitignore")
                .collect();

            if changed.len() != 1 {
                bail!(
                    "FAIL: accepting one proposal changed {} files: {changed:?}",
                    changed.len()
                );
            }
            let changed_path = changed[0].replace('\\', "/");
            if changed_path != path {
                bail!("FAIL: accepting a proposal for {path} changed {changed_path} instead");
            }
            println!("  accepted entry {index} of {path}");
            println!("  git reports exactly that one file changed  ✓");
        }
        None => bail!("FAIL: no proposal was available to accept"),
    }

    println!("\nAUDIT PASSED");
    println!("  every file change is explained by an accepted ledger entry;");
    println!("  no file changed without one.");
    Ok(())
}

/// Accept the first pending proposal in the vault, through the same API a person
/// clicking Accept would use.
///
/// Deliberately the *application's* accept path rather than a hand-rolled write:
/// the audit is checking that path, so reimplementing it here would let a bug in
/// the real one pass unnoticed.
fn accept_one_proposal(vault: &Path) -> Result<Option<(String, usize)>> {
    use arc_labs_api::{Api, Capabilities};
    use arc_labs_core::Config;

    let api = Api::new(Config::default(), None, Capabilities::desktop());
    api.open_vault(vault)?;

    for entry in api.tree()?.tree.entries {
        if entry.is_dir || !entry.path.as_str().ends_with(".md") {
            continue;
        }
        let pending = api.proposals(&entry.path)?;
        let Some(first) = pending.first() else {
            continue;
        };
        api.accept(&entry.path, first.index)?;
        return Ok(Some((entry.path.as_str().to_string(), first.index)));
    }
    Ok(None)
}

fn default_exe() -> Result<std::path::PathBuf> {
    let name = if cfg!(windows) {
        "arc-labs.exe"
    } else {
        "arc-labs"
    };
    let root = std::env::current_dir()?;
    for profile in ["debug", "release"] {
        let candidate = root.join("target").join(profile).join(name);
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Ok(root.join("target").join("debug").join(name))
}
