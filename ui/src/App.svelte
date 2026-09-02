<script lang="ts">
  /**
   * The application shell.
   *
   * Knows nothing about Tauri or HTTP — it talks to `transport`, which decided
   * once at load which shell it is in. That is the whole reason this file is the
   * same on Windows, on Linux, in a browser and in Docker.
   */
  import { transport } from "./lib/transport";
  import type { NoteView as Note, Status, TreeView } from "./lib/types";
  import { TransportError } from "./lib/types";
  import ArcMark from "./components/ArcMark.svelte";
  import Editor, { type SaveState } from "./components/Editor.svelte";
  import EmptyState from "./components/EmptyState.svelte";
  import FileTree from "./components/FileTree.svelte";
  import FirstRun from "./components/FirstRun.svelte";
  import NoteViewer from "./components/NoteView.svelte";
  import SaveStateBadge from "./components/SaveState.svelte";
  import VaultStatus from "./components/VaultStatus.svelte";

  const THEMES = ["arc-dark", "arc-light", "arc-terminal"] as const;

  let status = $state<Status | null>(null);
  let tree = $state<TreeView | null>(null);
  let note = $state<Note | null>(null);
  let selected = $state<string | null>(null);
  let error = $state<string | null>(null);
  let theme = $state(localStorage.getItem("arc-labs-theme") ?? "arc-dark");

  /** Read mode renders HTML; edit mode hands the source to CodeMirror. */
  let editing = $state(false);
  let saveState = $state<SaveState>("clean");
  let saveDetail = $state<string | undefined>(undefined);
  let editor = $state<ReturnType<typeof Editor> | null>(null);

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
      if (status.vault) {
        status = { ...status, status: "scanning" };
        tree = await transport.tree();
        status = { ...status, status: "online" };
      } else {
        tree = null;
      }
    } catch (e) {
      error = message(e);
    }
  }

  let isCanvas = $derived(selected?.toLowerCase().endsWith(".canvas") ?? false);

  async function openNote(path: string) {
    // Drain any pending save before the editor unmounts, or the last 400 ms of
    // typing in the previous note is lost on every click in the tree.
    await editor?.flush();

    selected = path;
    error = null;
    note = null;
    editing = false;
    saveState = "clean";
    saveDetail = undefined;

    if (path.toLowerCase().endsWith(".canvas")) return;

    try {
      note = await transport.note(path);
    } catch (e) {
      error = message(e);
    }
  }

  async function toggleEdit() {
    if (!selected || isCanvas) return;
    error = null;

    if (editing) {
      await editor?.flush();
      editing = false;
      // Re-render from disk so read mode shows what was actually saved.
      try {
        note = await transport.note(selected);
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

  function navigate(target: string, kind: "note" | "tag" | "embed") {
    if (kind === "tag") {
      error = `Searching by tag arrives in Phase 2 — #${target}`;
      return;
    }
    const wanted = target.toLowerCase();
    const hit = tree?.entries.find((e) => {
      if (e.is_dir) return false;
      const p = e.path.toLowerCase();
      const stem = p.replace(/\.[^./]+$/, "");
      return stem === wanted || p === wanted || e.name.toLowerCase().replace(/\.[^./]+$/, "") === wanted;
    });
    if (hit) void openNote(hit.path);
    else error = `No note named "${target}" in this vault.`;
  }

  function cycleTheme() {
    const i = THEMES.indexOf(theme as (typeof THEMES)[number]);
    theme = THEMES[(i + 1) % THEMES.length] ?? "arc-dark";
  }

  function onKey(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key === "e") {
      e.preventDefault();
      void toggleEdit();
    }
    // Saving is continuous, but the reflex is universal — honour it rather than
    // letting the browser open a Save Page dialog over the top of the notebook.
    if (mod && e.key === "s") {
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
      <div class="brand">
        <ArcMark size={14} />
        <span class="data wordmark">ARC-LABS</span>
      </div>
      <VaultStatus status={status.status} name={status.vault.name} />
      <div class="spacer"></div>
      <SaveStateBadge state={saveState} detail={saveDetail} />
      {#if selected && !isCanvas}
        <button class="mode data" class:active={editing} onclick={toggleEdit} title="⌘E">
          {editing ? "editing" : "reading"}
        </button>
      {/if}
      <span class="data counts">
        {status.vault.noteCount.toLocaleString()} notes
        {#if status.vault.canvasCount}· {status.vault.canvasCount} canvases{/if}
      </span>
      <button class="theme data" onclick={cycleTheme} title="Cycle theme">
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

        {#if isCanvas}
          <EmptyState
            title={selected?.split("/").pop() ?? "Canvas"}
            description="This is a canvas — a spatial board of cards and connections, stored as JSONCanvas. Canvases open in Phase 4; the file is untouched on disk."
            hint={selected ?? ""}
          />
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
              }}
              onerror={(m) => (error = m)}
            />
          {/key}
        {:else if note}
          <NoteViewer {note} onnavigate={navigate} />
        {:else if tree && tree.note_count === 0}
          <EmptyState
            title="No notes yet"
            description="ARC-LABS reads plain markdown files. Create one in this folder — or open a different vault."
            hint={status.vault.path ?? ""}
          />
        {:else}
          <EmptyState
            title="Nothing open"
            description="Choose a note from the left to read it. Everything stays plain markdown on disk."
            hint="{tree?.note_count ?? 0} notes · ⌘E to edit"
          />
        {/if}
      </main>
    </div>
  </div>
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
  .wordmark {
    letter-spacing: 0.1em;
  }

  .spacer {
    flex: 1;
  }

  .counts {
    color: var(--arc-fg-faint);
  }

  .mode,
  .theme {
    color: var(--arc-fg-faint);
    padding: 2px var(--arc-space-2);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius-sm);
    transition:
      color var(--arc-dur-fast) var(--arc-ease),
      border-color var(--arc-dur-fast) var(--arc-ease);
  }
  .mode:hover,
  .theme:hover {
    color: var(--arc-fg-dim);
    border-color: var(--arc-line-strong);
  }
  .mode.active {
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
