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

import type { DirListing, NoteView, Status, TreeView, VaultInfo } from "./types";
import { TransportError } from "./types";

export interface Transport {
  readonly kind: "desktop" | "server";
  status(): Promise<Status>;
  tree(): Promise<TreeView>;
  note(path: string): Promise<NoteView>;
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
