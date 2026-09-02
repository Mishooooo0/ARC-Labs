/**
 * The transport boundary.
 *
 * This is the load-bearing decision of the whole build. ARC-LABS runs as a Tauri
 * desktop app on Windows and Linux, as an HTTP server for a browser, and inside
 * Docker. That is four shells — and exactly one UI, because everything above
 * this file talks to the `Transport` interface and never learns which one it is
 * in.
 *
 * **No component may import `@tauri-apps/api`.** The moment one does, the
 * browser and Docker builds break in a way that only shows up at runtime, in the
 * one environment nobody is testing in. Everything Tauri-specific is here.
 *
 * Adding an operation means adding it to the interface and to both
 * implementations, which is deliberate friction: it is the compiler asking
 * whether the browser shell can do this too.
 */

import type {
  Backlink, DirListing, EntryDiff, GraphData, IndexStats, NoteRef, NoteView,
  OutgoingLink, Proposal, SaveResult, SearchHit, Status, TagCount, TimelineEntry,
  TreeView, UnresolvedLink, VaultInfo,
} from "./types";
import { TransportError } from "./types";

export interface Transport {
  readonly kind: "desktop" | "server";
  status(): Promise<Status>;
  tree(): Promise<TreeView>;
  note(path: string): Promise<NoteView>;
  /** Same as `note`, plus the raw markdown for the editor. */
  noteForEdit(path: string): Promise<NoteView>;
  /**
   * Save. `baseHash` is the hash the editor started from; the server refuses
   * rather than overwriting if the file changed underneath.
   */
  saveNote(path: string, text: string, baseHash: string): Promise<SaveResult>;
  // ── Ledger (Phase 3) ──────────────────────────────────────────────────────
  timeline(path: string): Promise<TimelineEntry[]>;
  proposals(path: string): Promise<Proposal[]>;
  entryDiff(path: string, index: number): Promise<EntryDiff>;
  restore(path: string, index: number): Promise<SaveResult>;
  accept(path: string, index: number): Promise<SaveResult>;
  reject(path: string, index: number): Promise<void>;
  propose(
    path: string, agent: string, model: string, session: string, reason: string, content: string,
  ): Promise<Proposal>;

  // ── Index-backed (Phase 2) ────────────────────────────────────────────────
  search(q: string, limit?: number): Promise<SearchHit[]>;
  quickOpen(q: string, limit?: number): Promise<NoteRef[]>;
  recent(limit?: number): Promise<NoteRef[]>;
  backlinks(path: string): Promise<Backlink[]>;
  outgoing(path: string): Promise<OutgoingLink[]>;
  unresolved(limit?: number): Promise<UnresolvedLink[]>;
  tags(): Promise<TagCount[]>;
  tagNotes(tag: string): Promise<NoteRef[]>;
  graph(): Promise<GraphData>;
  indexStats(): Promise<IndexStats>;

  browse(path?: string): Promise<DirListing>;
  openVault(path: string): Promise<VaultInfo>;
  /** Native folder picker. `null` in a browser, which has none. */
  pickFolder(): Promise<string | null>;
}

/**
 * Tauri 2 installs `__TAURI_INTERNALS__` before any app script runs. Checked
 * once at module load rather than per call, so behaviour cannot change halfway
 * through a session.
 */
function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

// ── Server transport ────────────────────────────────────────────────────────

/**
 * Bearer token for a server bound past loopback. It arrives once as `?token=`,
 * is moved straight into sessionStorage, and is stripped from the URL — so it
 * does not sit in the address bar, in history, or in a `Referer` header.
 *
 * sessionStorage rather than localStorage: the grant should end with the tab.
 */
function takeToken(): string | null {
  if (typeof window === "undefined") return null;
  const key = "arc-labs-token";
  try {
    const url = new URL(window.location.href);
    const fromUrl = url.searchParams.get("token");
    if (fromUrl) {
      sessionStorage.setItem(key, fromUrl);
      url.searchParams.delete("token");
      window.history.replaceState({}, "", url.toString());
      return fromUrl;
    }
    return sessionStorage.getItem(key);
  } catch {
    // Private mode, or storage disabled. The app still works on loopback, where
    // no token is needed at all.
    return null;
  }
}

class ServerTransport implements Transport {
  readonly kind = "server" as const;
  #token = takeToken();

  async #call<T>(path: string, init?: RequestInit): Promise<T> {
    const headers = new Headers(init?.headers);
    if (this.#token) headers.set("Authorization", `Bearer ${this.#token}`);
    if (init?.body) headers.set("Content-Type", "application/json");

    let res: Response;
    try {
      res = await fetch(`api/${path}`, { ...init, headers });
    } catch {
      // A dead socket is not an API error, and must not be reported as one —
      // "the server went away" is a different problem from "no such note".
      throw new TransportError({
        code: "io",
        message: "cannot reach the ARC-LABS server",
      });
    }

    if (!res.ok) {
      let body: unknown = null;
      try {
        body = await res.json();
      } catch {
        /* a non-JSON error body is still an error */
      }
      if (body && typeof body === "object" && "code" in body) {
        throw new TransportError(body as never);
      }
      throw new TransportError({ code: "io", message: `request failed (${res.status})` });
    }
    return (await res.json()) as T;
  }

  status() {
    return this.#call<Status>("status");
  }
  tree() {
    return this.#call<TreeView>("tree");
  }
  note(path: string) {
    return this.#call<NoteView>(`note?path=${encodeURIComponent(path)}`);
  }
  noteForEdit(path: string) {
    return this.#call<NoteView>(`note/edit?path=${encodeURIComponent(path)}`);
  }
  saveNote(path: string, text: string, baseHash: string) {
    return this.#call<SaveResult>("note/save", {
      method: "POST",
      body: JSON.stringify({ path, text, base_hash: baseHash }),
    });
  }
  timeline(path: string) {
    return this.#call<TimelineEntry[]>(`timeline?path=${encodeURIComponent(path)}`);
  }
  proposals(path: string) {
    return this.#call<Proposal[]>(`proposals?path=${encodeURIComponent(path)}`);
  }
  entryDiff(path: string, index: number) {
    return this.#call<EntryDiff>(
      `entry-diff?path=${encodeURIComponent(path)}&index=${index}`,
    );
  }
  restore(path: string, index: number) {
    return this.#call<SaveResult>("restore", {
      method: "POST", body: JSON.stringify({ path, index }),
    });
  }
  accept(path: string, index: number) {
    return this.#call<SaveResult>("accept", {
      method: "POST", body: JSON.stringify({ path, index }),
    });
  }
  reject(path: string, index: number) {
    return this.#call<void>("reject", {
      method: "POST", body: JSON.stringify({ path, index }),
    });
  }
  propose(path: string, agent: string, model: string, session: string, reason: string, content: string) {
    return this.#call<Proposal>("propose", {
      method: "POST",
      body: JSON.stringify({ path, agent, model, session, reason, content }),
    });
  }

  search(q: string, limit = 50) {
    return this.#call<SearchHit[]>(`search?q=${encodeURIComponent(q)}&limit=${limit}`);
  }
  quickOpen(q: string, limit = 50) {
    return this.#call<NoteRef[]>(`quick-open?q=${encodeURIComponent(q)}&limit=${limit}`);
  }
  recent(limit = 20) {
    return this.#call<NoteRef[]>(`recent?limit=${limit}`);
  }
  backlinks(path: string) {
    return this.#call<Backlink[]>(`backlinks?path=${encodeURIComponent(path)}`);
  }
  outgoing(path: string) {
    return this.#call<OutgoingLink[]>(`outgoing?path=${encodeURIComponent(path)}`);
  }
  unresolved(limit = 100) {
    return this.#call<UnresolvedLink[]>(`unresolved?limit=${limit}`);
  }
  tags() {
    return this.#call<TagCount[]>("tags");
  }
  tagNotes(tag: string) {
    return this.#call<NoteRef[]>(`tag?q=${encodeURIComponent(tag)}`);
  }
  graph() {
    return this.#call<GraphData>("graph");
  }
  indexStats() {
    return this.#call<IndexStats>("index-stats");
  }

  browse(path?: string) {
    const q = path ? `?path=${encodeURIComponent(path)}` : "";
    return this.#call<DirListing>(`browse${q}`);
  }
  openVault(path: string) {
    return this.#call<VaultInfo>("vault/open", {
      method: "POST",
      body: JSON.stringify({ path }),
    });
  }
  async pickFolder() {
    // A browser cannot open a native folder dialog, and must not pretend to.
    // The first-run surface uses `browse()` to build a picker instead.
    return null;
  }
}

// ── Desktop transport ───────────────────────────────────────────────────────

class DesktopTransport implements Transport {
  readonly kind = "desktop" as const;

  async #invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
    // Imported lazily and by name so the browser bundle never resolves it.
    const { invoke } = await import("@tauri-apps/api/core");
    try {
      return (await invoke<T>(cmd, args)) as T;
    } catch (e) {
      // Tauri rejects with whatever the command returned — our ApiError shape.
      if (e && typeof e === "object" && "code" in e) throw new TransportError(e as never);
      throw new TransportError({ code: "io", message: String(e) });
    }
  }

  status() {
    return this.#invoke<Status>("status");
  }
  tree() {
    return this.#invoke<TreeView>("tree");
  }
  note(path: string) {
    return this.#invoke<NoteView>("note", { path });
  }
  noteForEdit(path: string) {
    return this.#invoke<NoteView>("note_for_edit", { path });
  }
  saveNote(path: string, text: string, baseHash: string) {
    return this.#invoke<SaveResult>("save_note", { path, text, baseHash });
  }
  timeline(path: string) {
    return this.#invoke<TimelineEntry[]>("timeline", { path });
  }
  proposals(path: string) {
    return this.#invoke<Proposal[]>("proposals", { path });
  }
  entryDiff(path: string, index: number) {
    return this.#invoke<EntryDiff>("entry_diff", { path, index });
  }
  restore(path: string, index: number) {
    return this.#invoke<SaveResult>("restore", { path, index });
  }
  accept(path: string, index: number) {
    return this.#invoke<SaveResult>("accept", { path, index });
  }
  reject(path: string, index: number) {
    return this.#invoke<void>("reject", { path, index });
  }
  propose(path: string, agent: string, model: string, session: string, reason: string, content: string) {
    return this.#invoke<Proposal>("propose", { path, agent, model, session, reason, content });
  }

  search(q: string, limit = 50) {
    return this.#invoke<SearchHit[]>("search", { q, limit });
  }
  quickOpen(q: string, limit = 50) {
    return this.#invoke<NoteRef[]>("quick_open", { q, limit });
  }
  recent(limit = 20) {
    return this.#invoke<NoteRef[]>("recent", { limit });
  }
  backlinks(path: string) {
    return this.#invoke<Backlink[]>("backlinks", { path });
  }
  outgoing(path: string) {
    return this.#invoke<OutgoingLink[]>("outgoing", { path });
  }
  unresolved(limit = 100) {
    return this.#invoke<UnresolvedLink[]>("unresolved", { limit });
  }
  tags() {
    return this.#invoke<TagCount[]>("tags");
  }
  tagNotes(tag: string) {
    return this.#invoke<NoteRef[]>("tag_notes", { q: tag });
  }
  graph() {
    return this.#invoke<GraphData>("graph");
  }
  indexStats() {
    return this.#invoke<IndexStats>("index_stats");
  }

  browse(path?: string) {
    return this.#invoke<DirListing>("browse", { path: path ?? null });
  }
  openVault(path: string) {
    return this.#invoke<VaultInfo>("open_vault", { path });
  }
  pickFolder() {
    return this.#invoke<string | null>("pick_folder");
  }
}

/** Chosen once, at module load. */
export const transport: Transport = inTauri() ? new DesktopTransport() : new ServerTransport();
