# ARC-LABS

> ARC Collection / Obsidian Labs
> A lab notebook your agents can hold.

**It is a notebook.** A folder of plain markdown files with `[[wikilinks]]`,
tags, backlinks, a graph and a canvas. It opens an existing Obsidian vault
unchanged, and any vault it touches still opens in Obsidian afterwards. Nothing
it stores is in a format you need this program to read.

**It also holds agents to account.** It can call a local LLM to work on the
vault and embed it, and it ships an MCP server so other agents reach your notes
through one surface. But an agent never writes to a file. It writes a
*proposal*, attached to the note, that you accept or reject — and every change,
human or agent, appends to an immutable ledger with an actor, a timestamp, a
reason and a content hash.

That is the whole pitch: the notebook is the product, the provenance is why you
would choose it, and neither costs you the ability to walk away with your files.

**Status:** Phase 0 of 7. The vault opens, the tree renders, notes render, in all
four run modes. Editing arrives in Phase 1. See [Build phases](#build-phases).

---

## Run it anywhere

One engine, four shells. Pick the row that matches where you are.

| | Command | Needs |
|---|---|---|
| **Windows desktop** | `arc-labs-app` | WebView2 (ships with Windows 11) |
| **Linux desktop** | `arc-labs-app` | `webkit2gtk-4.1` — run `arc-labs setup` |
| **Linux headless** | `arc-labs serve --vault ~/notes` | a browser, somewhere |
| **Docker** | `docker run --user "$(id -u):$(id -g)" -v ~/notes:/vault -p 7777:7777 arc-labs` | nothing |

The `--user` is not optional on Linux. The image runs as uid 10001, a
bind-mounted vault belongs to whoever made it, and a mismatch means the
container cannot write the index or the ledger — the app comes up looking
perfectly normal and silently cannot save. It now says **READ-ONLY** next to the
vault name when that happens, and the log tells you this line, but running as
yourself avoids it entirely. Docker Desktop on Windows and macOS papers over the
ownership difference, so this only bites on a real Linux host.

Bound to `0.0.0.0`, the container generates a bearer token and prints it once —
read it from `docker logs`. `GET /healthz` is the one unauthenticated route, so
the container's own healthcheck can reach it; it answers `ok` and says nothing
about the vault.

Two more, once a vault is open:

| | Command |
|---|---|
| **Serve MCP to another agent** | `arc-labs mcp` (stdio) |
| **Suggest links** | `arc-labs weave --once` |

Not sure what this machine can do:

```bash
arc-labs doctor
```

It prints what it found, what is missing, and the exact command that fixes each
missing thing. It never installs anything.

---

## Why it is shaped this way

Supporting a desktop window *and* a browser is the decision that shaped
everything else. Doing it badly means two front-ends that drift apart until a
feature works in one and is broken in the other. So there is one of everything
that matters:

```
                       ┌──────────────────────────────┐
                       │  ui/   Svelte 5 + TypeScript │
                       │  written ONCE                │
                       │  lib/transport.ts  ──────────┼── picks at runtime:
                       └──────────────┬───────────────┘   Tauri invoke() when
                                      │                   running in a window,
   ┌────────────────┬─────────────────┼──────────────┬──  else fetch + WebSocket
   │                │                 │              │
┌──▼───────────┐ ┌──▼────────────┐ ┌──▼───────────┐ ┌▼─────────────┐
│ Tauri /      │ │ Tauri /       │ │ arc-labs-    │ │ Docker image │
│ Windows      │ │ Linux GTK     │ │ server (Axum)│ │ = server+ui  │
│ WebView2     │ │ webkit2gtk    │ │ any browser  │ │ vault: mount │
└──────┬───────┘ └──────┬────────┘ └──────┬───────┘ └──────┬───────┘
       └────────────────┴─────────┬───────┴────────────────┘
                                  │
                    ┌─────────────▼──────────────┐
                    │  arc-labs-api              │  every operation,
                    │  transport-agnostic        │  defined exactly once
                    └─────────────┬──────────────┘
                                  │
        core ← index ← ledger ← canvas ← runtime ← { mcp, weave }
                      strict one-way fan-out
```

**`arc-labs-api` defines every operation exactly once.** A Tauri command is a
ten-line wrapper over it. An HTTP handler is a ten-line wrapper over it. An MCP
tool will be a ten-line wrapper over it. An operation added once appears in all
of them — and an operation that forgets to go through the ledger cannot exist in
one shell and not another, because there is only one place it could be written.

**No component imports `@tauri-apps/api`.** Everything shell-specific lives in
`ui/src/lib/transport.ts`. The moment a component reaches past it, the browser
and Docker builds break at runtime, in the environment nobody is testing in.

**`arc-labs-core` cannot open a socket.** Not by policy — nothing in its
dependency tree can. "Fully offline" is a property of the dependency graph
rather than a promise in a document.

---

## The constraints, and what enforces each

A constraint that is only an intention gets traded away under deadline. Each of
these has a mechanism.

| Constraint | Mechanism |
|---|---|
| **Files are the source of truth** | The SQLite index (Phase 2) is derived. Delete `.arc/index.db`, reopen, lose nothing — and that is a gate, not a claim. |
| **Obsidian compatibility both ways** | `fixtures/vaults/sample` is the oracle — Obsidian's exact output shape with invented content. `.canvas` files round-trip byte-for-byte, including Obsidian's unstable per-node key order. |
| **Fully offline** | `arc-labs-core` has no network-capable dependency. Both shells set a CSP with `connect-src 'self'`, so the webview *refuses* rather than the code promising. |
| **No silent agent writes** | Agent output is a proposal. The MCP server has no `note_write` tool — the operation does not exist. Audited against git by `cargo run -p xtask -- audit-agents`. |
| **Immutable ledger** | Append-only JSONL per note; any note restores to any prior state. |
| **Provenance visible without reading** | Amber is human, blue is agent, everywhere. `cargo xtask lint-tokens` fails the build if either colour is spent on decoration. |
| **Never fabricate relationships** | Inferred links live in a different table, arrive over a different API type, and are drawn in a different register — dashed, dimmer, scored, and naming the model. Where resolution is unknown, the API returns `null` rather than a guess. |

Two more that fall out of having a server shell:

- **Loopback by default.** Binding anywhere else requires *both* `--host` and a
  generated token. There is no "no auth on 0.0.0.0" configuration to fall into.
- **Off-loopback means less, not more.** A remote client cannot browse the
  filesystem and is never told where the vault lives on the host.

---

## Bootstrap: `doctor` and `setup`

`doctor` reads. `setup` installs. They are separate commands because they are
different kinds of act.

```bash
arc-labs doctor          # what this machine has, what it is missing, how to fix it
arc-labs setup           # install the missing system packages — asks first
```

`setup` detects the platform and its package manager — `apt`, `dnf`, `pacman`,
`zypper`, `apk` on Linux, `winget` on Windows — then **prints the literal
commands it would run and waits for a yes.** `--yes` exists for Docker builds
and CI; it is never the interactive default.

### About that, and "fully offline"

`setup` makes network calls. That does not contradict the offline constraint,
for four reasons that together are the design:

1. It is the **only** outbound path in the product. All installer code lives in
   `arc-labs-bootstrap`, and `arc-labs-core` cannot depend on it.
2. It **never runs on its own** — not at startup, not on first run, not as a
   side effect of opening a vault. Only when typed.
3. It **says what it will do first**, and waits.
4. It **touches no vault content.** It installs system packages. Nothing you
   have written goes anywhere.

---

## Obsidian compatibility

| | Status |
|---|---|
| Markdown, `[[wikilinks]]`, `[[link\|alias]]`, `[[link#heading]]`, `![[embeds]]`, `#tags` | Read and rendered |
| YAML frontmatter | Split and preserved **as bytes** — never parsed and re-serialised, so key order, quoting and comments survive |
| Line endings, BOM, trailing newline | Detected per file, restored on write |
| `.canvas` (JSONCanvas) | Round-trips byte-for-byte (Phase 4) |
| ARC's `prompt` / `query` / `transform` nodes | Serialised as `type: "text"` with an `arc` key, so Obsidian degrades them to plain cards rather than erroring |
| Raw HTML in a note | **Escaped, not executed** — a deliberate difference. ARC-LABS serves vaults to browsers; a note containing `<script>` must not run in one. Your text stays visible as literal characters. |

---

## Other agents, and the daemon

Two things a notebook does not usually have. Both are opt-in, and both are built
so that the vault cannot be changed behind your back.

### The MCP server — there is no tool that writes

```bash
arc-labs mcp                 # stdio, for Claude Desktop and friends
arc-labs serve               # the same tools at POST /api/mcp
```

Seven tools: `vault_search`, `note_read`, `note_propose`, `link_suggest`,
`canvas_read`, `canvas_run`, `ledger_read`.

There is **no `note_write`**. Not "there is one but please don't", not "there is
one behind a flag" — the operation does not exist, so an agent on the other end
of the protocol cannot change a file no matter what it decides to do. Its output
lands as a proposal in the ledger and waits for a person. Asking for
`note_write` anyway returns an explanation of what to use instead, because an
agent that assumed the tool exists deserves a better answer than "unknown tool".

Both transports call the same `handle()` over the same `arc-labs-api`, so the
Docker container serves agents exactly what the desktop app does. There is no
second implementation to drift.

To check the guarantee rather than believe it:

```bash
cargo run -p xtask -- audit-agents --vault /path/to/a/copy --days 7
```

That spawns `arc-labs mcp` as a real subprocess, runs a week of agent traffic
through a pipe, and then asks **git** — not the application — which files
changed. Zero, out of hundreds of proposals. Then it accepts one as a person
would, and git reports exactly that one file.

### Weave — link suggestions, inside a budget

Off by default. Embedding a whole vault is real work on your machine, and that
is a thing to opt into rather than discover:

```toml
[weave]
enabled = true
threshold = 0.82      # cosine similarity; deliberately high
cpu_fraction = 0.15   # a ceiling, and it cannot be raised
interval_secs = 60
```

It embeds changed notes with `nomic-embed-text`, then proposes links between
notes that are semantically close and *structurally unlinked*. Suggestions go to
an inbox pane. They never go into a file, and they never go into the `links`
table — they live in `suggested_links`, and no query joins the two. Constraint 7
is a schema layout here, not a coding convention.

**The budget is not a preference.** Phase 1's typing target outranks Weave
completely, so:

- **≤15% of one core**, averaged over a rolling 60-second window. `cpu_fraction`
  may only make it stricter — a config file cannot raise it.
- **Nothing within 2 seconds of a keystroke.** A pass mid-flight stops, and says
  so on the inbox card.
- **Hard stop** when the index write queue is backed up.
- **Resumable.** Killing the app costs you at most the batch in flight.

Three things that had to be got right for those to hold, each of which was wrong
first:

1. **A pass never sleeps while holding the index.** Weave takes the index lock
   per SQL statement, never across the HTTP call to the embedding model. Holding
   it for a whole pass meant every save queued behind a network round trip — and
   on the server, saves run on the async runtime's workers, so enough of them
   blocked the *entire process*. The duty cycle, whose only job is protecting the
   editor, was the thing stalling it.
2. **The budget refuses work; it does not merely ask nicely.** A pass that has
   not been paid for cannot start. Relying on the daemon to sleep off its own
   debt worked until the "look now" button existed, at which point two passes
   landed in the same minute for 29% of a core.
3. **The window rolls; it does not reset.** A counter cleared every 60 seconds
   reports more work than elapsed time when sampled just after a burst — the
   status line said "133% of a core" while the daemon sat exactly on 15%.

Accepting a suggestion writes two ledger entries, not one: Weave proposed it, and
you accepted. Both stay blue, because a model wrote the words; *who* accepted is
in the reason, which is the question that matters on a shared vault.

---

## Crate map

Strict one-way fan-out. Nothing depends sideways.

| Crate | Responsibility |
|---|---|
| `arc-labs-core` | Vault model, path containment, byte fidelity, markdown rendering. No I/O policy, no UI, no network. |
| `arc-labs-api` | Every operation, once, transport-agnostic. |
| `arc-labs-bootstrap` | Platform detection, `doctor`, `setup`. The only crate allowed a network client. |
| `arc-labs-server` | Axum HTTP shell. Headless, browser, Docker. |
| `arc-labs-cli` | Composition root: `serve`, `doctor`, `setup`. |
| `arc-labs-app` | Tauri desktop shell. Commands only. |
| `xtask` | Fixture generation, token lint, fidelity manifests. Not shipped. |
| `arc-labs-weave` | The background daemon: embeddings, link suggestions, and the budget that outranks them. |
| `arc-labs-mcp` | MCP over stdio and HTTP. Seven tools, none of which writes. |
| `arc-labs-index` · `-ledger` · `-canvas` · `-runtime` | Phases 2–5. |

Two types in `arc-labs-core` carry more weight than their size suggests:

**`VaultPath`** is relative by construction and cannot become an absolute path
without a `VaultRoot`. Its `Deserialize` runs the same validation as its
constructor, so a traversal in an HTTP query string or an MCP argument is
rejected before any handler runs. Canonicalisation catches a vault file that is
a symlink to `/etc/shadow` — which no amount of string validation can.

**`NoteText`** carries the text *and* the evidence needed to undo its own
normalisation. Unchanged content re-encodes to the original bytes **verbatim**,
which is what makes "type a character, undo it, save, and the file is
byte-identical" achievable rather than aspirational.

---

## Build phases

| | | Gate |
|---|---|---|
| **0** | **Bench** | Opens a 5,000-note vault; renders every note; **vault byte-identical after reading all of it**; works in all four modes |
| 1 | Pen | 100 notes edited and undone are byte-identical; typing p99 < 16 ms |
| 2 | Index | 5,000 notes index in < 10 s; search < 50 ms; deleting the index loses nothing |
| 3 | Ledger | 50 mixed mutations, restore to state #17 exactly; a stranger spots agent activity in a screenshot |
| 4 | Canvas | 300 nodes at 60 fps; real `.canvas` fixtures round-trip byte-identical |
| 5 | Runtime | A four-node pipeline runs offline; cancel mid-stream leaves the note byte-identical |
| 6 | Bridge & Weave | A week of agent activity shows zero file changes without a matching accepted ledger entry; typing p99 stays under 16 ms with Weave live on 5,000 notes |
| 7 | Ship | Installers and image run on a clean machine with no toolchain |

---

## Building and testing

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo run -p xtask -- lint-tokens
```

Generate the fixture vault every performance gate runs against. It is
deterministic — the same seed produces byte-identical output forever, which is
what makes recorded baselines comparable across phases:

```bash
cargo run -p xtask -- gen-vault --notes 5000 --seed 42
```

Prove a vault was not touched:

```bash
cargo run -p xtask -- manifest --vault ~/notes --out before.json
```

```bash
cargo run -p xtask -- verify --vault ~/notes --manifest before.json
```

Run the server against a vault, with the UI built:

```bash
npm --prefix ui run build && cargo run -p arc-labs-cli -- serve --vault ~/notes
```

Desktop window (needs the platform's webview — `arc-labs doctor` will say):

```bash
npx --prefix ui tauri dev --config crates/arc-labs-app/tauri.conf.json
```

Docker:

```bash
docker build -t arc-labs:0.1.0 . && docker run --rm -v ~/notes:/vault:ro -p 7777:7777 arc-labs:0.1.0
```

### Line endings

`.gitattributes` disables git's line-ending translation for the whole repo.
This is not fussiness: `core.autocrlf` would rewrite the LF `.canvas` fixtures
on checkout and break the Phase 4 byte-compatibility oracle, in a way that looks
exactly like a code bug. A product whose value is byte fidelity cannot let its
own version control normalise its test data.

---

## Not being built

Named here so they stay decided:

a sync engine (use Syncthing or git) · a plugin API (freezing one before the
core is stable inherits a decade of compatibility debt) · a text editor
(CodeMirror 6 exists) · a mobile app · accounts, servers or workspaces · a
separate "Research Mode" (it is a filter over surfaces that already exist, not a
second application to maintain) · a discovery feed on the home screen (inferred
connections belong in the Weave inbox where the register rules apply, not on the
first screen where they read as fact) · an onboarding tour (Phase 0's
sixty-second test is the onboarding; if it needs a tour, the surface failed).

---

## License

MIT OR Apache-2.0
