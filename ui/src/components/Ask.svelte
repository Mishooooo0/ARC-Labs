<script lang="ts">
  /**
   * The one dialog in the app: name something, or confirm something.
   *
   * Native `confirm()` and `prompt()` are not used anywhere in ARC-LABS, and
   * this is why — they cannot be themed, they cannot be tested, they block the
   * whole webview, and they look like a browser rather than like the app. This
   * costs a hundred lines and behaves.
   *
   * Deliberately not a general modal system. It answers exactly two questions,
   * and a third kind of dialog should be a third component rather than a fourth
   * mode in this one.
   */

  interface Props {
    open: boolean;
    /** The question. One line. */
    title: string;
    /** Optional second line, for consequences worth spelling out. */
    detail?: string;
    /** Present for a naming prompt; absent for a plain confirm. */
    value?: string;
    placeholder?: string;
    /** The affirmative button. Say what will happen, not "OK". */
    confirmLabel?: string;
    /** Danger styling for a destructive answer. */
    danger?: boolean;
    onconfirm: (value: string) => void;
    oncancel: () => void;
  }

  let {
    open = $bindable(),
    title,
    detail = undefined,
    value = $bindable(undefined),
    placeholder = "",
    confirmLabel = "OK",
    danger = false,
    onconfirm,
    oncancel,
  }: Props = $props();

  let input = $state<HTMLInputElement | null>(null);
  const isPrompt = $derived(value !== undefined);

  $effect(() => {
    if (!open || !input) return;
    // A macrotask, not a frame: rAF is starved in an occluded webview, and a
    // dialog that opens without focus is a dialog you have to click first.
    const t = setTimeout(() => {
      input?.focus();
      // Select the stem, not the extension — renaming "Note.md" should not make
      // you retype ".md" or carefully avoid deleting it.
      const dot = (value ?? "").lastIndexOf(".");
      input?.setSelectionRange(0, dot > 0 ? dot : (value ?? "").length);
    }, 0);
    return () => clearTimeout(t);
  });

  function submit(e?: Event) {
    e?.preventDefault();
    const answer = (value ?? "").trim();
    if (isPrompt && !answer) return;
    onconfirm(answer);
  }

  // Escape only. Enter is the form's job — see the note on the <form> below.
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      oncancel();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="scrim" hidden={!open} onmousedown={oncancel} role="presentation">
  <!--
    A real <form>, not a div with a keydown handler.

    The handler version worked when a test dispatched a synthetic Enter on the
    input and did nothing for a real keypress, which is the worst kind of bug:
    green in automation, broken for the person typing. A form submits on Enter
    natively, in every browser and every webview, with no bubbling to rely on.
  -->
  <div
    class="ask"
    onmousedown={(e) => e.stopPropagation()}
    onkeydown={onKey}
    role="dialog"
    tabindex="-1"
    aria-modal="true"
    aria-label={title}
  >
   <form onsubmit={submit}>
    <h2>{title}</h2>
    {#if detail}<p class="detail">{detail}</p>{/if}

    {#if isPrompt}
      <input
        bind:this={input}
        bind:value
        {placeholder}
        spellcheck="false"
        autocomplete="off"
      />
    {/if}

    <div class="row">
      <button type="button" class="cancel" onclick={oncancel}>Cancel</button>
      <button
        type="submit"
        class="go"
        class:danger
        disabled={isPrompt && !(value ?? "").trim()}
      >
        {confirmLabel}
      </button>
      </div>
    </form>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: var(--arc-z-modal);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 20vh;
    background: var(--arc-scrim);
  }
  /* display:flex above beats [hidden]'s display:none without this. */
  .scrim[hidden] {
    display: none !important;
  }

  .ask form {
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-3);
  }

  .ask {
    width: min(460px, calc(100vw - var(--arc-space-6)));
    background: var(--arc-bg-1);
    border: 1px solid var(--arc-line-strong);
    border-radius: var(--arc-radius-lg);
    padding: var(--arc-space-4);
    display: flex;
    flex-direction: column;
    gap: var(--arc-space-3);
  }

  h2 {
    margin: 0;
    font-size: var(--arc-text-lg);
    font-weight: 500;
  }
  .detail {
    margin: 0;
    color: var(--arc-fg-dim);
    font-size: var(--arc-text-sm);
    line-height: var(--arc-leading);
  }

  input {
    width: 100%;
    box-sizing: border-box;
    background: var(--arc-bg-2);
    border: 1px solid var(--arc-line-strong);
    border-radius: var(--arc-radius);
    color: var(--arc-fg);
    font-family: var(--arc-font-data);
    font-size: var(--arc-text-md);
    padding: var(--arc-space-2) var(--arc-space-3);
  }
  input:focus {
    outline: none;
    border-color: var(--arc-accent-dim);
  }

  .row {
    display: flex;
    justify-content: flex-end;
    gap: var(--arc-space-2);
  }
  .row button {
    border-radius: var(--arc-radius);
    padding: var(--arc-space-2) var(--arc-space-4);
    font-size: var(--arc-text-sm);
    font-family: var(--arc-font-ui);
    cursor: pointer;
    border: 1px solid var(--arc-line-strong);
    background: var(--arc-bg-2);
    color: var(--arc-fg);
    transition: background var(--arc-dur-fast) var(--arc-ease);
  }
  .row button:hover:not(:disabled) {
    background: var(--arc-bg-3);
  }
  .row .go {
    border-color: var(--arc-accent-dim);
    color: var(--arc-accent);
  }
  .row .go.danger {
    border-color: var(--arc-danger);
    color: var(--arc-danger);
  }
  .row button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
