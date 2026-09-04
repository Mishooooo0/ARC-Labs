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
| Windows desktop (Tauri / WebView2) | **builds, installs, runs** — MSI and NSIS produced |
| Headless server → browser | works, built and used |
| Linux headless (x86-64 ELF) | **builds and runs** — 397 tests pass on Ubuntu 24.04 |
| Linux desktop (webkit2gtk) | **still never built** — blocked on system packages |
| Docker image | **builds and runs** — healthy, vault open, token enforced |

The Linux headless binary was built and exercised on Ubuntu 24.04 under WSL:
handshake, `/healthz`, tree, note read, note **create**, the ledger entry that
create produced, the JSON 404 for an unknown endpoint, and the UI bundle served.
The whole suite runs there — **397 passed, 0 failed, the same count as on
Windows**, so the two platforms are not merely both green but running the same
set of tests.

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

- **Linux desktop artifacts do not exist.** `.AppImage` and `.deb` are unbuilt.
  The build stops before it reaches Tauri at all: `libdbus-sys` cannot find
  `pkg-config`, and `webkit2gtk-4.1` is missing behind it. Both are system
  packages, so this needs one `sudo apt install` on the build machine and
  nothing else — the Rust side already compiles there, as the headless binary
  proves.
- **The Docker image builds and runs.** It comes up healthy, opens a bind-mounted
  vault, enforces its token and serves the hub endpoints. Two things it revealed:
  the compose file mounted the vault read-only, five phases after that stopped
  being right, and `ARC_LABS_TOKEN` has to be pinned or a restart invalidates
  every device pointed at it.
- **The container has not been exercised over MCP.** The rest of the Phase 6
  gate is verified against the same binary outside a container; that half is
  still untested inside one.
- **Windows installers exist now.** `tauri build` produces
  `ARC-LABS_0.0.1_x64_en-US.msi` (7.6 MB) and `ARC-LABS_0.0.1_x64-setup.exe`
  (5.6 MB), and the built `arc-labs-app.exe` runs: it opens a vault, reports
  `shell: desktop` with the folder-picker capability, and serves its companion
  API to a browser on 127.0.0.1:7777.

  ```
  0600695f74083cf157297eed51114f2d3934db7844632feb8edbf07c0373e058  ARC-LABS_0.0.1_x64_en-US.msi
  873af97e4aa1c6926b3cc292d28fb5eb6f58538342fb9eef051ae6131ed71b88  ARC-LABS_0.0.1_x64-setup.exe
  ```

  Unsigned. Windows SmartScreen will warn on both until they are code-signed,
  which needs a certificate nobody has bought yet.
- **The NSIS installer has been run, and works.** Silent install
  (`ARC-LABS_0.0.1_x64-setup.exe /S`) lands in
  `%LOCALAPPDATA%\ARC-LABS`, creates a desktop shortcut and a Start Menu
  shortcut, registers an uninstall entry under Programs and Features, and the
  installed binary runs: vault open, index built, Weave enabled, companion API
  on 127.0.0.1:7777. It installs **per user** — `RequestExecutionLevel user` —
  so it never prompts for admin and touches nothing outside the profile.
- **It has not been installed on a *clean* machine.** That run was on the
  machine that built it, which has the whole toolchain. Phase 7's gate asks for
  a Windows box with no developer tools, and that is still untested — what is
  now known is that the installer itself is not broken.
- **Neither installer is code-signed.** SmartScreen will warn on both until
  someone buys a certificate.
- **Weave's default threshold (0.82) is conservative.** On a small real vault the
  strongest genuine pair scored 0.759, so the default suggests nothing. Lower it
  in `[weave]` if you want the inbox to have opinions.
- **`arc-labs mcp` has no create-note tool**, and neither does the API. Notes are
  created in the filesystem; the app reads them.

### Requires

Rust 1.88+, Node 20+, and — for the LLM and embedding features only — a local
Ollama with `qwen3.5:0.8b` and `nomic-embed-text`. Everything else runs offline
with no model present.
