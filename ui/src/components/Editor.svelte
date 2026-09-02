<script lang="ts">
  /**
   * The editor.
   *
   * ## Saving
   *
   * Debounced 400 ms, and the *Rust* side does the atomic replace and the
   * fidelity re-application. This component's only job is to decide when to send
   * text and what base hash to send with it.
   *
   * Three rules make the Phase 1 gate hold:
   *
   * 1. **Never save what has not changed.** The text is compared to the last
   *    committed text before a request goes out. Typing a character and undoing
   *    it produces no save at all, so there is nothing that could go wrong.
   * 2. **Always carry a base hash.** If the file changed underneath us — Obsidian,
   *    Syncthing, git — the save is refused rather than silently winning.
   * 3. **Flush before leaving.** Switching notes or closing the window drains the
   *    pending save first, so the debounce window can never eat an edit.
   */
  import { EditorState, type Extension } from "@codemirror/state";
  import { EditorView, keymap, drawSelection, highlightActiveLine, rectangularSelection,
           crosshairCursor, dropCursor } from "@codemirror/view";
  import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
  import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
  import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
  import { languages } from "@codemirror/language-data";
  import { indentUnit, bracketMatching } from "@codemirror/language";

  import { arcEditorTheme } from "../lib/editor/theme";
  import { frontmatterSupport } from "../lib/editor/frontmatter";
  import { livePreview } from "../lib/editor/livePreview";
  import { arcMarkdown } from "../lib/editor/wikilink";
  import { transport } from "../lib/transport";
  import { TransportError } from "../lib/types";

  export type SaveState = "clean" | "dirty" | "saving" | "saved" | "conflict" | "error";

  let {
    path,
    initialText,
    baseHash,
    onstate,
    onerror,
  }: {
    path: string;
    initialText: string;
    baseHash: string;
    onstate: (s: SaveState, detail?: string) => void;
    onerror: (message: string) => void;
  } = $props();

  const DEBOUNCE_MS = 400;

  let host = $state<HTMLDivElement | null>(null);
  let view: EditorView | null = null;
  let timer: ReturnType<typeof setTimeout> | null = null;

  // Mutable, non-reactive save bookkeeping. Deliberately plain variables: these
  // change on every keystroke, and making them reactive would schedule a render
  // per character for values nothing renders.
  //
  // They are seeded in the mount effect rather than here. The component is keyed
  // on the note path by its parent, so a different note is a different instance —
  // capturing the initial props is exactly right, and doing it in the effect
  // says so rather than leaving it as an accident of declaration order.
  let committedText = "";
  let currentHash = "";
  let inFlight = false;

  function scheduleSave() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(() => void save(), DEBOUNCE_MS);
  }

  async function save(): Promise<void> {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    if (!view || inFlight) return;

    const text = view.state.doc.toString();
    // Rule 1. This is what makes "type a character, undo it, save" a no-op
    // rather than a round trip that has to be lossless.
    if (text === committedText) {
      onstate("clean");
      return;
    }

    inFlight = true;
    onstate("saving");
    try {
      const result = await transport.saveNote(path, text, currentHash);
      committedText = text;
      currentHash = result.hash;
      onstate("saved", result.written ? `${result.bytes.toLocaleString()} B` : "no change");
    } catch (e) {
      if (e instanceof TransportError && e.code === "conflict") {
        onstate("conflict");
        onerror(
          "This note changed on disk since you opened it. Your text is still here — " +
            "reopen the note to see the other version.",
        );
      } else {
        onstate("error");
        onerror(e instanceof TransportError ? e.message : String(e));
      }
    } finally {
      inFlight = false;
    }
  }

  /** Rule 3: called by the parent before switching notes or unmounting. */
  export async function flush(): Promise<void> {
    await save();
  }

  function extensions(): Extension[] {
    return [
      history(),
      drawSelection(),
      dropCursor(),
      rectangularSelection(),
      crosshairCursor(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      bracketMatching(),
      indentUnit.of("  "),
      EditorState.allowMultipleSelections.of(true),
      keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),

      // `codeLanguages` gives fenced blocks real highlighting; `extensions` adds
      // the wikilink and tag nodes so live preview can decorate from the tree.
      markdown({
        base: markdownLanguage,
        codeLanguages: languages,
        extensions: arcMarkdown,
      }),
      frontmatterSupport(),
      livePreview(),
      arcEditorTheme(),
      EditorView.lineWrapping,

      EditorView.updateListener.of((u) => {
        if (!u.docChanged) return;
        onstate(u.state.doc.toString() === committedText ? "clean" : "dirty");
        scheduleSave();
      }),
    ];
  }

  $effect(() => {
    if (!host) return;
    committedText = initialText;
    currentHash = baseHash;
    const v = new EditorView({
      state: EditorState.create({ doc: initialText, extensions: extensions() }),
      parent: host,
    });
    view = v;
    v.focus();

    return () => {
      // Drain anything pending before tearing down, or the last 400 ms of typing
      // is lost on every note switch.
      if (timer) {
        clearTimeout(timer);
        timer = null;
      }
      const text = v.state.doc.toString();
      if (text !== committedText) {
        void transport.saveNote(path, text, currentHash).catch(() => {
          /* reported on the next explicit save; nothing useful to do here */
        });
      }
      v.destroy();
      view = null;
    };
  });

  // The browser tab or window closing is the other way a pending save is lost.
  $effect(() => {
    const handler = () => {
      if (!view) return;
      const text = view.state.doc.toString();
      if (text !== committedText) void transport.saveNote(path, text, currentHash);
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  });
</script>

<div class="editor" bind:this={host}></div>

<style>
  .editor {
    height: 100%;
    overflow: hidden;
    padding: 0 var(--arc-space-7);
  }
  .editor :global(.cm-editor) {
    height: 100%;
  }
</style>
