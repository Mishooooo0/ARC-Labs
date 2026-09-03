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
    ApiVersion, CanvasRunnability, CanvasView, Config, CreateKind, LinkSuggestion, Proposal, RunStatus, SearchHit, Status, Template,
    TagCount, TimelineEntry, TreeView, UnresolvedLink, VaultEvent, WeaveStatus,
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
  import Ask from "./components/Ask.svelte";
  import Home from "./components/Home.svelte";
  import Inbox from "./components/Inbox.svelte";
  import Settings from "./components/Settings.svelte";
  import Create from "./components/Create.svelte";
  import NoteViewer from "./components/NoteView.svelte";
  import Proposals from "./components/Proposals.svelte";
  import RunPanel from "./components/RunPanel.svelte";
  import Palette, { type Command, type Mode } from "./components/Palette.svelte";
  import SaveStateBadge from "./components/SaveState.svelte";
  import SearchPane from "./components/SearchPane.svelte";
  import Timeline from "./components/Timeline.svelte";
  import VaultStatus from "./components/VaultStatus.svelte";

  type View = "home" | "note" | "search" | "graph" | "inbox";

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

  // Weave. Inferred links live in their own state, never merged into
  // `outgoing` or `backlinks` — those are observed, and the two must not be
  // able to reach the same component by the same route.
  let suggestions = $state<LinkSuggestion[]>([]);
  let weave = $state<WeaveStatus | null>(null);
  let weaveBusy = $state<number | null>(null);
  let weaveWorking = $state(false);

  // The one dialog. `askKind` decides what confirming does, so a stale callback
  // from a dismissed dialog cannot fire against the wrong note.
  type AskKind = "create" | "rename" | "delete";
  let askOpen = $state(false);
  let askKind = $state<AskKind>("create");
  let askTitle = $state("");
  let askDetail = $state<string | undefined>(undefined);
  let askValue = $state<string | undefined>(undefined);
  let askConfirm = $state("OK");
  let askDanger = $state(false);
  let askTarget = $state<string | null>(null);

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
  let runnability = $state<CanvasRunnability | null>(null);
  let activeRun = $state<RunStatus | null>(null);
  let runningNodes = $state<Set<string>>(new Set());
  let lastRunTarget: string | null = null;

  /**
   * Poll a run while it is in flight.
   *
   * 10 Hz: fast enough that streamed text reads as live and the throughput
   * figure moves smoothly, slow enough that the cost is nothing. See
   * `arc-labs-api::runs` for why this is a poll rather than a push.
   */
  async function watchRun(id: string) {
    for (;;) {
      let status: RunStatus;
      try {
        status = await transport.runStatus(id);
      } catch (e) {
        error = message(e);
        runningNodes = new Set();
        return;
      }
      activeRun = status;
      runningNodes = new Set(status.nodes.filter((n) => n.running).map((n) => n.id));

      if (status.state !== "running") {
        runningNodes = new Set();
        // A finished run may have left proposals and ledger entries behind.
        if (selected) {
          void loadHistory(selected);
          canvasView = await transport.canvas(selected).catch(() => canvasView);
        }
        return;
      }
      await new Promise((r) => setTimeout(r, 100));
    }
  }

  async function runNode(nodeId: string, approveEgress = false) {
    if (!selected) return;
    error = null;
    lastRunTarget = nodeId;
    try {
      const id = await transport.startRun(selected, nodeId, approveEgress);
      void watchRun(id);
    } catch (e) {
      error = message(e);
    }
  }

  let paletteOpen = $state(false);
  let paletteMode = $state<Mode>("commands");
  let showContext = $state(true);

  let isCanvas = $derived(selected?.toLowerCase().endsWith(".canvas") ?? false);
  let indexReady = $derived(stats !== null);

  /** Density, like the theme, is a token swap on the root element. */
  let density = $state<"comfortable" | "compact">("comfortable");

  $effect(() => {
    document.documentElement.setAttribute("data-theme", theme);
    localStorage.setItem("arc-labs-theme", theme);
  });

  $effect(() => {
    // "comfortable" is the absence of the attribute, so the base tokens are the
    // default rather than one branch of two.
    if (density === "compact") {
      document.documentElement.setAttribute("data-density", "compact");
    } else {
      document.documentElement.removeAttribute("data-density");
    }
  });

  /**
   * Live changes from every other surface on this vault.
   *
   * The desktop window and a browser tab share one engine, so a note created in
   * one appears in the other without a refresh. External writers land here too,
   * once a watcher exists.
   */
  $effect(() => {
    const stop = transport.subscribe((e) => void onVaultEvent(e));
    return stop;
  });

  /** Set when a remote change lands on the note being edited with unsaved work. */
  let staleWarning = $state<string | null>(null);

  /**
   * Why the app could not start.
   *
   * The boot screen used to be the one surface in ARC-LABS that could show
   * nothing useful for ever: the error banner lives inside the booted layout, so
   * anything that failed before `status` was set left "starting…" on screen with
   * no explanation and no way forward. A remote server with a stale token — the
   * single most likely first-run problem — looked exactly like a hang.
   */
  let bootError = $state<{ message: string; needsToken: boolean } | null>(null);

  // Settings.
  let config = $state<Config | null>(null);
  let apiVersion = $state<ApiVersion | null>(null);
  let savingConfig = $state(false);
  let configAdjusted = $state<string | null>(null);
  let tokenEntry = $state("");

  async function onVaultEvent(e: VaultEvent) {
    // Our own doing. Reacting to it would mean the surface that just saved
    // reloads because of its own save, and the editor would fight the person
    // typing into it.
    if (e.origin && e.origin === transport.clientId) return;

    // The stream dropped events for us, so what we hold may be wrong in ways we
    // cannot enumerate. Refetch rather than guess.
    if (e.kind === "lagged") {
      await refreshTree();
      void loadIndex();
      return;
    }

    if (e.kind === "indexReady") {
      void loadIndex();
      return;
    }
    if (e.kind === "suggested" || e.kind === "proposed") {
      // Inbox and proposal counts move; nothing on disk did.
      void loadInbox();
      if (selected && e.path === selected) void loadHistory(selected);
      return;
    }

    // Anything structural changes the tree.
    if (e.kind === "created" || e.kind === "deleted" || e.kind === "renamed") {
      await refreshTree();
      void loadIndex();
    }

    const touched = e.path === selected || (e.from && e.from === selected);
    if (!touched || !selected) return;

    if (e.kind === "deleted") {
      selected = null;
      note = null;
      view = "home";
      error = "The note you were reading was deleted somewhere else.";
      return;
    }
    if (e.kind === "renamed" && e.path) {
      await openNote(e.path);
      return;
    }

    // The note under the cursor changed elsewhere. This is the one case that can
    // destroy work, so it is the one case that never acts on its own: if there
    // are unsaved edits, say so and let the person decide. Silently replacing
    // what someone is typing is the worst thing this feature could do.
    if (editing && saveState !== "clean") {
      staleWarning =
        "This note changed somewhere else while you were editing. Your unsaved " +
        "text is untouched — saving will be refused until you reload.";
      return;
    }
    await openNote(selected);
    void loadHistory(selected);
  }

  function message(e: unknown): string {
    return e instanceof TransportError ? e.message : String(e);
  }

  async function refresh() {
    try {
      // The handshake first. Everything below assumes the two ends agree about
      // what the wire means, and this is where that is established. A major
      // mismatch throws here rather than halfway through rendering.
      apiVersion = await transport.version();
      status = await transport.status();
      bootError = null;
      // Read once at boot so a saved density and theme apply immediately,
      // rather than only after someone opens the settings panel.
      void transport
        .config()
        .then((c) => {
          config = c;
          theme = c.ui.theme;
          density = c.ui.density === "compact" ? "compact" : "comfortable";
        })
        .catch(() => {});
      if (!status.vault) {
        tree = null;
        return;
      }
      status = { ...status, status: "scanning" };
      tree = await transport.tree();
      status = { ...status, status: "online" };
      void loadIndex();
    } catch (e) {
      // If we never got as far as a status, the booted layout does not exist
      // yet and its error banner cannot be seen. Record it for the boot screen
      // instead of leaving "starting…" on the glass.
      if (!status) {
        const needsToken = e instanceof TransportError && e.code === "not_permitted";
        bootError = {
          message: needsToken
            ? "This server needs an access token."
            : message(e),
          needsToken,
        };
      } else {
        error = message(e);
      }
    }
  }

  /** Store a pasted token and start again. */
  function useToken() {
    const t = tokenEntry.trim();
    if (!t) return;
    try {
      sessionStorage.setItem("arc-labs-token", t);
    } catch {
      // Private mode. The reload below still carries it in the URL.
    }
    // A reload rather than a retry in place: the transport reads the token once
    // when it is constructed, and reloading is the one way to be certain every
    // later request uses the new one.
    location.href = `${location.pathname}?token=${encodeURIComponent(t)}`;
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
      // So the inbox count is right before anyone opens the inbox.
      void loadInbox();
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
      try {
        runnability = await transport.runnability(path);
      } catch {
        runnability = null;
      }
      return;
    }
    canvasView = null;
    runnability = null;

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

  /** Settings is an overlay, not a view: it never takes the pane. */
  let settingsOpen = $state(false);

  async function openSettings() {
    settingsOpen = true;
    configAdjusted = null;
    try {
      config = await transport.config();
      density = config.ui.density === "compact" ? "compact" : "comfortable";
    } catch (e) {
      error = message(e);
    }
  }

  async function saveConfig(next: Config) {
    savingConfig = true;
    configAdjusted = null;
    try {
      // Render what came *back*, not what was sent: the server clamps the Weave
      // budget, refuses to move the vault and bounds motion. A panel showing the
      // value you typed would quietly disagree with the one in force.
      const stored = await transport.saveConfig(next);
      const changes: string[] = [];
      if (stored.weave.cpuFraction !== next.weave.cpuFraction) {
        changes.push(
          `the CPU ceiling stayed at ${Math.round(stored.weave.cpuFraction * 100)}% — 15% is a hard limit`,
        );
      }
      if (stored.ui.motion !== next.ui.motion) {
        changes.push(`motion was clamped to ${stored.ui.motion}×`);
      }
      if (stored.vault !== next.vault) {
        changes.push("the vault is changed by opening one, not here");
      }
      configAdjusted = changes.length ? `Saved, with adjustments: ${changes.join("; ")}.` : null;

      config = stored;
      theme = stored.ui.theme;
      density = stored.ui.density === "compact" ? "compact" : "comfortable";
      // Motion is a CSS token, so a change has to reach the document.
      document.documentElement.style.setProperty("--arc-motion", String(stored.ui.motion));
    } catch (e) {
      error = message(e);
    } finally {
      savingConfig = false;
    }
  }

  async function openInbox() {
    view = "inbox";
    await loadInbox();
  }

  async function loadInbox() {
    try {
      // Both, always together: a suggestion list with no daemon state behind it
      // cannot explain why it is empty, and "empty" is this pane's normal state.
      [suggestions, weave] = await Promise.all([
        transport.suggestions(50),
        transport.weaveStatus(),
      ]);
    } catch {
      // The index may not be up yet. The empty state says so; an error banner
      // over a pane the user just opened would be noise.
    }
  }

  async function acceptSuggestion(id: number) {
    weaveBusy = id;
    try {
      await transport.acceptSuggestion(id);
      await loadInbox();
      // The link is real now, so anything showing observed links is stale.
      if (selected) await loadContext(selected);
      graphData = null;
      if (selected && note) await openNote(selected);
    } catch (e) {
      error = message(e);
    } finally {
      weaveBusy = null;
    }
  }

  async function dismissSuggestion(id: number) {
    weaveBusy = id;
    try {
      await transport.dismissSuggestion(id);
      await loadInbox();
    } catch (e) {
      error = message(e);
    } finally {
      weaveBusy = null;
    }
  }

  async function weavePass() {
    weaveWorking = true;
    try {
      await transport.weavePass();
      await loadInbox();
    } catch (e) {
      error = message(e);
    } finally {
      weaveWorking = false;
    }
  }

  // ── Note lifecycle ─────────────────────────────────────────────────────

  // ── Creation ────────────────────────────────────────────────────────────
  let createOpen = $state(false);
  let createFolder = $state("");
  let createKind = $state<CreateKind>("note");
  let templates = $state<Template[]>([]);
  let drafting = $state(false);
  let draftError = $state<string | null>(null);
  let draft = $state<string | null>(null);

  /**
   * Open the creation window, aimed at `folder` (or the selection's folder)
   * and at `kind`.
   *
   * `folder` comes from wherever the user was: a new note made while reading
   * `Daily/2026-09-03.md` belongs in `Daily/`, not at the vault root. Guessing
   * that correctly is the difference between a feature and a chore.
   */
  async function newThing(folder?: string, kind: CreateKind = "note") {
    createFolder =
      folder ??
      (selected?.includes("/") ? selected.slice(0, selected.lastIndexOf("/")) : "");
    createKind = kind;
    draft = null;
    draftError = null;
    createOpen = true;
    try {
      templates = await transport.templates();
    } catch {
      // No index or no templates folder. The picker simply has nothing to
      // offer, which is the normal state of a vault that has never used one.
      templates = [];
    }
  }

  async function create(kind: CreateKind, name: string, template: string | null) {
    createOpen = false;
    const dir = createFolder ? `${createFolder}/` : "";
    const stem = name.replace(/\.(md|canvas)$/i, "");

    try {
      if (kind === "folder") {
        await transport.createFolder(`${dir}${stem}`);
        await refreshTree();
        return;
      }
      if (kind === "canvas") {
        const path = `${dir}${stem}.canvas`;
        await transport.createCanvas(path);
        await refreshTree();
        await openNote(path);
        return;
      }

      // A note. Collisions become "Untitled 2" rather than an error to think
      // about — the same rule the old prompt used.
      let path = `${dir}${stem}.md`;
      try {
        path = await transport.uniquePath(`${dir}${stem}`);
      } catch {
        /* no index yet; the create call reports anything real */
      }
      if (template) await transport.createFromTemplate(path, template);
      else await transport.createNote(path, "");
      await refreshTree();
      await openNote(path);
      editing = true;
    } catch (e) {
      error = message(e);
    }
  }

  async function draftTemplate(description: string) {
    drafting = true;
    draftError = null;
    try {
      draft = await transport.draftTemplate(description);
    } catch (e) {
      // The model's own reason, not a generic failure: "no such model" and
      // "not reachable" need different actions from whoever reads it.
      draftError = message(e);
    } finally {
      drafting = false;
    }
  }

  /**
   * Save a drafted template.
   *
   * `true` is not a parameter here because the only way to reach this is the
   * button under the draft preview. If a hand-written template ever gets its
   * own save, that one passes `false` — and the flag exists so it can.
   */
  async function saveTemplate(name: string, body: string) {
    try {
      await transport.saveTemplate(name, body, true);
      templates = await transport.templates();
      await refreshTree();
    } catch (e) {
      draftError = message(e);
    }
  }

  function renameNote(path?: string) {
    const target = path ?? selected;
    if (!target) return;
    askKind = "rename";
    askTitle = "Rename note";
    askDetail = "Its history comes with it.";
    askValue = target;
    askConfirm = "Rename";
    askDanger = false;
    askTarget = target;
    askOpen = true;
  }

  function deleteNote(path?: string) {
    const target = path ?? selected;
    if (!target) return;
    askKind = "delete";
    askTitle = `Delete ${target.split("/").pop()}?`;
    // Say what is actually true. It is recoverable, and saying so is the
    // difference between a confident click and a nervous one.
    askDetail = "The file moves to the vault's trash and its history is kept, so this can be undone.";
    askValue = undefined;
    askConfirm = "Delete";
    askDanger = true;
    askTarget = target;
    askOpen = true;
  }

  async function askConfirmed(answer: string) {
    askOpen = false;
    error = null;
    try {
      if (askKind === "create") {
        const path = answer.endsWith(".md") ? answer : `${answer}.md`;
        const created = await transport.createNote(path, "");
        await refreshTree();
        await openNote(path);
        // Straight into the editor: you asked for a new note in order to write.
        if (!editing) await toggleEdit();
        void created;
      } else if (askKind === "rename" && askTarget) {
        const to = answer.endsWith(".md") ? answer : `${answer}.md`;
        await transport.renameNote(askTarget, to);
        await refreshTree();
        await openNote(to);
      } else if (askKind === "delete" && askTarget) {
        const gone = askTarget;
        const out = await transport.deleteNote(gone);
        await refreshTree();
        if (selected === gone) {
          selected = null;
          note = null;
          view = "home";
        }
        error = out.recoverable
          ? `Deleted ${gone.split("/").pop()} — its history is kept, so it can be restored.`
          : null;
      }
      void loadIndex();
    } catch (e) {
      error = message(e);
    }
  }

  async function refreshTree() {
    try {
      tree = await transport.tree();
      status = await transport.status();
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
    { id: "new", label: "New note", hint: "⌘N", run: () => void newThing() },
    {
      id: "new-folder",
      label: "New folder",
      run: () => void newThing(undefined, "folder"),
    },
    {
      id: "new-canvas",
      label: "New canvas",
      run: () => void newThing(undefined, "canvas"),
    },
    {
      id: "rename",
      label: selected ? `Rename ${selected.split("/").pop()}` : "Rename this note",
      hint: "F2",
      run: () => renameNote(),
    },
    {
      id: "delete",
      label: selected ? `Delete ${selected.split("/").pop()}` : "Delete this note",
      run: () => deleteNote(),
    },
    { id: "home", label: "Go home", hint: "⌘⇧H", run: () => (view = "home") },
    { id: "search", label: "Search vault", hint: "⌘⇧F", run: () => void openSearch() },
    { id: "graph", label: "Open graph", hint: "⌘⇧G", run: () => void openGraph() },
    {
      id: "inbox",
      label: suggestions.length
        ? `Agent inbox (${suggestions.length} suggested)`
        : "Agent inbox",
      hint: "⌘⇧I",
      run: () => void openInbox(),
    },
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
    { id: "settings", label: "Settings", hint: "⌘,", run: () => void openSettings() },
    { id: "theme", label: `Theme: ${theme.replace("arc-", "")}`, run: cycleTheme },
    { id: "reindex", label: "Rebuild the index", run: () => void reindex() },
  ]);

  function onKey(e: KeyboardEvent) {
    // F2 carries no modifier, so it is checked before the modifier gate. It is
    // also the one shortcut a Windows user reaches for without being told.
    if (e.key === "F2" && !e.ctrlKey && !e.metaKey && selected && !isCanvas) {
      e.preventDefault();
      renameNote();
      return;
    }

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
    if (e.shiftKey && k === "i") {
      e.preventDefault();
      void openInbox();
      return;
    }
    if (k === "," && !e.shiftKey) {
      e.preventDefault();
      void openSettings();
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
    if (k === "n" && !e.shiftKey) {
      e.preventDefault();
      void newThing();
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
    {#if bootError}
      <p class="boot-msg">{bootError.message}</p>

      {#if bootError.needsToken}
        <!-- Not an error so much as an expected state for a server bound past
             loopback: it printed a token once at startup and this client does
             not have it. Ask for it here rather than making someone reconstruct
             a URL by hand. -->
        <form
          class="boot-token"
          onsubmit={(e) => {
            e.preventDefault();
            useToken();
          }}
        >
          <input
            bind:value={tokenEntry}
            placeholder="paste the access token"
            spellcheck="false"
            autocomplete="off"
          />
          <button type="submit" disabled={!tokenEntry.trim()}>Connect</button>
        </form>
        <p class="boot-hint data">
          The server prints it once when it starts. In Docker:
          <code>docker logs &lt;container&gt;</code>
        </p>
      {:else}
        <button class="boot-retry" onclick={() => void refresh()}>Try again</button>
      {/if}
    {:else}
      <span class="data">starting…</span>
    {/if}
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
      <VaultStatus
        status={status.status}
        name={status.vault.name}
        readOnly={status.vault.readOnly ?? false}
      />
      <div class="spacer"></div>

      <SaveStateBadge state={saveState} detail={saveDetail} />

      <nav class="views data">
        {#each [["home", "home"], ["search", "search"], ["graph", "graph"], ["inbox", "inbox"]] as [id, label] (id)}
          <button
            class:active={view === id}
            onclick={() => {
              if (id === "search") void openSearch();
              else if (id === "graph") void openGraph();
              else if (id === "inbox") void openInbox();
              else view = "home";
            }}
          >
            {label}
            {#if id === "inbox" && suggestions.length}
              <!-- Blue, because what is waiting there came from an agent. -->
              <span class="count">{suggestions.length}</span>
            {/if}
          </button>
        {/each}
      </nav>

      {#if selected && !isCanvas && view === "note"}
        <button class="chip data" class:active={editing} onclick={toggleEdit} title="⌘E">
          {editing ? "editing" : "reading"}
        </button>
      {/if}
      <button class="chip data" onclick={() => (paletteOpen = true)} title="⌘K">⌘K</button>
      <button
        class="chip gear"
        onclick={() => void openSettings()}
        title="Settings · ⌘,"
        aria-label="Settings"
      >
        <svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor"
             stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.6 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6h.09A1.65 1.65 0 0 0 10.6 3.09V3a2 2 0 1 1 4 0v.09A1.65 1.65 0 0 0 16.11 4.6a1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 20.4 9v.09a1.65 1.65 0 0 0 1.51 1H22a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </button>
      <button class="chip data" onclick={cycleTheme} title="Cycle theme">
        {theme.replace("arc-", "")}
      </button>
    </header>

    <div class="body">
      <aside class="sidebar">
        <div class="sidehead">
          <span class="data label">notes</span>
          <button class="new" onclick={() => void newThing()} title="New · ⌘N">+</button>
        </div>
        {#if tree && tree.entries.length}
          <FileTree {tree} {selected} onselect={openNote} />
        {:else if tree}
          <EmptyState
            title="This vault is empty"
            description="Make the first note, or drop a markdown file into the folder."
            actionLabel="New note"
            onaction={() => void newThing()}
            hint={status.vault.path ?? ""}
          />
        {/if}
      </aside>

      <!-- `view` as the key so the pane replays its entrance on every view
           change: a swap that just appears is what read as dead. -->
      {#key view}
        <main class="pane arc-settle">
        <!-- One banner. The Phase 1 save guard and the live-change notice are
             two mechanisms reporting the same event, and showing both stacked
             says it twice and reads as two separate problems. Whichever fires
             first owns the message; the reload affordance rides along either
             way, because it is the same recovery. -->
        {#if error || staleWarning}
          <div
            class="banner"
            class:bad={saveState === "conflict" || staleWarning !== null}
            role="alert"
          >
            <span>{error ?? staleWarning}</span>
            {#if staleWarning}
              <!-- Never automatic: reloading discards unsaved text, so it is a
                   choice someone makes, not a side effect of a notification. -->
              <button
                class="reload"
                onclick={async () => {
                  staleWarning = null;
                  error = null;
                  if (selected) await openNote(selected);
                }}>Reload and lose my edits</button>
            {/if}
            <button
              onclick={() => {
                error = null;
                staleWarning = null;
              }}
              aria-label="Dismiss">×</button>
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
        {:else if view === "inbox"}
          <Inbox
            {suggestions}
            status={weave}
            busy={weaveBusy}
            working={weaveWorking}
            onaccept={(id) => void acceptSuggestion(id)}
            ondismiss={(id) => void dismissSuggestion(id)}
            onopen={openNote}
            onpass={() => void weavePass()}
          />
        {:else if view === "graph"}
          {#if graphData}
            <Graph data={graphData} {selected} inferred={suggestions} onopen={openNote} />
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
              {runnability}
              activeRun={runningNodes}
              onrun={(id) => runNode(id)}
              onopen={openNote}
              onmove={async (moves) => {
                if (!selected) return;
                try {
                  await transport.moveCanvasNodes(selected, moves);
                  canvasView = await transport.canvas(selected);
                  runnability = await transport.runnability(selected);
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

        {#if activeRun}
          <RunPanel
            run={activeRun}
            oncancel={() => activeRun && void transport.cancelRun(activeRun.id)}
            onapprove={() => lastRunTarget && runNode(lastRunTarget, true)}
            onclose={() => (activeRun = null)}
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
      {/key}

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

  <Create
    bind:open={createOpen}
    folder={createFolder}
    initialKind={createKind}
    {templates}
    {drafting}
    {draftError}
    bind:draft
    oncreate={(k, n, t) => void create(k, n, t)}
    ondraft={(d) => void draftTemplate(d)}
    onsavetemplate={(n, b) => void saveTemplate(n, b)}
    oncancel={() => (createOpen = false)}
  />

  {#if settingsOpen && config}
    <Settings
      {config}
      version={apiVersion}
      can={(c) => transport.can(c)}
      saving={savingConfig}
      adjusted={configAdjusted}
      onsave={(c) => void saveConfig(c)}
      onclose={() => (settingsOpen = false)}
    />
  {/if}

  <Palette bind:open={paletteOpen} bind:mode={paletteMode} {commands} onopen={openNote} />

  <Ask
    bind:open={askOpen}
    bind:value={askValue}
    title={askTitle}
    detail={askDetail}
    confirmLabel={askConfirm}
    danger={askDanger}
    onconfirm={(v) => void askConfirmed(v)}
    oncancel={() => (askOpen = false)}
  />
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

  .banner .reload {
    color: var(--arc-danger);
    border: 1px solid var(--arc-danger);
    border-radius: var(--arc-radius-sm);
    padding: 0 var(--arc-space-2);
    margin-left: var(--arc-space-2);
    white-space: nowrap;
  }

  .boot-msg {
    margin: 0;
    color: var(--arc-fg);
    font-size: var(--arc-text-md);
    text-align: center;
    max-width: 46ch;
    line-height: var(--arc-leading);
  }
  .boot-token {
    display: flex;
    gap: var(--arc-space-2);
  }
  .boot-token input {
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-line-strong);
    border-radius: var(--arc-radius);
    color: var(--arc-fg);
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    padding: var(--arc-space-2) var(--arc-space-3);
    min-width: 28ch;
  }
  .boot-token input:focus {
    outline: none;
    border-color: var(--arc-accent-dim);
  }
  .boot-token button,
  .boot-retry {
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-accent-dim);
    color: var(--arc-accent);
    border-radius: var(--arc-radius);
    padding: var(--arc-space-2) var(--arc-space-4);
    font-size: var(--arc-text-sm);
    cursor: pointer;
  }
  .boot-token button:disabled {
    opacity: 0.5;
    cursor: default;
    border-color: var(--arc-line-strong);
    color: var(--arc-fg-faint);
  }
  .boot-hint {
    margin: 0;
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
  }
  .boot-hint code {
    font-family: var(--arc-font-data);
    color: var(--arc-fg-dim);
  }

  .sidehead {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--arc-space-2) var(--arc-space-3);
    border-bottom: 1px solid var(--arc-line);
  }
  .sidehead .label {
    color: var(--arc-fg-faint);
    font-size: var(--arc-text-xs);
    letter-spacing: 0.06em;
  }
  .sidehead .new {
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-lg);
    line-height: 1;
    padding: 0 var(--arc-space-2);
    border-radius: var(--arc-radius-sm);
    transition: color var(--arc-dur-fast) var(--arc-ease),
      background var(--arc-dur-fast) var(--arc-ease);
  }
  .sidehead .new:hover {
    color: var(--arc-fg);
    background: var(--arc-bg-3);
  }

  .views {
    display: flex;
    gap: 2px;
    /* A held surface rather than an outlined box — the segments read as one
       control without a rule drawing a rectangle around them. */
    background: var(--arc-bg-2);
    padding: 2px;
    border-radius: var(--arc-radius-pill);
  }
  .views button {
    padding: 3px var(--arc-space-3);
    color: var(--arc-fg-faint);
    border-radius: var(--arc-radius-pill);
    transition:
      color var(--arc-dur-fast) var(--arc-ease),
      background var(--arc-dur-fast) var(--arc-ease);
  }
  .views button:hover {
    color: var(--arc-fg-dim);
  }
  .views button.active {
    background: var(--arc-bg-3);
    color: var(--arc-fg);
  }
  /* Blue, because what is waiting in there came from an agent. The same rule as
     the timeline rail and the inbox cards: amber is what a person did. */
  .views .count {
    display: inline-block;
    margin-left: var(--arc-space-1);
    padding: 0 4px;
    border-radius: var(--arc-radius-sm);
    background: var(--arc-agent-wash);
    color: var(--arc-agent);
    font-size: var(--arc-text-xs);
  }

  .chip {
    color: var(--arc-fg-faint);
    padding: 3px var(--arc-space-3);
    background: var(--arc-bg-2);
    border: 0;
    border-radius: var(--arc-radius-pill);
    transition:
      color var(--arc-dur-fast) var(--arc-ease),
      background var(--arc-dur-fast) var(--arc-ease);
  }
  .chip:hover {
    color: var(--arc-fg-dim);
    background: var(--arc-bg-3);
  }
  .gear {
    display: inline-flex;
    align-items: center;
    padding: var(--arc-space-1) var(--arc-space-2);
  }

  .chip.active {
    color: var(--arc-accent);
    background: var(--arc-accent-wash);
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
