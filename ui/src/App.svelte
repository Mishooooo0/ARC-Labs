<script lang="ts">
  /**
   * The application shell.
   *
   * Knows nothing about Tauri or HTTP — it talks to `transport`, which decided
   * once at load which shell it is in. That is the whole reason this file is the
   * same on Windows, on Linux, in a browser and in Docker.
   *
   * ## Three keystrokes, no mouse
   *
   * The Phase 2 gate is that every core action is reachable in three keystrokes
   * or fewer without touching the mouse. The shortcut table below is the whole
   * answer: one chord opens the palette, and inside it Tab switches between
   * commands and files, so anything reachable at all is reachable in two.
   */
  import { transport } from "./lib/transport";
  import type {
    Backlink, GraphData, IndexStats, NoteRef, NoteView as Note, OutgoingLink,
    CanvasView, Proposal, SearchHit, Status, TagCount, TimelineEntry, TreeView,
    UnresolvedLink,
  } from "./lib/types";
  import { TransportError } from "./lib/types";
  import ArcMark from "./components/ArcMark.svelte";
  import CanvasBoard from "./components/Canvas.svelte";
  import Context from "./components/Context.svelte";
  import DiffView from "./components/DiffView.svelte";
  import Editor, { type SaveState } from "./components/Editor.svelte";
  import EmptyState from "./components/EmptyState.svelte";
  import FileTree from "./components/FileTree.svelte";
  import FirstRun from "./components/FirstRun.svelte";
  import Graph from "./components/Graph.svelte";
  import Home from "./components/Home.svelte";
  import NoteViewer from "./components/NoteView.svelte";
  import Proposals from "./components/Proposals.svelte";
  import Palette, { type Command, type Mode } from "./components/Palette.svelte";
  import SaveStateBadge from "./components/SaveState.svelte";
  import SearchPane from "./components/SearchPane.svelte";
  import Timeline from "./components/Timeline.svelte";
  import VaultStatus from "./components/VaultStatus.svelte";

  type View = "home" | "note" | "search" | "graph";

  const THEMES = ["arc-dark", "arc-light", "arc-terminal"] as const;

  let status = $state<Status | null>(null);
  let tree = $state<TreeView | null>(null);
  let note = $state<Note | null>(null);
  let selected = $state<string | null>(null);
  let error = $state<string | null>(null);
  let theme = $state(localStorage.getItem("arc-labs-theme") ?? "arc-dark");

  let view = $state<View>("home");
  let editing = $state(false);
  let saveState = $state<SaveState>("clean");
  let saveDetail = $state<string | undefined>(undefined);
  let editor = $state<ReturnType<typeof Editor> | null>(null);

  // Index-backed state.
  let stats = $state<IndexStats | null>(null);
  let recent = $state<NoteRef[]>([]);
  let tags = $state<TagCount[]>([]);
  let unresolvedLinks = $state<UnresolvedLink[]>([]);
  let backlinks = $state<Backlink[]>([]);
  let outgoing = $state<OutgoingLink[]>([]);
  let contextLoading = $state(false);
  let graphData = $state<GraphData | null>(null);
  let graphLoading = $state(false);

  let searchQuery = $state("");
  let searchHits = $state<SearchHit[]>([]);
  let searching = $state(false);
  let tagFilter = $state<string | null>(null);
  let tagNotes = $state<NoteRef[]>([]);
  let searchPane = $state<ReturnType<typeof SearchPane> | null>(null);

  // Ledger state.
  let timeline = $state<TimelineEntry[]>([]);
  let proposals = $state<Proposal[]>([]);
  let selectedEntry = $state<number | null>(null);
  let entryPatch = $state<string>("");
  let proposalBusy = $state(false);
  let showTimeline = $state(true);

  let selectedTimelineEntry = $derived(
    selectedEntry === null ? null : (timeline[selectedEntry] ?? null),
  );

  let canvasView = $state<CanvasView | null>(null);

  let paletteOpen = $state(false);
  let paletteMode = $state<Mode>("commands");
  let showContext = $state(true);

  let isCanvas = $derived(selected?.toLowerCase().endsWith(".canvas") ?? false);
  let indexReady = $derived(stats !== null);

  $effect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("arc-labs-theme", theme);
  });

  function message(e: unknown): string {
    return e instanceof TransportError ? e.message : String(e);
  }

  async function refresh() {
    try {
      status = await transport.status();
      if (!status.vault) {
        tree = null;
        return;
      }
      status = { ...status, status: "scanning" };
      tree = await transport.tree();
      status = { ...status, status: "online" };
      void loadIndex();
    } catch (e) {
      error = message(e);
    }
  }

  /**
   * Poll for the index becoming ready.
   *
   * The server builds it on a background thread so the app is usable
   * immediately, which means the index arrives *after* the first paint. Rather
   * than block, the status indicator says INDEXING and this retries — a
   * 5,000-note vault takes about four seconds, and making the whole app wait for
   * that would be the wrong trade.
   */
  async function loadIndex(attempt = 0) {
    try {
      const [s, r, t, u] = await Promise.all([
        transport.indexStats(),
        transport.recent(20),
        transport.tags(),
        transport.unresolved(50),
      ]);
      stats = s;
      recent = r;
      tags = t;
      unresolvedLinks = u;
      if (status) status = { ...status, status: "online" };
    } catch {
      if (status?.vault) status = { ...status, status: "indexing" };
      if (attempt < 40) setTimeout(() => void loadIndex(attempt + 1), 500);
    }
  }

  async function openNote(path: string) {
    await editor?.flush();

    selected = path;
    view = "note";
    error = null;
    note = null;
    editing = false;
    saveState = "clean";
    saveDetail = undefined;
    paletteOpen = false;

    if (path.toLowerCase().endsWith(".canvas")) {
      canvasView = null;
      try {
        canvasView = await transport.canvas(path);
      } catch (e) {
        error = message(e);
      }
      void loadHistory(path);
      return;
    }
    canvasView = null;

    try {
      note = await transport.note(path);
    } catch (e) {
      error = message(e);
    }
    void loadContext(path);
    void loadHistory(path);
  }

  /**
   * A note's history and any open proposals.
   *
   * Reloaded after every write, because the timeline is the record of what
   * happened — a stale one would be the one surface in the product that lies.
   */
  async function loadHistory(path: string) {
    selectedEntry = null;
    entryPatch = "";
    try {
      const [t, p] = await Promise.all([transport.timeline(path), transport.proposals(path)]);
      if (selected === path) {
        timeline = t;
        proposals = p;
      }
    } catch {
      timeline = [];
      proposals = [];
    }
  }

  async function selectEntry(index: number | null) {
    selectedEntry = index;
    entryPatch = "";
    if (index === null || !selected) return;
    try {
      entryPatch = (await transport.entryDiff(selected, index)).patch;
    } catch (e) {
      error = message(e);
    }
  }

  async function restoreTo(index: number) {
    if (!selected) return;
    try {
      await transport.restore(selected, index);
      note = await transport.note(selected);
      await loadHistory(selected);
      void loadContext(selected);
    } catch (e) {
      error = message(e);
    }
  }

  async function decide(index: number, accept: boolean) {
    if (!selected) return;
    proposalBusy = true;
    try {
      if (accept) await transport.accept(selected, index);
      else await transport.reject(selected, index);
      note = await transport.note(selected);
      await loadHistory(selected);
      void loadContext(selected);
    } catch (e) {
      error = message(e);
    } finally {
      proposalBusy = false;
    }
  }

  async function loadContext(path: string) {
    if (!indexReady) return;
    contextLoading = true;
    try {
      const [b, o] = await Promise.all([transport.backlinks(path), transport.outgoing(path)]);
      // Guard against a slower response for a note the user has already left.
      if (selected === path) {
        backlinks = b;
        outgoing = o;
      }
    } catch {
      backlinks = [];
      outgoing = [];
    } finally {
      contextLoading = false;
    }
  }

  async function toggleEdit() {
    if (!selected || isCanvas) return;
    error = null;

    if (editing) {
      await editor?.flush();
      editing = false;
      try {
        note = await transport.note(selected);
        void loadContext(selected);
      } catch (e) {
        error = message(e);
      }
      return;
    }
    try {
      note = await transport.noteForEdit(selected);
      editing = true;
    } catch (e) {
      error = message(e);
    }
  }

  // Search-as-you-type, with a sequence guard so a slow response for an old
  // query cannot overwrite a fast one for a new query.
  let searchSeq = 0;
  $effect(() => {
    const q = searchQuery;
    if (view !== "search" || tagFilter) return;
    if (!q.trim()) {
      searchHits = [];
      return;
    }
    const mine = ++searchSeq;
    searching = true;
    void (async () => {
      try {
        const hits = await transport.search(q, 60);
        if (mine === searchSeq) searchHits = hits;
      } catch {
        if (mine === searchSeq) searchHits = [];
      } finally {
        if (mine === searchSeq) searching = false;
      }
    })();
  });

  async function openSearch() {
    view = "search";
    tagFilter = null;
    // A macrotask rather than a frame: see the note in Palette.svelte.
    setTimeout(() => searchPane?.focus(), 0);
  }

  async function openTag(tag: string) {
    view = "search";
    tagFilter = tag;
    try {
      tagNotes = await transport.tagNotes(tag);
    } catch (e) {
      error = message(e);
    }
  }

  async function openGraph() {
    view = "graph";
    if (graphData || graphLoading) return;
    graphLoading = true;
    try {
      graphData = await transport.graph();
    } catch (e) {
      error = message(e);
    } finally {
      graphLoading = false;
    }
  }

  function navigate(target: string, kind: "note" | "tag" | "embed") {
    if (kind === "tag") {
      void openTag(target);
      return;
    }
    // Resolution comes from the index when it is ready, so a click follows the
    // same rule the backlinks pane and the graph use.
    const hit = outgoing.find(
      (l) => l.target.toLowerCase() === target.toLowerCase() && l.resolvedPath,
    );
    if (hit?.resolvedPath) {
      void openNote(hit.resolvedPath);
      return;
    }
    const wanted = target.toLowerCase();
    const fromTree = tree?.entries.find(
      (e) => !e.is_dir && e.path.toLowerCase().replace(/\.[^./]+$/, "") === wanted,
    );
    if (fromTree) void openNote(fromTree.path);
    else error = `No note named "${target}" in this vault.`;
  }

  function cycleTheme() {
    const i = THEMES.indexOf(theme as (typeof THEMES)[number]);
    theme = THEMES[(i + 1) % THEMES.length] ?? "arc-dark";
  }

  async function reindex() {
    stats = null;
    graphData = null;
    if (status) status = { ...status, status: "indexing" };
    await refresh();
  }

  let commands = $derived<Command[]>([
    { id: "home", label: "Go home", hint: "⌘⇧H", run: () => (view = "home") },
    { id: "search", label: "Search vault", hint: "⌘⇧F", run: () => void openSearch() },
    { id: "graph", label: "Open graph", hint: "⌘⇧G", run: () => void openGraph() },
    {
      id: "edit",
      label: editing ? "Stop editing" : "Edit this note",
      hint: "⌘E",
      run: () => void toggleEdit(),
    },
    {
      id: "context",
      label: showContext ? "Hide backlinks pane" : "Show backlinks pane",
      hint: "⌘⇧B",
      run: () => (showContext = !showContext),
    },
    {
      id: "timeline",
      label: showTimeline ? "Hide history rail" : "Show history rail",
      hint: "⌘⇧T",
      run: () => (showTimeline = !showTimeline),
    },
    { id: "theme", label: `Theme: ${theme.replace("arc-", "")}`, run: cycleTheme },
    { id: "reindex", label: "Rebuild the index", run: () => void reindex() },
  ]);

  function onKey(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey;
    if (!mod) return;
    const k = e.key.toLowerCase();

    // ⌘K command palette, ⌘P quick open — the two doors to everything else.
    if (k === "k" && !e.shiftKey) {
      e.preventDefault();
      paletteMode = "commands";
      paletteOpen = true;
      return;
    }
    if (k === "p" && !e.shiftKey) {
      e.preventDefault();
      paletteMode = "files";
      paletteOpen = true;
      return;
    }
    if (e.shiftKey && k === "f") {
      e.preventDefault();
      void openSearch();
      return;
    }
    if (e.shiftKey && k === "g") {
      e.preventDefault();
      void openGraph();
      return;
    }
    if (e.shiftKey && k === "h") {
      e.preventDefault();
      view = "home";
      return;
    }
    if (e.shiftKey && k === "b") {
      e.preventDefault();
      showContext = !showContext;
      return;
    }
    if (e.shiftKey && k === "t") {
      e.preventDefault();
      showTimeline = !showTimeline;
      return;
    }
    if (k === "e" && !e.shiftKey) {
      e.preventDefault();
      void toggleEdit();
      return;
    }
    if (k === "s" && !e.shiftKey) {
      // Saving is continuous, but the reflex is universal — honour it rather
      // than letting the browser open a Save Page dialog over the notebook.
      e.preventDefault();
      void editor?.flush();
    }
  }

  $effect(() => {
    void refresh();
  });
</script>

<svelte:window onkeydown={onKey} />

{#if !status}
  <div class="boot">
    <ArcMark size={20} />
    <span class="data">starting…</span>
  </div>
{:else if !status.vault}
  <FirstRun {status} onopened={refresh} />
{:else}
  <div class="app">
    <header class="topbar">
      <button class="brand" onclick={() => (view = "home")} title="Home · ⌘⇧H">
        <ArcMark size={14} />
        <span class="data wordmark">ARC-LABS</span>
      </button>
      <VaultStatus status={status.status} name={status.vault.name} />
      <div class="spacer"></div>

      <SaveStateBadge state={saveState} detail={saveDetail} />

      <nav class="views data">
        {#each [["home", "home"], ["search", "search"], ["graph", "graph"]] as [id, label] (id)}
          <button
            class:active={view === id}
            onclick={() => {
              if (id === "search") void openSearch();
              else if (id === "graph") void openGraph();
              else view = "home";
            }}
          >
            {label}
          </button>
        {/each}
      </nav>

      {#if selected && !isCanvas && view === "note"}
        <button class="chip data" class:active={editing} onclick={toggleEdit} title="⌘E">
          {editing ? "editing" : "reading"}
        </button>
      {/if}
      <button class="chip data" onclick={() => (paletteOpen = true)} title="⌘K">⌘K</button>
      <button class="chip data" onclick={cycleTheme} title="Cycle theme">
        {theme.replace("arc-", "")}
      </button>
    </header>

    <div class="body">
      <aside class="sidebar">
        {#if tree && tree.entries.length}
          <FileTree {tree} {selected} onselect={openNote} />
        {:else if tree}
          <EmptyState
            title="This vault is empty"
            description="Add a markdown file to the folder and it will appear here."
            hint={status.vault.path ?? ""}
          />
        {/if}
      </aside>

      <main class="pane">
        {#if error}
          <div class="banner" class:bad={saveState === "conflict"} role="alert">
            <span>{error}</span>
            <button onclick={() => (error = null)} aria-label="Dismiss">×</button>
          </div>
        {/if}

        {#if view === "home"}
          <Home
            vaultName={status.vault.name}
            {stats}
            {recent}
            {tags}
            unresolved={unresolvedLinks}
            onopen={openNote}
            ontag={openTag}
          />
        {:else if view === "search"}
          <SearchPane
            bind:this={searchPane}
            bind:query={searchQuery}
            hits={searchHits}
            {tagFilter}
            {tagNotes}
            {searching}
            onopen={openNote}
            onclear={() => {
              searchQuery = "";
              tagFilter = null;
              searchHits = [];
            }}
          />
        {:else if view === "graph"}
          {#if graphData}
            <Graph data={graphData} {selected} onopen={openNote} />
          {:else}
            <EmptyState
              title={graphLoading ? "Laying out the graph" : "Graph"}
              description={graphLoading
                ? "Placing every note and every link between them. This runs off the main thread, so the rest of the app stays responsive."
                : "The graph needs the index. It will appear once indexing finishes."}
            />
          {/if}
        {:else if isCanvas}
          {#if canvasView}
            <CanvasBoard
              canvas={canvasView}
              onopen={openNote}
              onmove={async (moves) => {
                if (!selected) return;
                try {
                  await transport.moveCanvasNodes(selected, moves);
                  canvasView = await transport.canvas(selected);
                  await loadHistory(selected);
                } catch (e) {
                  error = message(e);
                }
              }}
            />
          {:else}
            <EmptyState
              title={selected?.split("/").pop() ?? "Canvas"}
              description="Reading this canvas…"
              hint={selected ?? ""}
            />
          {/if}
        {:else if editing && note?.text !== undefined}
          {#key note.path}
            <Editor
              bind:this={editor}
              path={note.path}
              initialText={note.text}
              baseHash={note.hash}
              onstate={(s, d) => {
                saveState = s;
                saveDetail = d;
                // A completed save is a new ledger entry; pull it in so the
                // rail is never behind the file it describes.
                if (s === "saved" && selected) void loadHistory(selected);
              }}
              onerror={(m) => (error = m)}
            />
          {/key}
        {:else if note}
          <div class="note-stack">
            {#if proposals.length}
              <div class="proposals">
                <Proposals
                  {proposals}
                  busy={proposalBusy}
                  onaccept={(i) => decide(i, true)}
                  onreject={(i) => decide(i, false)}
                />
              </div>
            {/if}
            <NoteViewer {note} onnavigate={navigate} />
          </div>
        {:else}
          <EmptyState
            title="Nothing open"
            description="Choose a note from the left, or press the key below to find one by name."
            hint="⌘P"
            onaction={() => {
              paletteMode = "files";
              paletteOpen = true;
            }}
            actionLabel="Find a note"
          />
        {/if}

        {#if selectedTimelineEntry}
          <DiffView
            entry={selectedTimelineEntry}
            patch={entryPatch}
            onrestore={restoreTo}
            onclose={() => selectEntry(null)}
          />
        {/if}
      </main>

      {#if showTimeline && view === "note" && selected && !isCanvas && timeline.length}
        <Timeline
          entries={timeline}
          selected={selectedEntry}
          onselect={selectEntry}
          onrestore={restoreTo}
        />
      {/if}

      {#if showContext && view === "note" && selected && !isCanvas}
        <aside class="context-pane">
          <Context
            path={selected}
            {backlinks}
            {outgoing}
            loading={contextLoading}
            onopen={openNote}
          />
        </aside>
      {/if}
    </div>
  </div>

  <Palette bind:open={paletteOpen} bind:mode={paletteMode} {commands} onopen={openNote} />
{/if}

<style>
  .boot {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--arc-space-3);
    color: var(--arc-fg-faint);
  }

  .app {
    height: 100%;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    height: var(--arc-topbar-h);
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--arc-space-4);
    padding: 0 var(--arc-space-4);
    background: var(--arc-bg-1);
    border-bottom: 1px solid var(--arc-line);
  }

  .brand {
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    color: var(--arc-fg-dim);
  }
  .brand:hover {
    color: var(--arc-fg);
  }
  .wordmark {
    letter-spacing: 0.1em;
  }

  .spacer {
    flex: 1;
  }

  .views {
    display: flex;
    gap: 1px;
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius-sm);
    overflow: hidden;
  }
  .views button {
    padding: 2px var(--arc-space-3);
    color: var(--arc-fg-faint);
    transition: color var(--arc-dur-fast) var(--arc-ease);
  }
  .views button:hover {
    color: var(--arc-fg-dim);
  }
  .views button.active {
    background: var(--arc-bg-3);
    color: var(--arc-fg);
  }

  .chip {
    color: var(--arc-fg-faint);
    padding: 2px var(--arc-space-2);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius-sm);
    transition:
      color var(--arc-dur-fast) var(--arc-ease),
      border-color var(--arc-dur-fast) var(--arc-ease);
  }
  .chip:hover {
    color: var(--arc-fg-dim);
    border-color: var(--arc-line-strong);
  }
  .chip.active {
    color: var(--arc-accent);
    border-color: var(--arc-accent-dim);
  }

  .body {
    flex: 1;
    display: flex;
    min-height: 0;
  }

  .sidebar {
    width: var(--arc-sidebar-w);
    flex: none;
    background: var(--arc-bg-1);
    border-right: 1px solid var(--arc-line);
    min-height: 0;
  }

  .context-pane {
    width: 250px;
    flex: none;
    background: var(--arc-bg-1);
    border-left: 1px solid var(--arc-line);
    min-height: 0;
  }

  .note-stack {
    height: 100%;
    overflow-y: auto;
  }
  .proposals {
    padding: var(--arc-space-5) var(--arc-space-7) 0;
  }
  .note-stack :global(.note) {
    height: auto;
    overflow: visible;
  }

  .pane {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--arc-bg-0);
  }

  .banner {
    flex: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--arc-space-3);
    padding: var(--arc-space-2) var(--arc-space-4);
    background: var(--arc-bg-2);
    border-bottom: 1px solid var(--arc-line);
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-sm);
  }
  .banner.bad {
    border-bottom-color: var(--arc-danger);
  }
  .banner button {
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-lg);
    line-height: 1;
  }
  .banner button:hover {
    color: var(--arc-fg);
  }
</style>
