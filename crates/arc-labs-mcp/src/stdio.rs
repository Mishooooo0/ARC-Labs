//! MCP over stdio: one JSON-RPC message per line.
//!
//! This is how Claude Desktop, ARC-SLASH and anything else that speaks MCP
//! reaches the vault on a desktop machine — the client spawns `arc-labs mcp`
//! and talks to its stdin.
//!
//! Nothing may be written to stdout except protocol messages. Logging goes to
//! stderr, always: a stray `println!` here corrupts the stream and produces a
//! failure that looks like the client's fault.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::Arc;

use arc_labs_api::Api;

/// Read requests from `input`, write responses to `output`, until EOF.
pub fn serve<R: Read, W: Write>(api: &Arc<Api>, input: R, mut output: W) -> std::io::Result<()> {
    let reader = BufReader::new(input);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = crate::handle(api, &line) {
            output.write_all(response.as_bytes())?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(())
}

/// Serve on the process's own stdin and stdout.
pub fn serve_stdio(api: &Arc<Api>) -> std::io::Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve(api, stdin.lock(), stdout.lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arc_labs_api::Capabilities;
    use arc_labs_core::Config;
    use serde_json::Value;

    #[test]
    fn a_session_over_a_pipe_initialises_lists_and_calls() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), b"# Alpha\n\nledgered provenance\n").unwrap();
        let api = Arc::new(Api::new(Config::default(), None, Capabilities::desktop()));
        api.open_vault(tmp.path()).unwrap();
        api.open_index(false).unwrap();

        // A real client's opening exchange, notification included.
        let input = concat!(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            "\n",
            "\n",
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            "\n",
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"note_read","arguments":{"path":"a.md"}}}"#,
            "\n"
        );

        let mut out = Vec::new();
        serve(&api, input.as_bytes(), &mut out).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();

        // Three requests, three responses — the notification and the blank line
        // produce nothing, which is the part clients actually break on.
        assert_eq!(lines.len(), 3, "got {lines:#?}");
        for (line, id) in lines.iter().zip([1, 2, 3]) {
            let v: Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["id"], id);
            assert!(v["error"].is_null(), "{line}");
        }
        assert!(lines[2].contains("Alpha"));
    }

    #[test]
    fn one_bad_line_does_not_end_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        let api = Arc::new(Api::new(Config::default(), None, Capabilities::desktop()));
        api.open_vault(tmp.path()).unwrap();

        let input = "not json\n{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}\n";
        let mut out = Vec::new();
        serve(&api, input.as_bytes(), &mut out).unwrap();

        let lines: Vec<&str> = std::str::from_utf8(&out).unwrap().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("error"));
        let v: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(v["id"], 7, "the session should survive a bad line");
    }
}
