<script lang="ts">
  /**
   * The creation window.
   *
   * Small on purpose: a name and a type, and nothing else until you want it.
   * `Ask.svelte` stays as it is and keeps serving rename and delete — a create
   * form is a different shape from a one-line question, and folding both into
   * one component would make each worse.
   *
   * The type sets the extension, so you type a name rather than a filename.
   * Location is the folder you had selected, shown as a prefix rather than a
   * field to fill in: it is context, not a decision.
   *
   * ## Drafting
   *
   * The prompt box produces a **reusable template**, and its output appears here
   * to be read and edited before anything reaches disk. That ordering is the
   * point. Everywhere else in this app, a model's words arrive as a proposal —
   * because nobody asked for them and nobody has seen them. Here you asked, and
   * it is on screen in front of you, so what finally lands is your write with
   * the model named in the reason.
   */
  import type { CreateKind, Template } from "../lib/types";

  interface Props {
    open: boolean;
    /** The folder the new thing lands in. "" is the vault root. */
    folder: string;
    /**
     * What the window opens on. "New canvas" in the palette has to mean
     * canvas — otherwise the command is a label with nothing behind it.
     */
    initialKind?: CreateKind;
    templates: Template[];
    /** True while a model is generating. */
    drafting?: boolean;
    /** Set when drafting failed, in the model's own terms. */
    draftError?: string | null;
    /** The drafted template, once there is one to read. */
    draft?: string | null;
    oncreate: (kind: CreateKind, name: string, template: string | null) => void;
    ondraft: (description: string) => void;
    onsavetemplate: (name: string, body: string) => void;
    oncancel: () => void;
  }

  let {
    open = $bindable(),
    folder,
    initialKind = "note",
    templates,
    drafting = false,
    draftError = null,
    draft = $bindable(),
    oncreate,
    ondraft,
    onsavetemplate,
    oncancel,
  }: Props = $props();

  let kind = $state<CreateKind>("note");
  let name = $state("");
  let template = $state<string>("");
  let aiOpen = $state(false);
  let description = $state("");
  let templateName = $state("");
  let field = $state<HTMLInputElement | null>(null);

  const KINDS: Array<[CreateKind, string, string]> = [
    ["note", "Note", ".md"],
    ["canvas", "Canvas", ".canvas"],
    ["folder", "Folder", ""],
  ];

  const extension = $derived(KINDS.find(([k]) => k === kind)?.[2] ?? "");

  /**
   * Exactly the three `substitute()` knows, spelled exactly as it spells them.
   *
   * That function matches literally — no trimming, no case folding — so any
   * cleverness here would report a stray that does get filled in, or miss one
   * that does not.
   */
  const KNOWN = new Set(["{{title}}", "{{date}}", "{{time}}"]);

  /**
   * Placeholders in the draft that nothing will ever fill in.
   *
   * Small models invent these constantly despite being told not to —
   * `{{decision_1}}`, `{{attendees}}` — and `substitute()` deliberately leaves
   * an unknown placeholder alone rather than silently deleting part of
   * someone's template. Both of those are right, and together they mean a
   * drafted template can quietly put literal `{{decision_1}}` into every note
   * made from it. So: name them. Not deleted, not hidden — the preview is
   * where a person decides, and they cannot decide about something that looks
   * exactly as legitimate as `{{title}}`.
   */
  const strays = $derived(
    [...new Set(draft?.match(/\{\{[^{}]*\}\}/g) ?? [])].filter((p) => !KNOWN.has(p)),
  );
  const prefix = $derived(folder ? `${folder}/` : "");
  const canCreate = $derived(name.trim().length > 0);

  $effect(() => {
    if (!open) return;
    // The opener decides the type, and it is reset on every open rather than
    // remembered: the pill you last used is not what you want next time.
    kind = initialKind;
    // A macrotask rather than a frame: an occluded window starves rAF, and the
    // field has to be focused whether or not the compositor is running. Same
    // reason as Palette.svelte.
    setTimeout(() => field?.select(), 0);
  });

  function reset() {
    name = "";
    template = "";
    aiOpen = false;
    description = "";
    templateName = "";
    draft = null;
  }

  function submit() {
    if (!canCreate) return;
    oncreate(kind, name.trim(), kind === "note" && template ? template : null);
    reset();
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      oncancel();
      reset();
    }
  }
</script>

<svelte:window onkeydown={onKey} />

{#if open}
  <div
    class="scrim arc-fade"
    role="presentation"
    onclick={(e) => {
      if (e.target === e.currentTarget) {
        oncancel();
        reset();
      }
    }}
  >
    <div class="win arc-rise" role="dialog" aria-modal="true" aria-label="Create">
      <h2>New</h2>

      <!-- Type first: it decides what the name means. -->
      <div class="pills" role="group" aria-label="What to create">
        {#each KINDS as [k, label] (k)}
          <button class="pill" class:on={kind === k} onclick={() => (kind = k)}>{label}</button>
        {/each}
      </div>

      <label class="field">
        <span class="lab">Name</span>
        <span class="input">
          {#if prefix}<span class="data prefix" title="The folder it lands in">{prefix}</span>{/if}
          <input
            bind:this={field}
            bind:value={name}
            placeholder={kind === "folder" ? "Projects" : "Untitled"}
            spellcheck="false"
            onkeydown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                submit();
              }
            }}
          />
          {#if extension}<span class="data ext">{extension}</span>{/if}
        </span>
      </label>

      {#if kind === "note" && templates.length > 0}
        <label class="field">
          <span class="lab">From</span>
          <select bind:value={template}>
            <option value="">Blank</option>
            {#each templates as t (t.path)}
              <option value={t.path}>{t.name}</option>
            {/each}
          </select>
        </label>
      {/if}

      {#if kind === "note"}
        <!-- Collapsed behind one line: an accelerator, not a step. -->
        {#if !aiOpen}
          <button class="ailink" onclick={() => (aiOpen = true)}>
            Let the AI create a template for you
          </button>
        {:else}
          <div class="ai">
            <label class="field">
              <span class="lab">Describe it</span>
              <textarea
                bind:value={description}
                rows="2"
                placeholder="a weekly review with wins, blockers and next week"
                spellcheck="false"
              ></textarea>
            </label>

            <div class="airow">
              <button
                class="draft"
                onclick={() => ondraft(description)}
                disabled={drafting || description.trim().length === 0}
              >
                {drafting ? "drafting…" : "Draft"}
              </button>
              <span class="hint">
                Your description is sent to the configured model. No note content goes with it.
              </span>
            </div>

            {#if draftError}
              <p class="err">{draftError}</p>
            {/if}

            {#if draft}
              <!-- Editable, and nothing is written until Save as template. -->
              <label class="field">
                <span class="lab">Draft</span>
                <textarea class="preview data" bind:value={draft} rows="10" spellcheck="false"
                ></textarea>
              </label>
              {#if strays.length}
                <p class="stray">
                  Nothing fills in {strays.join(", ")}. Only {"{{title}}"}, {"{{date}}"} and
                  {"{{time}}"} are substituted — the rest arrive as text in every note made
                  from this template.
                </p>
              {/if}
              <div class="airow">
                <input
                  class="tname"
                  bind:value={templateName}
                  placeholder="template name"
                  spellcheck="false"
                />
                <button
                  class="draft"
                  disabled={templateName.trim().length === 0}
                  onclick={() => {
                    onsavetemplate(templateName.trim(), draft ?? "");
                    aiOpen = false;
                    draft = null;
                    templateName = "";
                  }}>Save as template</button>
              </div>
            {/if}
          </div>
        {/if}
      {/if}

      <div class="actions">
        <button class="go" onclick={submit} disabled={!canCreate}>Create</button>
        <button
          class="cancel"
          onclick={() => {
            oncancel();
            reset();
          }}>Cancel</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: var(--arc-z-modal);
    background: var(--arc-scrim);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding: 12vh var(--arc-space-5) var(--arc-space-5);
    overflow-y: auto;
  }
  .win {
    width: min(520px, 100%);
    background: var(--arc-bg-1);
    border-radius: var(--arc-radius-lg);
    padding: var(--arc-space-5);
    box-shadow: 0 24px 64px var(--arc-scrim);
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-4);
  }
  h2 {
    margin: 0;
    font-size: var(--arc-text-lg);
    font-weight: 500;
  }

  .pills {
    display: flex;
    gap: var(--arc-space-2);
  }
  .pill {
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    padding: var(--arc-space-2) var(--arc-space-4);
    border: 0;
    border-radius: var(--arc-radius-pill);
    background: var(--arc-bg-2);
    color: var(--arc-fg-faint);
    cursor: pointer;
    transition:
      background var(--arc-dur) var(--arc-ease),
      color var(--arc-dur) var(--arc-ease);
  }
  .pill:hover {
    background: var(--arc-bg-3);
    color: var(--arc-fg-dim);
  }
  .pill.on {
    background: var(--arc-accent-wash);
    color: var(--arc-accent);
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-2);
  }
  .lab {
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .input {
    display: flex;
    align-items: center;
    gap: var(--arc-space-1);
    background: var(--arc-bg-2);
    border-radius: var(--arc-radius-sm);
    padding: 0 var(--arc-space-3);
  }
  .prefix,
  .ext {
    font-size: var(--arc-text-sm);
    color: var(--arc-fg-faint);
    flex: none;
  }
  .input input {
    flex: 1;
    min-width: 0;
    background: none;
    border: 0;
    color: var(--arc-fg);
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    padding: var(--arc-space-2) 0;
  }
  .input input:focus {
    outline: none;
  }

  select,
  textarea,
  .tname {
    background: var(--arc-bg-2);
    border: 0;
    border-radius: var(--arc-radius-sm);
    color: var(--arc-fg);
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-sm);
    padding: var(--arc-space-2) var(--arc-space-3);
    resize: vertical;
  }
  select:focus,
  textarea:focus,
  .tname:focus {
    outline: none;
  }
  .preview {
    line-height: var(--arc-leading);
  }

  .ailink {
    align-self: flex-start;
    background: none;
    border: 0;
    padding: 0;
    color: var(--arc-accent);
    font-size: var(--arc-text-sm);
    cursor: pointer;
    transition: color var(--arc-dur-fast) var(--arc-ease);
  }
  .ailink:hover {
    color: var(--arc-fg);
  }
  .ai {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-3);
    background: var(--arc-bg-0);
    border-radius: var(--arc-radius);
    padding: var(--arc-space-4);
  }
  .airow {
    display: flex;
    align-items: center;
    gap: var(--arc-space-3);
  }
  .tname {
    flex: 1;
    min-width: 0;
  }
  .hint {
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-faint);
    line-height: var(--arc-leading);
  }
  .err {
    margin: 0;
    font-size: var(--arc-text-xs);
    color: var(--arc-danger);
    line-height: var(--arc-leading);
  }
  /* Not an error — the draft is usable, and this is something to notice. */
  .stray {
    margin: 0;
    font-size: var(--arc-text-xs);
    color: var(--arc-fg-dim);
    line-height: var(--arc-leading);
  }

  .actions {
    display: flex;
    gap: var(--arc-space-2);
    justify-content: flex-end;
  }
  .go,
  .cancel,
  .draft {
    border: 0;
    border-radius: var(--arc-radius-pill);
    padding: var(--arc-space-2) var(--arc-space-4);
    font-size: var(--arc-text-sm);
    cursor: pointer;
    background: var(--arc-bg-2);
    color: var(--arc-fg);
    transition: background var(--arc-dur) var(--arc-ease);
  }
  .go,
  .draft {
    background: var(--arc-accent-wash);
    color: var(--arc-accent);
  }
  .go:hover,
  .draft:hover:not(:disabled) {
    background: var(--arc-accent-dim);
    color: var(--arc-fg);
  }
  .cancel:hover {
    background: var(--arc-bg-3);
  }
  .go:disabled,
  .draft:disabled {
    opacity: 0.5;
    cursor: default;
    background: var(--arc-bg-2);
    color: var(--arc-fg-faint);
  }
</style>
