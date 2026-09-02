<script lang="ts">
  /**
   * The command palette (⌘K) and quick open (⌘P).
   *
   * One component for both, because they are the same surface with a different
   * starting mode — and because the Phase 2 gate is that **every core action is
   * reachable in three keystrokes or fewer without touching the mouse.** Two
   * separate widgets would double the keystroke budget for anything that lives
   * in the wrong one.
   *
   * ## Opening in under 50 ms
   *
   * The palette is **always mounted** and toggled with `hidden`, rather than
   * created on demand. Mounting a component, laying it out and focusing an input
   * costs more than the budget on a cold frame. Kept alive, opening is a class
   * change and a `focus()`.
   *
   * Results are fetched *after* the surface is up, never before, so a slow query
   * can never delay the open. An empty palette that fills in 20 ms feels
   * instant; a palette that appears 80 ms late feels broken.
   */
  import type { NoteRef } from "../lib/types";
  import { transport } from "../lib/transport";

  export type Mode = "commands" | "files";

  export interface Command {
    id: string;
    label: string;
    /** Shown right-aligned, in the data face. */
    hint?: string;
    run: () => void;
  }

  let {
    open = $bindable(false),
    mode = $bindable<Mode>("commands"),
    commands,
    onopen,
  }: {
    open: boolean;
    mode: Mode;
    commands: Command[];
    onopen: (path: string) => void;
  } = $props();

  let query = $state("");
  let selected = $state(0);
  let input = $state<HTMLInputElement | null>(null);
  let files = $state<NoteRef[]>([]);
  let seq = 0;

  /** Commands filtered by a simple subsequence match — no fuzzy ranking. */
  let matchedCommands = $derived.by(() => {
    const q = query.trim().toLowerCase();
    if (!q) return commands;
    return commands.filter((c) => c.label.toLowerCase().includes(q));
  });

  let rows = $derived(
    mode === "files"
      ? files.map((f) => ({ key: f.path, label: f.title, hint: f.path, isFile: true }))
      : matchedCommands.map((c) => ({ key: c.id, label: c.label, hint: c.hint, isFile: false })),
  );

  // File results come from the index. Each request carries a sequence number so
  // a slow response for an old query cannot overwrite a fast one for a new
  // query — the classic search-as-you-type bug.
  $effect(() => {
    if (!open || mode !== "files") return;
    const q = query;
    const mine = ++seq;
    void (async () => {
      try {
        const r = q.trim() ? await transport.quickOpen(q, 50) : await transport.recent(20);
        if (mine === seq) files = r;
      } catch {
        if (mine === seq) files = [];
      }
    })();
  });

  $effect(() => {
    if (open) {
      query = "";
      selected = 0;
      // A macrotask, not a frame: the element is display:none until this tick,
      // but requestAnimationFrame never fires in an occluded or background
      // window, and a palette that silently refuses to take typing is worse
      // than one that focuses a frame early.
      setTimeout(() => input?.focus(), 0);
    }
  });

  // Keep the selection in range as results change under it.
  $effect(() => {
    if (selected >= rows.length) selected = Math.max(0, rows.length - 1);
  });

  function choose(i: number) {
    const row = rows[i];
    if (!row) return;
    open = false;
    if (row.isFile) onopen(row.key);
    else commands.find((c) => c.id === row.key)?.run();
  }

  function onKey(e: KeyboardEvent) {
    switch (e.key) {
      case "Escape":
        e.preventDefault();
        open = false;
        break;
      case "ArrowDown":
        e.preventDefault();
        selected = rows.length ? (selected + 1) % rows.length : 0;
        break;
      case "ArrowUp":
        e.preventDefault();
        selected = rows.length ? (selected - 1 + rows.length) % rows.length : 0;
        break;
      case "Enter":
        e.preventDefault();
        choose(selected);
        break;
      case "Tab":
        // Switch between commands and files without reopening. Keeps anything
        // in either mode within the three-keystroke budget.
        e.preventDefault();
        mode = mode === "files" ? "commands" : "files";
        selected = 0;
        break;
    }
  }

  function scrollIntoView(el: HTMLElement, isSelected: boolean) {
    if (isSelected) el.scrollIntoView({ block: "nearest" });
    return {
      update(nowSelected: boolean) {
        if (nowSelected) el.scrollIntoView({ block: "nearest" });
      },
    };
  }
</script>

<!-- Always mounted, only hidden. See the note about the 50 ms budget. -->
<div class="scrim" hidden={!open} onmousedown={() => (open = false)} role="presentation">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="palette" onmousedown={(e) => e.stopPropagation()}>
    <div class="field">
      <span class="data mode">{mode === "files" ? "open" : "cmd"}</span>
      <input
        bind:this={input}
        bind:value={query}
        onkeydown={onKey}
        placeholder={mode === "files" ? "Find a note…" : "Type a command…"}
        spellcheck="false"
        autocomplete="off"
      />
      <span class="data tabhint">tab to switch</span>
    </div>

    <ul class="results" role="listbox" aria-label="Results">
      {#each rows as row, i (row.key)}
        <li>
          <button
            class="row"
            class:selected={i === selected}
            use:scrollIntoView={i === selected}
            onmouseenter={() => (selected = i)}
            onclick={() => choose(i)}
            role="option"
            aria-selected={i === selected}
          >
            <span class="label">{row.label}</span>
            {#if row.hint}<span class="data hint" class:path={row.isFile}>{row.hint}</span>{/if}
          </button>
        </li>
      {:else}
        <li class="none data">
          {mode === "files" ? "no note matches" : "no command matches"}
        </li>
      {/each}
    </ul>
  </div>
</div>

<style>
  /* `display: flex` on a class beats the user-agent's `[hidden] {display:none}`,
     which left the palette permanently open. Class+attribute wins it back. */
  .scrim[hidden] {
    display: none;
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: var(--arc-z-modal);
    display: flex;
    justify-content: center;
    padding-top: 14vh;
    background: var(--arc-scrim);
  }

  .palette {
    width: min(620px, calc(100vw - var(--arc-space-6)));
    max-height: 60vh;
    display: flex;
    flex-direction: column;
    background: var(--arc-bg-1);
    border: 1px solid var(--arc-line-strong);
    border-radius: var(--arc-radius-lg);
    overflow: hidden;
    /* No shadow: the direction is a precision instrument, and a drop shadow
       here would be the one piece of decoration in the whole product. The
       border and the dimmed scrim already separate it. */
  }

  .field {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
    padding: var(--arc-space-3) var(--arc-space-4);
    border-bottom: 1px solid var(--arc-line);
  }
  .field input {
    flex: 1;
    min-width: 0;
    background: none;
    border: none;
    outline: none;
    font-size: var(--arc-text-lg);
    color: var(--arc-fg);
  }
  .field input::placeholder {
    color: var(--arc-fg-faint);
  }

  .mode {
    flex: none;
    color: var(--arc-accent);
    border: 1px solid var(--arc-accent-dim);
    border-radius: var(--arc-radius-sm);
    padding: 1px var(--arc-space-2);
    letter-spacing: 0.06em;
  }
  .tabhint {
    flex: none;
    color: var(--arc-fg-faint);
  }

  .results {
    list-style: none;
    margin: 0;
    padding: var(--arc-space-1);
    overflow-y: auto;
  }

  .row {
    width: 100%;
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--arc-space-4);
    padding: var(--arc-space-2) var(--arc-space-3);
    border-radius: var(--arc-radius-sm);
    text-align: left;
    color: var(--arc-fg-dim);
  }
  .row.selected {
    background: var(--arc-bg-3);
    color: var(--arc-fg);
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hint {
    flex: none;
    max-width: 45%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--arc-fg-faint);
  }
  /* Only paths get RTL truncation, which keeps the filename visible when the
     folders are too long. Applying it to a keyboard hint reversed it: the shortcut
     for Search rendered as "F⇧⌘". */
  .hint.path {
    direction: rtl;
    text-align: right;
  }

  .none {
    padding: var(--arc-space-4);
    color: var(--arc-fg-faint);
    text-align: center;
  }
</style>
