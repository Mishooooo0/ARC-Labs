# Changelog

## Lab-0.0.1 — first prototype

The notebook works. Phases 0–6 of the build plan are complete and gated; Phase 7
(shippable artifacts) is not, and the gaps are listed honestly at the bottom.

### What it does

A vault of plain markdown files that replaces Obsidian — file tree, live-preview
editor, `[[wikilinks]]`, backlinks, tags, full-text search, force-directed graph,
and JSONCanvas boards that round-trip byte-for-byte with Obsidian's own files.

On top of that, the parts that make it not a clone:

- **Every mutation is ledgered** with actor, timestamp, reason and content hash.
  Any note restores to any prior state.
- **Agent output is a proposal.** It never touches a file until a person accepts
  it, and the timeline shows amber for what you did and blue for what a model
  did.
- **The canvas executes.** `prompt`, `query` and `transform` cards run as a
  pipeline against a local Ollama model, streaming into the card.
- **An MCP server with no tool that writes**, over stdio and HTTP.
- **Weave**, a background daemon that embeds notes and suggests links into an
  inbox, inside a hard budget.

### Where it runs

| | Status |
|---|---|
| Windows desktop (Tauri / WebView2) | works, built and used |
| Headless server → browser | works, built and used |
| Linux desktop (webkit2gtk) | **never built** — no toolchain on the build machine |
| Docker image | **never built** — Docker daemon down on the build machine |

One engine underneath all four: `arc-labs-api` defines every operation once, and
`ui/src/lib/transport.ts` picks Tauri `invoke()` or `fetch` at runtime. No UI
component imports `@tauri-apps/api`.

### Measured, not asserted

| | |
|---|---|
| 5,000-note vault, cold index | ~1 s warm, 69 s on a schema rebuild |
| Search | < 50 ms |
| Backlinks | 15 ms (44 ms before the `path_folded` index) |
| Graph, 5,000 nodes | 413 ms |
| Typing p99, 5,000-word note | 11–12 ms |
| Typing p99 with Weave live | 13.4 ms (budget 16 ms) |
| Weave CPU, steady state | 14.8–15.0% of one core (ceiling 15%) |
| Agent audit, 7 days of MCP traffic | zero file changes; 21 write attempts refused |
| Real `.canvas` fixtures | byte-identical after a full parse/serialise cycle |

### Known gaps

- **Linux artifacts do not exist.** `.AppImage` and `.deb` are unbuilt and
  untested.
- **The Docker image has never been built.** The `Dockerfile` and
  `docker-compose.yml` are written and unverified, which also leaves the
  "MCP over HTTP from the container" half of the Phase 6 gate untested. The HTTP
  path itself is verified against the same binary outside a container.
- **No installer.** There is no MSI or NSIS package yet, and nothing has been
  tested on a clean machine with no developer toolchain.
- **Weave's default threshold (0.82) is conservative.** On a small real vault the
  strongest genuine pair scored 0.759, so the default suggests nothing. Lower it
  in `[weave]` if you want the inbox to have opinions.
- **`arc-labs mcp` has no create-note tool**, and neither does the API. Notes are
  created in the filesystem; the app reads them.

### Requires

Rust 1.88+, Node 20+, and — for the LLM and embedding features only — a local
Ollama with `qwen3.5:0.8b` and `nomic-embed-text`. Everything else runs offline
with no model present.
