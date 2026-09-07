# Changelog

## Unreleased

### The library became a bookcase

It is one piece of furniture now — two sides, a top with a lip, a solid back and
a plinth — rather than a set of free-floating planks. The arrangement carries the
meaning, and four rules fall out of it with no branch anywhere that states them:

| | |
|---|---|
| no folders | **no boards.** Notes lie on the plinth — the messy drawer |
| X folders | **X boards** |
| a board, no notes | an empty board, still a drop target |
| a board, X notes | that board holds those X notes |

All four hold because **a folder's board is the surface its notes stand on, and
the plinth is the surface unfiled notes stand on.** The vault root needs no board
because a bookcase already has a floor, so "no folders means no boards" is the
absence of a loop iteration rather than a special case. The four are four tests,
named after them.

Unfiled notes lean a few degrees; notes on a board stand straight. That is the
whole visual difference between filed and unfiled, and it is what makes the
bottom of the case read as a drawer.

- **Three finishes** — walnut, painted white, and the logo's own emerald
  (`#5f9e8c`, which is `--arc-accent`). A finish is a choice about furniture, so
  it is declared once and does not change with the theme; it persists across
  reloads. The grain is a luminance mask computed pixel by pixel at startup,
  because the CSP allows no image from anywhere and because canvas stroke
  rasterisation is not identical across engines — this way the bookcase is the
  same every time you open it.
- **You can walk around it.** The azimuth clamp is gone, pan is on, and there is
  a reset. Dragging still works from any angle because the layout plane is fixed
  in the world and the camera is what moves — and from behind, the back panel
  stops you picking a book through it, which is correct and free.
- **Note names run up their spines**, rotated to the book's *projected* axis so
  they stay on the spine at any angle, and hidden when the case is seen from
  behind — DOM text has no idea the back panel is there.
- **Dropping a note on the floor** moves it back out to the vault root.

**The cost of one choice, stated.** A folder whose books do not fit gets them
scaled down by a single factor rather than wrapping onto a second board, because
a second board would mean one folder had two shelves and rule #2 would stop being
true. On the 5,000-note fixture — nine folders of ~557 notes — that makes spines
a hairline at full-case framing. Zoom is the answer, and free rotation is what
makes zoom a real gesture rather than a workaround.

The column packing added one commit earlier is gone. It existed because one plank
per folder stacked vertically made a thin strip in a wide window; a carcass with
a real interior width is portrait on its own. `bays` is replaced by `finish`.

### The desktop app carries it

Rebuilt and installed, so the Windows app is no longer four commits behind the
repository — it predated the Emerald & Patina logo as well as the whole
library. `ARC-LABS_0.0.1_x64-setup.exe`, 6.4 MB, installed per user into
`%LOCALAPPDATA%\ARC-LABS`; both shortcuts and the uninstall entry updated.

```
3cfdeb7506f25ab492279e7e48066d97f7a4428db23abd5467f6fdec85cd4fe2  ARC-LABS_0.0.1_x64-setup.exe
```

The installed binary embeds exactly `index-BvVyoSBh.js`, `Library-f0rsBjCy.js`
and their two stylesheets, and no stale asset. Those names are content hashes,
which is what makes them proof rather than a timestamp.

The MSI at `target/release/wix/x64/output.msi` is **older than this** — it comes
from the run before the empty-vault fix below, and was not rebuilt. Use the NSIS
installer, which is what installs this app anyway.

The desktop shell's CSP is `script-src 'self'` with no `worker-src`, the same as
the server's — so the DOM-label decision was forced here too, not only in a
browser. Worth knowing before anyone tries to "simplify" it back to 3D text.

**An empty vault drew a white cube.** Found by pointing the app at one: an
`InstancedMesh` starts every instance at the identity matrix — a 1×1×1 box at
the world origin — and the mesh is allocated with at least one instance so a
count of zero is never asked for. With no notes the write loop never runs, so
that instance kept its default box, sitting exactly where the origin is: the
top-left corner of the case. Every vault built against had notes in it, so
nothing ever touched it. Unused instances are now collapsed, which also removes
the ghost frame after a note is deleted.

**Two things this build did not settle.**

*The version is still 0.0.1*, the same as the build it replaced, so nothing on
the machine distinguishes them. Cutting 0.0.2 is a release decision and has not
been made.

*`--bundles all` fails on this machine* with `failed to bundle project: Access is
denied. (os error 5)`. WiX itself succeeds — the MSI is written to
`target/release/wix/x64/output.msi` — and the failure is the copy to its final
name, where the previous MSI cannot be opened for writing or deleted even by its
owner with FullControl. That is a filter-driver or pending-delete hold rather
than a permission, and `target/` sitting inside the OneDrive root is the likeliest
cause. `--bundles nsis` is unaffected, and NSIS is what installs this app.

### The graph became a library

The force-directed graph is gone. Folders are shelves, notes are books standing
on them with a spine width taken from the note's length, and dragging a book to
another shelf moves the file after a confirm — the step that got it into 3D,
before the carcass above replaced the loose planks it drew.

### A Phase 6 gate surface was removed, deliberately

This is the part worth stating rather than discovering later. The Phase 6 gate
says an inferred link must be **"distinguishable from a real edge at every zoom
level"** — dashed, dimmer, blue. That was drawn by the graph, and the graph no
longer exists.

Weave's suggestions are not gone: they still reach the
[inbox](ui/src/components/Inbox.svelte), which lists them with their scores and
sources and still accepts or dismisses each one. What has gone is the *drawn*
half — nothing plots an inferred edge any more, so "distinguishable at every
zoom level" is now vacuously true rather than satisfied.

It was removed rather than lost. A shelf has no edges to dash, and the wire
graph that replaces it will carry *sync* relationships between vaults, which are
not link guesses and must not be dressed up as them. If inferred links get a
drawn surface again it will be its own thing, gated again on its own terms.

### Also

- Books are drawn as one `InstancedMesh` written by hand each frame, so a spawn
  or a settle costs one buffer upload rather than 5,000 component updates.
- Labels are DOM over the canvas, not geometry. The 3D text library builds its
  font atlas in a worker spawned from a `blob:` URL, which this app's CSP
  refuses — and the CSP is there to enforce constraint 3, so it stays as it is.
  Real text is also the first thing in this view a screen reader can read.
- `TreeEntry` gained `size`, which is what lets a spine be as wide as its note
  without waiting for an index.
- The library is loaded on demand: Three.js and Threlte are 800 KB that a
  session which never opens the library should not pay for. The entry bundle is
  unchanged at 420 KB.

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
  7abfe79c9a9b48d171bd24ac54083bd3c0ebb1f79fe900eabdfabc9da1d39f99  ARC-LABS_0.0.1_x64_en-US.msi
  fb29ab1f5a2c154eda195450559137fa44bedea73ffdd614b0d79791ee480965  ARC-LABS_0.0.1_x64-setup.exe
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
