<script lang="ts">
  /**
   * The vault file tree.
   *
   * The API returns a flat list with parent indices rather than a nested
   * structure, and this component keeps it that way. It renders only the rows
   * that are actually visible — a collapsed folder's children cost nothing, and
   * a 5,000-note vault is a filter over one array rather than a few thousand
   * component instances.
   *
   * Folders start collapsed except along the path to the open note, so opening a
   * vault shows its shape rather than its contents.
   */
  import type { TreeEntry, TreeView } from "../lib/types";

  let {
    tree,
    selected,
    onselect,
  }: {
    tree: TreeView;
    selected: string | null;
    onselect: (path: string) => void;
  } = $props();

  let expanded = $state<Set<string>>(new Set());

  function toggle(path: string) {
    // Reassigned rather than mutated: a Set mutated in place is not a signal.
    const next = new Set(expanded);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    expanded = next;
  }

  /** Reveal the folders containing the selected note. */
  $effect(() => {
    if (!selected) return;
    const parts = selected.split("/");
    if (parts.length < 2) return;
    const next = new Set(expanded);
    let acc = "";
    for (const part of parts.slice(0, -1)) {
      acc = acc ? `${acc}/${part}` : part;
      next.add(acc);
    }
    if (next.size !== expanded.size) expanded = next;
  });

  interface Row {
    entry: TreeEntry;
    depth: number;
  }

  /**
   * Visible rows, in order. An entry is visible when every ancestor folder is
   * expanded; depth comes from walking parent indices, which the API guarantees
   * always point backwards to a real directory.
   */
  let rows = $derived.by((): Row[] => {
    const depths = new Map<number, number>();
    const out: Row[] = [];

    tree.entries.forEach((entry, i) => {
      const depth = entry.parent === null ? 0 : (depths.get(entry.parent) ?? 0) + 1;
      depths.set(i, depth);

      let visible = true;
      let p = entry.parent;
      while (p !== null && p !== undefined) {
        const parent = tree.entries[p];
        if (!parent) break;
        if (!expanded.has(parent.path)) {
          visible = false;
          break;
        }
        p = parent.parent;
      }
      if (visible) out.push({ entry, depth });
    });
    return out;
  });

  function activate(entry: TreeEntry) {
    if (entry.is_dir) toggle(entry.path);
    else onselect(entry.path);
  }
</script>

<nav class="tree" aria-label="Vault files">
  <!-- Rows settle rather than snapping in. Capped at eight by `.arc-stagger`,
       so a 5,000-note vault animates the first handful and nothing else. -->
  <ul class="arc-stagger">
    {#each rows as { entry, depth } (entry.path)}
      <li>
        <button
          class="row"
          class:dir={entry.is_dir}
          class:selected={entry.path === selected}
          style="padding-left: calc(var(--arc-space-3) + {depth} * var(--arc-space-4))"
          onclick={() => activate(entry)}
          aria-expanded={entry.is_dir ? expanded.has(entry.path) : undefined}
          title={entry.path}
        >
          {#if entry.is_dir}
            <span class="chev" class:open={expanded.has(entry.path)}>›</span>
          {:else}
            <span class="chev spacer"></span>
          {/if}
          <span class="name">{entry.name}</span>
        </button>
      </li>
    {/each}
  </ul>

  {#if tree.skipped.length > 0}
    <!-- Named rather than hidden: a vault that is only partly visible has to say
         so, or the user silently loses notes. -->
    <details class="skipped">
      <summary class="data">{tree.skipped.length} item(s) could not be shown</summary>
      <ul>
        {#each tree.skipped as s (s.path)}
          <li class="data"><span class="sp">{s.path}</span> — {s.reason}</li>
        {/each}
      </ul>
    </details>
  {/if}
</nav>

<style>
  .tree {
    height: 100%;
    overflow-y: auto;
    padding: var(--arc-space-2) 0;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: var(--arc-space-2);
    padding-right: var(--arc-space-3);
    padding-top: 3px;
    padding-bottom: 3px;
    text-align: left;
    color: var(--arc-fg-dim);
    border-radius: var(--arc-radius-sm);
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .row:hover {
    background: var(--arc-bg-2);
    color: var(--arc-fg);
  }
  .row.selected {
    background: var(--arc-bg-3);
    color: var(--arc-fg);
  }
  .row.dir {
    color: var(--arc-fg);
  }

  .chev {
    flex: none;
    width: 10px;
    color: var(--arc-fg-faint);
    transition: transform var(--arc-dur-fast) var(--arc-ease);
    display: inline-block;
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .chev.spacer {
    visibility: hidden;
  }

  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .skipped {
    margin: var(--arc-space-3);
    padding: var(--arc-space-2);
    border: 1px solid var(--arc-line);
    border-radius: var(--arc-radius-sm);
    color: var(--arc-warn);
  }
  .skipped summary {
    cursor: pointer;
    color: var(--arc-warn);
  }
  .skipped ul {
    margin-top: var(--arc-space-2);
    color: var(--arc-fg-faint);
  }
  .sp {
    color: var(--arc-fg-dim);
  }
</style>
