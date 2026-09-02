//! The MCP bridge.
//!
//! Exposes the vault to other agents — E-Tron, ARC-SLASH, Claude Desktop —
//! through one surface, over stdio or HTTP.
//!
//! # There is no tool that writes
//!
//! `note_propose`, never `note_write`. That is not a naming convention, it is
//! the enforcement: an agent on the other end of this protocol **cannot** change
//! a file, because no operation exists that would let it. Its output lands as a
//! proposal in the ledger and waits for a person.
//!
//! The Phase 6 gate is a week of agent activity audited against git history
//! showing zero file changes without a matching accepted ledger entry. A server
//! that offered a write tool and relied on callers not to use it would fail that
//! the first time anyone was in a hurry.
//!
//! Every call is logged, and every mutation path goes through the proposal
//! system — which it does automatically, because these tools are thin wrappers
//! over `arc-labs-api`, the same layer the desktop and browser shells use.

use std::sync::Arc;

use arc_labs_api::Api;
use arc_labs_core::VaultPath;
use serde_json::{json, Value};

pub mod stdio;

/// JSON-RPC error codes, from the spec.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The protocol version this speaks.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Every tool this server offers.
///
/// Read the list: seven tools, and the only one that changes anything produces a
/// proposal. Someone auditing this file should be able to see the guarantee
/// without reading the implementation.
pub fn tool_definitions() -> Value {
    json!([
        {
            "name": "vault_search",
            "description": "Full-text search across every note in the vault. Returns paths, titles and the sentence around each match.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for. Whole words and prefixes." },
                    "limit": { "type": "integer", "description": "Maximum results (default 20)." }
                },
                "required": ["query"]
            }
        },
        {
            "name": "note_read",
            "description": "Read one note's markdown source, exactly as it is on disk.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Vault-relative path, e.g. Daily/2026-09-02.md" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "note_propose",
            "description":
                "Propose a change to a note. THE NOTE IS NOT MODIFIED. The proposal is recorded \
                 in the ledger, attributed to you, and a person accepts or rejects it. There is \
                 deliberately no tool that writes to a file directly.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":    { "type": "string", "description": "Vault-relative path of the note." },
                    "content": { "type": "string", "description": "The complete proposed content of the note." },
                    "reason":  { "type": "string", "description": "Why you are proposing this. Shown to the user." },
                    "agent":   { "type": "string", "description": "Your name, for attribution." },
                    "model":   { "type": "string", "description": "The model you are, for attribution." }
                },
                "required": ["path", "content", "reason"]
            }
        },
        {
            "name": "link_suggest",
            "description":
                "Notes that are semantically close but not linked to each other. These are \
                 INFERRED, not observed: each carries the score and the model that produced it.",
            "inputSchema": {
                "type": "object",
                "properties": { "limit": { "type": "integer" } }
            }
        },
        {
            "name": "canvas_read",
            "description": "Read a canvas: its cards, their positions, and the connections between them.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        },
        {
            "name": "canvas_run",
            "description":
                "Run a pipeline on a canvas. Any note output becomes a proposal, never a write. \
                 Returns a run id; poll canvas_run again with it to see progress.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path":   { "type": "string", "description": "The canvas." },
                    "node":   { "type": "string", "description": "The card to run, with everything it depends on." },
                    "run_id": { "type": "string", "description": "Poll an existing run instead of starting one." }
                }
            }
        },
        {
            "name": "ledger_read",
            "description":
                "The history of a note: who changed it, when, why, and whether each entry \
                 touched the file. Human entries and agent entries are distinguished.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } },
                "required": ["path"]
            }
        }
    ])
}

/// Handle one JSON-RPC request, returning the response.
///
/// `None` for a notification, which by the spec gets no reply.
pub fn handle(api: &Arc<Api>, request: &str) -> Option<String> {
    let parsed: Value = match serde_json::from_str(request) {
        Ok(v) => v,
        Err(e) => return Some(error_response(Value::Null, PARSE_ERROR, &e.to_string())),
    };

    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    let is_notification = parsed.get("id").is_none();
    let Some(method) = parsed.get("method").and_then(Value::as_str) else {
        return Some(error_response(id, INVALID_REQUEST, "no method"));
    };
    let params = parsed.get("params").cloned().unwrap_or(json!({}));

    tracing::info!(method, "mcp request");

    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "arc-labs", "version": env!("CARGO_PKG_VERSION") }
        })),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => call_tool(api, &params),
        // Notifications the client sends that need no answer.
        "notifications/initialized" | "initialized" => return None,
        "ping" => Ok(json!({})),
        _ => Err((METHOD_NOT_FOUND, format!("no method {method}"))),
    };

    if is_notification {
        return None;
    }
    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string(),
        Err((code, message)) => error_response(id, code, &message),
    })
}

type ToolResult = std::result::Result<Value, (i64, String)>;

fn call_tool(api: &Arc<Api>, params: &Value) -> ToolResult {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or((INVALID_PARAMS, "no tool name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));

    tracing::info!(tool = name, "mcp tool call");

    // Tool errors come back as a *result* with `isError`, not a protocol error:
    // "no such note" is something the calling agent should read and act on, not
    // a transport failure.
    let outcome = match name {
        "vault_search" => tool_search(api, &args),
        "note_read" => tool_note_read(api, &args),
        "note_propose" => tool_note_propose(api, &args),
        "link_suggest" => tool_link_suggest(api, &args),
        "canvas_read" => tool_canvas_read(api, &args),
        "canvas_run" => tool_canvas_run(api, &args),
        "ledger_read" => tool_ledger_read(api, &args),
        // Named explicitly so an agent that assumes a write tool exists gets an
        // answer that tells it what to do instead of a bare "unknown tool".
        "note_write" | "note_edit" | "note_save" | "note_create" | "note_delete" => {
            return Ok(tool_error(
                "ARC-LABS has no tool that writes to a file. Use note_propose: the change is \
                 recorded and attributed to you, and a person accepts or rejects it.",
            ))
        }
        other => return Err((INVALID_PARAMS, format!("no tool {other}"))),
    };

    Ok(match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(message) => tool_error(&message),
    })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

fn arg_str(args: &Value, key: &str) -> std::result::Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("{key} is required"))
}

fn arg_path(args: &Value, key: &str) -> std::result::Result<VaultPath, String> {
    // Through VaultPath, so an MCP client gets the same containment the HTTP and
    // Tauri shells do. This is the boundary that makes a hostile tool call
    // harmless.
    VaultPath::new(arg_str(args, key)?).map_err(|e| e.to_string())
}

fn arg_usize(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(default)
}

type TextResult = std::result::Result<String, String>;

fn tool_search(api: &Arc<Api>, args: &Value) -> TextResult {
    let q = arg_str(args, "query")?;
    let hits = api
        .search(&q, arg_usize(args, "limit", 20))
        .map_err(|e| e.message)?;
    if hits.is_empty() {
        return Ok(format!("No note matches {q:?}."));
    }
    Ok(hits
        .iter()
        .map(|h| {
            format!(
                "{}\n  {}\n  {}",
                h.title,
                h.path,
                h.snippet.replace('\n', " ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n"))
}

fn tool_note_read(api: &Arc<Api>, args: &Value) -> TextResult {
    let path = arg_path(args, "path")?;
    let note = api.read_note_for_edit(&path).map_err(|e| e.message)?;
    Ok(note.text.unwrap_or_default())
}

fn tool_note_propose(api: &Arc<Api>, args: &Value) -> TextResult {
    let path = arg_path(args, "path")?;
    let content = arg_str(args, "content")?;
    let reason = arg_str(args, "reason")?;
    let agent = args.get("agent").and_then(Value::as_str).unwrap_or("mcp");
    let model = args
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");

    let proposal = api
        .propose(&path, agent, model, "mcp", &reason, &content)
        .map_err(|e| e.message)?;
    Ok(format!(
        "Proposed a change to {path} (+{} −{}).\n\n\
         The note has NOT been modified. The proposal is entry {} in its ledger, attributed to \
         {agent} ({model}), and is waiting for a person to accept or reject it.",
        proposal.added, proposal.removed, proposal.index
    ))
}

fn tool_link_suggest(api: &Arc<Api>, args: &Value) -> TextResult {
    let items = api
        .suggestions(arg_usize(args, "limit", 20))
        .map_err(|e| e.message)?;
    if items.is_empty() {
        return Ok("No link suggestions. Weave may still be embedding.".into());
    }
    // Every line carries its score and model, because these are inferred and
    // must never read as observed.
    Ok(items
        .iter()
        .map(|s| {
            format!(
                "INFERRED  {} ↔ {}\n  score {:.3} · model {} · not an existing link",
                s.src_path, s.dst_path, s.score, s.model
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n"))
}

fn tool_canvas_read(api: &Arc<Api>, args: &Value) -> TextResult {
    let path = arg_path(args, "path")?;
    let canvas = api.read_canvas(&path).map_err(|e| e.message)?;
    serde_json::to_string_pretty(&canvas).map_err(|e| e.to_string())
}

fn tool_canvas_run(api: &Arc<Api>, args: &Value) -> TextResult {
    // Polling an existing run.
    if let Some(id) = args.get("run_id").and_then(Value::as_str) {
        let status = api.run_status(id).map_err(|e| e.message)?;
        return serde_json::to_string_pretty(&status).map_err(|e| e.to_string());
    }
    let path = arg_path(args, "path")?;
    let node = arg_str(args, "node")?;
    // Never auto-approved. An MCP client cannot consent on the user's behalf to
    // sending their vault off the machine.
    let id = api.start_run(&path, &node, false).map_err(|e| e.message)?;
    Ok(format!(
        "Started run {id} on {path} from node {node}.\n\
         Poll with canvas_run and run_id={id}. Any note output becomes a proposal, not a write."
    ))
}

fn tool_ledger_read(api: &Arc<Api>, args: &Value) -> TextResult {
    let path = arg_path(args, "path")?;
    let entries = api.timeline(&path).map_err(|e| e.message)?;
    if entries.is_empty() {
        return Ok(format!("{path} has no recorded history."));
    }
    Ok(entries
        .iter()
        .map(|e| {
            format!(
                "{:>3} {} {} {} {}{}  {}",
                e.index,
                e.ts,
                e.actor_kind,
                e.actor_id,
                e.op,
                if e.touched_file {
                    ""
                } else {
                    " (did not touch the file)"
                },
                e.reason
            )
        })
        .collect::<Vec<_>>()
        .join("\n"))
}

fn error_response(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_labs_api::Capabilities;
    use arc_labs_core::Config;

    fn api_with_vault() -> (tempfile::TempDir, Arc<Api>) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("a.md"),
            b"# Alpha\n\nThe ledger records provenance for every mutation.\n",
        )
        .unwrap();
        std::fs::write(tmp.path().join("b.md"), b"# Beta\n\nplain note\n").unwrap();

        let api = Arc::new(Api::new(Config::default(), None, Capabilities::desktop()));
        api.open_vault(tmp.path()).unwrap();
        api.open_index(false).unwrap();
        (tmp, api)
    }

    fn call(api: &Arc<Api>, tool: &str, args: Value) -> Value {
        let req = json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        })
        .to_string();
        serde_json::from_str(&handle(api, &req).expect("a request must get a response")).unwrap()
    }

    fn text_of(response: &Value) -> String {
        response["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn initialize_and_list_tools() {
        let (_t, api) = api_with_vault();
        let init: Value = serde_json::from_str(
            &handle(
                &api,
                &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}).to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(init["result"]["serverInfo"]["name"], "arc-labs");

        let list: Value = serde_json::from_str(
            &handle(
                &api,
                &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}).to_string(),
            )
            .unwrap(),
        )
        .unwrap();
        let names: Vec<&str> = list["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 7);
        assert!(names.contains(&"note_propose"));
    }

    /// **The Phase 6 gate, as a property of the interface.**
    #[test]
    fn no_tool_writes_to_a_file() {
        let list = tool_definitions();
        let names: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();

        for forbidden in [
            "note_write",
            "note_edit",
            "note_save",
            "note_create",
            "note_delete",
        ] {
            assert!(
                !names.contains(&forbidden),
                "a write tool exists: {forbidden}"
            );
        }
        // The only mutating tool proposes.
        assert!(names.contains(&"note_propose"));
    }

    #[test]
    fn asking_for_a_write_tool_explains_what_to_use_instead() {
        // An agent that assumes note_write exists should be told what to do, not
        // handed a bare "unknown tool".
        let (_t, api) = api_with_vault();
        let r = call(
            &api,
            "note_write",
            json!({ "path": "a.md", "content": "x" }),
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(text_of(&r).contains("note_propose"), "got {}", text_of(&r));
    }

    #[test]
    fn search_and_read_work() {
        let (_t, api) = api_with_vault();
        let r = call(&api, "vault_search", json!({ "query": "provenance" }));
        assert_eq!(r["result"]["isError"], false);
        assert!(text_of(&r).contains("a.md"));

        let r = call(&api, "note_read", json!({ "path": "a.md" }));
        assert!(text_of(&r).starts_with("# Alpha"));
    }

    /// **Constraint 4 through the bridge.**
    #[test]
    fn propose_records_a_proposal_and_does_not_touch_the_file() {
        let (tmp, api) = api_with_vault();
        let file = tmp.path().join("a.md");
        let before = std::fs::read(&file).unwrap();
        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));

        let r = call(
            &api,
            "note_propose",
            json!({
                "path": "a.md",
                "content": "# Alpha\n\nRewritten by an agent.\n",
                "reason": "tighten the wording",
                "agent": "e-tron",
                "model": "qwen3.5:0.8b"
            }),
        );
        assert_eq!(r["result"]["isError"], false);
        let text = text_of(&r);
        assert!(text.contains("has NOT been modified"), "got {text}");

        assert_eq!(std::fs::read(&file).unwrap(), before, "the file changed");
        assert_eq!(
            std::fs::metadata(&file).unwrap().modified().unwrap(),
            mtime,
            "mtime moved"
        );

        // And it is visible as a proposal, attributed to the caller.
        let p = api.proposals(&VaultPath::new("a.md").unwrap()).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].actor_id, "e-tron");
        assert_eq!(p[0].model.as_deref(), Some("qwen3.5:0.8b"));
    }

    #[test]
    fn a_traversal_in_a_tool_argument_is_rejected() {
        // The same containment the HTTP and Tauri shells get, because it is the
        // same VaultPath.
        let (_t, api) = api_with_vault();
        for bad in [
            "../../etc/passwd",
            "/etc/shadow",
            "C:\\Windows\\win.ini",
            "a.md:stream",
        ] {
            let r = call(&api, "note_read", json!({ "path": bad }));
            assert_eq!(r["result"]["isError"], true, "accepted {bad}");
        }
    }

    #[test]
    fn ledger_read_distinguishes_human_from_agent() {
        let (_t, api) = api_with_vault();
        let path = VaultPath::new("a.md").unwrap();
        let n = api.read_note_for_edit(&path).unwrap();
        api.write_note(&path, "# Alpha\n\nedited by a person\n", Some(&n.hash))
            .unwrap();
        api.propose(
            &path,
            "weave",
            "m",
            "s",
            "a suggestion",
            "# Alpha\n\nby an agent\n",
        )
        .unwrap();

        let text = text_of(&call(&api, "ledger_read", json!({ "path": "a.md" })));
        assert!(text.contains("human"), "got {text}");
        assert!(text.contains("agent"), "got {text}");
        assert!(
            text.contains("did not touch the file"),
            "a proposal must be marked: {text}"
        );
    }

    #[test]
    fn link_suggestions_are_labelled_as_inferred_with_a_score() {
        let (_t, api) = api_with_vault();
        let text = text_of(&call(&api, "link_suggest", json!({})));
        // Either there are none yet, or every line says it is inferred.
        assert!(
            text.contains("No link suggestions") || text.contains("INFERRED"),
            "an inferred edge must be labelled: {text}"
        );
    }

    #[test]
    fn a_notification_gets_no_reply() {
        let (_t, api) = api_with_vault();
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
        assert!(handle(&api, &req).is_none());
    }

    #[test]
    fn malformed_input_is_a_protocol_error_not_a_panic() {
        let (_t, api) = api_with_vault();
        for bad in ["not json", "{}", "[]", "{\"jsonrpc\":\"2.0\",\"id\":1}"] {
            let response = handle(&api, bad).expect("should still answer");
            let v: Value = serde_json::from_str(&response).unwrap();
            assert!(v["error"].is_object(), "expected an error for {bad:?}");
        }
    }

    #[test]
    fn an_unknown_method_is_reported_as_such() {
        let (_t, api) = api_with_vault();
        let req = json!({"jsonrpc":"2.0","id":9,"method":"does/not/exist"}).to_string();
        let v: Value = serde_json::from_str(&handle(&api, &req).unwrap()).unwrap();
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
    }
}
