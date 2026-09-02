# ARC-LABS — BUILD PROMPT v2

> ARC Collection / Obsidian Labs
> A lab notebook your agents can hold.
>
> v2 = the v1 engineering spine, with the product surface it was missing made binding rather than deferred.

---

## 0. MASTER BRIEF

You are building **ARC-LABS**, a local-first, agent-native knowledge workspace.

It does everything Obsidian does — plain-markdown vault, `[[wikilinks]]`, backlinks, tags, search, graph, canvas — and one thing Obsidian structurally cannot do: it treats **autonomous agents as first-class authors with full provenance**.

Two failure modes end this project. Build against both, continuously:

- **Clone failure** — it becomes a prettier Obsidian. Prevented by the ledger (Phase 3) and the executable canvas (Phase 5).
- **Instrument failure** — it becomes technically correct and nobody wants to open it. Prevented by the Surface Contract in §2, which is binding in *every* phase. A phase whose logic works and whose surface does not is not done.

### Hard constraints — violating any one fails the build

1. **Files are the source of truth.** Plain UTF-8 markdown on disk, Obsidian-compatible `[[wikilinks]]`, YAML frontmatter. The SQLite index is a *derived cache*: deletable and rebuildable at any moment with zero data loss.
2. **Bidirectional compatibility.** ARC-LABS must open an existing Obsidian vault unmodified, and any vault ARC-LABS touches must still open correctly in Obsidian afterwards.
3. **Fully offline.** No cloud service, no telemetry, no network call the user did not initiate. Models are local via Ollama. The app must work with the cable pulled.
4. **No silent agent writes.** Agent output lands as a *proposal* attached to the note and requires acceptance, unless a per-folder policy explicitly grants auto-accept.
5. **Immutable ledger.** Every mutation — human or agent — appends an entry with actor, timestamp, reason and content hash. Any note restores to any prior state.
6. **Provenance is visible without reading.** Authorship is encoded by colour before it is encoded by text: **amber = human, blue = agent**, in ledger entries, canvas node borders, diff gutters, timeline rail and link proposals. *Test:* screenshot any view, hand it to someone who has never used the app — they can point at what an agent touched.
7. **Never fabricate relationships.** Observed relationships (a real `[[link]]`, a real tag, a real folder) and inferred ones (embedding proximity, model suggestion) never share a visual register. Inferred edges are dashed, dimmer, and carry their source and score. A user must never click to find out whether a connection is real.

Constraints 6 and 7 are surface rules that earned constraint status: they are falsifiable, and they are the entire reason a user would trust agent activity in their own notes.

---

## 1. FIXED STACK

| Layer | Choice | Reason |
|---|---|---|
| Core | Rust, Tauri 2 | Small binary, real file-system performance, no Electron tax |
| Watch / parse | `notify`, `comrak` | Incremental indexing |
| Index | SQLite + FTS5 + `sqlite-vec` | One file, no server, vector search in-process |
| UI | TypeScript + Svelte 5 | No virtual-DOM overhead on a canvas with hundreds of live nodes |
| Editor | CodeMirror 6 | Do not write an editor. This is the single hardest component in the category |
| Canvas | DOM nodes on a CSS transform matrix | Nodes must host **live** CodeMirror instances — rules out canvas/WebGL libraries |
| Graph | `d3-force` in a web worker, drawn to `<canvas>` | SVG dies at a few thousand nodes; the worker keeps the UI thread free |
| Styling | CSS custom properties, one token file | Themes are a token swap, not a stylesheet fork |
| Models | Ollama HTTP (`nomic-embed-text` + small instruct model) | Already running in the ARC homelab |
| Attachments | Delegate to **ARC-Vault** (SHA-256 content-addressed blobs) | Do not reimplement storage that already exists — see §5 for the standalone fallback |

Deviate from a row only when its stated reason no longer holds. The reasons are the contract; the products are the implementation.

---

## 2. THE SURFACE CONTRACT

Binding in every phase below. Not a phase of its own.

### Direction

A research instrument built by someone with taste. An oscilloscope, not a spaceship cockpit. Deep layered darks, thin borders, one restrained accent, generous negative space. Monospace is reserved for *data* — paths, hashes, timestamps, token counts, tok/s — and never used for prose.

The reference is a precision tool you are glad to pick up, not an interface that announces the future.

### Avoid (the harder half of the brief)

- Excessive neon; glowing text
- Cyberpunk and gaming-UI clichés; HUD framing; "mission control" chrome
- Large gradients; cheap glassmorphism
- More than one accent hue in a single view
- Decorative animation of any kind
- Blank panes (see empty states)

### Reserved colours

Amber and blue are **semantic, never decorative**. They mean human and agent authorship and nothing else, anywhere in the product. Pick the neutral accent for the rest of the UI so it never collides with either.

### Motion budget

120–200 ms, ease-out, always interruptible. No animation on any path that runs per-keystroke. Animate to explain state change — a proposal materialising, a node beginning to run, a relationship appearing — never to decorate. If motion makes the app feel slower, it is a bug, not a preference.

### Empty states

Never show a blank surface. Every empty pane states what it is, what belongs in it, and offers exactly one primary action. An empty vault, an empty canvas, an empty search, an empty agent inbox and a zero-backlink note are five separate designs, not one generic placeholder.

### Themes

Three at minimum: **ARC Dark** (default), **ARC Light**, **ARC Terminal** (monochrome). User-configurable accent, font, UI density and motion intensity. All of it resolves to custom properties in a single token file — if a theme requires touching component CSS, the token layer is wrong.

---

## PHASE 0 — Bench

Scaffold: Tauri 2 + Svelte 5 + TypeScript, Rust workspace with `arc-labs-core` (library) and `arc-labs-app` (Tauri binary).

Vault opening: folder picker storing the vault root in `~/.config/arc-labs/config.toml`, recursive file tree in the left pane, read-only rendered markdown in the right. Handle `[[wikilinks]]`, YAML frontmatter, `#tags` and embeds (`![[...]]`) at render time only.

**Nothing in this phase writes to disk inside the vault.**

**Surface:** first-run screen (create vault / open existing), the empty-vault state, the ARC mark, the token file and ARC Dark theme, and the vault status indicator — `● VAULT ONLINE` / `INDEXING…` — which is load-bearing from here on.

*Acceptance:* opens a real 3,000+ note Obsidian vault; renders any note correctly; `git status` inside the vault is clean after an hour of browsing. **A person who has never seen the app gets from launch to a rendered note in under 60 seconds with no instructions.** No blank panes exist anywhere in the build.

---

## PHASE 1 — Pen

CodeMirror 6 in live-preview mode: markdown source stays authoritative, rendered widgets replace syntax in place, cursor entering a widget reveals its source.

Required decorations: headings, bold/italic, `[[wikilink]]` (rendered as a link, source on cursor), tags, code blocks with highlighting, block quotes, task checkboxes, tables.

Saving is debounced (400 ms) and atomic: write temp file, fsync, rename.

**Surface:** the widget-reveal transition, the save-state indicator, and the note-open transition (one subtle fade, inside the motion budget).

*Acceptance:* open 100 notes, type one character in each, undo it, save. All 100 files are **byte-identical** to their originals — no trailing-whitespace drift, no line-ending changes, no frontmatter reordering. **Typing latency p99 under 16 ms in a 5,000-word note. Cursor entering a widget produces zero layout shift.**

---

## PHASE 2 — Index

Derived index in SQLite: `notes`, `links`, `tags`, `blocks` plus an FTS5 virtual table over note bodies. Populate on first open with a progress bar; keep current with a `notify` watcher doing incremental single-note reindexing.

Ship: full-text search with snippets, backlinks pane, unresolved-link list, tag browser, force-directed graph view. Add `arc-labs reindex` as a CLI subcommand and prove disposability by deleting `.arc/index.db` and reopening.

**Surface:** the command palette and the home surface — the two things that make an index feel like a product instead of a query engine.

Palette (`⌘K`) and quick open (`⌘P`). Baseline shortcuts:

```
⌘K   Command palette        ⌘⇧G  Graph
⌘P   Quick open             ⌘⇧C  Canvas
⌘N   New note               ⌘⇧T  Timeline
⌘⇧N  New canvas             ⌘⇧A  Agent inbox
⌘F   Search in note         ⌘⇧F  Search vault
⌘S   Save                   ⌘B / ⌘I  Bold / italic
```

Home surface, shown when nothing is open: note and link counts, recent notes, open canvases, agent inbox count. Nothing inferred yet — inference arrives in Phase 6 and gets its own register per constraint 7.

*Acceptance:* 5,000 notes cold-index in under 10 s; any search returns in under 50 ms; deleting the index file loses nothing; the graph stays interactive at 5,000 nodes. **The palette opens in under 50 ms, and every core action is reachable in three keystrokes or fewer without touching the mouse.**

---

## PHASE 3 — Ledger

This is the phase that makes ARC-LABS different. Build it properly.

Per note, an append-only JSONL ledger at `.arc/ledger/<blake3-of-relpath>.jsonl`. One entry per mutation:

```json
{
  "ts": "2026-09-02T14:03:11Z",
  "actor": {"kind": "human", "id": "mishal"},
  "op": "edit",
  "reason": "manual edit",
  "before": "blake3:...",
  "after": "blake3:...",
  "patch": "@@ -12,3 +12,5 @@ ..."
}
```

`actor.kind` is `human` or `agent`; agents also carry `model` and `session`. `op` is one of `create | edit | rename | delete | propose | accept | reject`.

**Surface:** a **Timeline** rail down the right edge of the editor — one thin bar per entry, amber for human, blue for agent, colour density showing where an agent has been working. Click a bar for the diff; restore any point. Proposals render as inline suggestion blocks with Accept / Reject. A proposal never touches the file until accepted.

*Acceptance:* mutate a note 50 times through mixed human and simulated agent actions, then restore it to state #17 exactly. The ledger survives the note being renamed. **Screenshot the timeline and hand it to someone who has not used the app — they identify agent activity without reading a word. A proposal that is never accepted leaves the file's mtime untouched.**

---

## PHASE 4 — Bench Canvas

Canvas as a **superset of the open JSONCanvas spec**, saved as `.canvas` files in the vault.

Standard node types (`text`, `file`, `link`, `group`) round-trip byte-compatibly so Obsidian opens the same file. ARC-LABS adds `prompt`, `query` and `transform`, serialised as `type: "text"` with an `arc` metadata key, so an unknown reader degrades them to plain text cards instead of erroring.

Mechanics: pan/zoom via a CSS `matrix()` on a single transform layer; nodes are absolutely positioned DOM elements; hit-testing in transformed coordinates; edges as SVG bezier curves beneath the nodes; box-select, multi-drag, snap-to-grid, z-order.

`file` nodes embed a **live** CodeMirror instance — editing inside a card writes to the underlying note and appends to its ledger. Virtualise: only nodes intersecting the viewport instantiate an editor; the rest render a static preview.

**Surface:** the empty-canvas state, node authorship borders (constraint 6), and a visible distinction between a live-editor node and a virtualised preview node — the user must never wonder whether a card is editable.

*Acceptance:* a 300-node canvas pans at 60 fps; a canvas created in ARC-LABS opens in Obsidian without a warning; a canvas created in Obsidian opens here unchanged. **Every canvas card's authorship is readable at 40% zoom.**

---

## PHASE 5 — Runtime

Make the canvas executable. This is the payoff.

- `prompt` node — a prompt template with `{{slots}}` filled from inbound edges.
- `query` node — a vault query (FTS or vector) whose results flow outward.
- `transform` node — a small deterministic operation (extract frontmatter, split by heading, dedupe, map to list).

Edges carry data. Run executes the ancestor subgraph in topological order against Ollama, streaming tokens into the node body. Cycles are rejected at edit time with a marker on the offending edge.

Any output written to a note is written as an **agent proposal**, entering the Phase 3 ledger with the model name and canvas node id as the reason. Runs are cancellable and show live token throughput — knowing tok/s on constrained hardware is not optional.

**Egress control.** Constraint 3 makes models local, but a configurable remote Ollama endpoint is still an egress path. So egress is *ledgered*, not merely configured:

```
MODEL ACCESS
  ● Local only — this machine                    (default)
  ○ Trusted endpoint — <configured host>
  ○ Ask each run
```

Any run that sends vault bytes off this machine writes an `egress` ledger entry naming the destination, the note ids and the byte count, and shows a persistent indicator while it is in flight. There is no configuration in which vault content leaves silently.

*Hardware note:* the primary node has no GPU and roughly 0.24 GB/s effective memory bandwidth. Execution defaults to a small local model, runs one node at a time, and exposes a configurable remote endpoint so heavy runs can target the workstation node when it is awake.

**Surface:** a running node is distinguishable from an idle one peripherally, without reading. Streaming text must not reflow the canvas or steal focus.

*Acceptance:* a four-node pipeline (query → prompt → transform → note) runs end to end offline; the resulting note shows a blue ledger entry naming the model and node; rejecting the proposal leaves the file untouched. **Cancelling mid-stream leaves the target note byte-identical to before the run. A `SIGKILL` mid-write leaves no partial file. A cycle introduced at edit time is marked within 100 ms and disables Run. Every run records tok/s and peak RSS. The same graph run twice produces identical `transform` node output.**

---

## PHASE 6 — Bridge & Weave

**Bridge** — expose ARC-LABS as an MCP server over stdio and HTTP so E-Tron, ARC-SLASH and Claude Desktop all speak to the vault through one surface. This replaces the Local REST API plugin approach entirely.

Tools: `vault_search` (text + vector), `note_read`, `note_propose` (never `note_write`), `link_suggest`, `canvas_read`, `canvas_run`, `ledger_read`. Every call is logged; every mutation path goes through the proposal system. There is no tool that writes directly.

**Weave** — a background daemon embedding changed notes with `nomic-embed-text` into `sqlite-vec`, then proposing links between semantically close, structurally unlinked notes. Proposals land in an inbox pane, never in the files.

Weave's budget is a hard requirement, not a tuning parameter — it shares a machine with the editor, and Phase 1's typing target outranks it:

- No more than 15% of one core, averaged over any 60 s window
- Zero work within 2 s of a keystroke
- Hard stop if the index write queue exceeds its threshold
- Resumable: killing the app mid-batch loses at most the current note

**Surface:** the agent inbox, and the inferred-relationship register required by constraint 7 — dashed, dimmer, scored, sourced. An inferred edge in the graph must be distinguishable from a real one at a glance, at every zoom level.

*Acceptance:* an external agent can search, read and propose through MCP; a full audit of the vault's git history over a week of agent activity shows **zero** file changes without a matching accepted ledger entry. **Typing latency (Phase 1) is unchanged with Weave running on a 5,000-note vault.**

---

## DO NOT BUILD

- **A sync engine.** Syncthing or git for v1. CRDT sync is a multi-year product on its own.
- **A plugin API.** Freezing an API before the core is stable is how you inherit a decade of compatibility debt.
- **A text editor.** CodeMirror 6 exists.
- **A mobile app.** Not until 0–6 are stable on desktop.
- **An account system, a server, a "workspace", or anything requiring a network.**
- **A separate "Research Mode."** Its value is real but it is not a mode — a topic workspace is Phase 2's local graph, plus Phase 6's vector search and inbox, filtered to one subject. Ship it as a filter over surfaces that already exist. A mode is a second application to maintain.
- **A discovery feed on Home.** Inferred connections belong in the Weave inbox where the register rules apply, not on the first screen where they will be read as fact.
- **An onboarding tour.** Phase 0's 60-second test is the onboarding. If it needs a tour, the surface failed.
- **A prettier Obsidian.** If the ledger and the executable canvas are not working, there is no reason for this software to exist.

---

## WHERE ARC CONTEXT IS ASSUMED

This prompt assumes the ARC Collection. Each assumption is one interface with a standalone fallback, so nothing in the build hard-depends on the rest of the ecosystem:

| Assumption | Standalone fallback |
|---|---|
| ARC-Vault for attachments | Content-addressed files in the vault's own `Attachments/`, SHA-256 filenames. One trait, two implementations. |
| E-Tron / ARC-SLASH as MCP clients | Any MCP client. Phase 6 depends on none of them. |
| Ollama on the homelab | A single config value; `localhost:11434` default. |
| Actor id `mishal` | Read from config; fall back to `$USER`. |

---

## THE TWO TESTS

**Engine:** *"Which of my 4,000 notes did an agent touch last week, what did it change, and can I undo just that?"*

**Surface:** hand it to someone for ten minutes with no explanation. They can say what it is for, and point at something an agent did.

Fail the first and it is a clone. Fail the second and it is a lab instrument nobody opens. It has to pass both.
