/**
 * Wire types, mirroring `arc-labs-api::types`.
 *
 * Hand-written rather than generated. The surface is small, it changes with the
 * phases, and a generator would be a build step to maintain for a file this
 * size. If it grows past a screen or two, generate it — but not before.
 */

export type VaultStatus = "offline" | "scanning" | "online" | "indexing";
export type Shell = "desktop" | "server";

export interface VaultInfo {
  name: string;
  /** Absent when the server is serving a client that must not learn its layout. */
  path?: string;
  noteCount: number;
  canvasCount: number;
}

export interface Status {
  status: VaultStatus;
  shell: Shell;
  version: string;
  vault?: VaultInfo;
  canBrowse: boolean;
  canPickFolder: boolean;
}

export interface Link {
  target: string;
  anchor?: string;
  alias?: string;
  display: string;
  /**
   * Whether the link resolves to a real note. `undefined` until Phase 2 builds
   * the index — constraint 7: an unknown is never rendered as an answer.
   */
  resolved?: boolean;
}

export interface NoteView {
  path: string;
  name: string;
  html: string;
  frontmatter?: string;
  links: Link[];
  embeds: Link[];
  tags: string[];
  size: number;
  lineEnding: string;
}

export interface TreeEntry {
  path: string;
  name: string;
  is_dir: boolean;
  parent: number | null;
}

export interface Skipped {
  path: string;
  reason: string;
}

export interface TreeView {
  entries: TreeEntry[];
  note_count: number;
  canvas_count: number;
  skipped: Skipped[];
}

export interface DirEntry {
  name: string;
  path: string;
  isVault: boolean;
}

export interface DirListing {
  path: string;
  parent?: string;
  entries: DirEntry[];
}

export type ErrorCode =
  | "no_vault"
  | "vault_not_found"
  | "not_a_directory"
  | "note_not_found"
  | "invalid_path"
  | "not_utf8"
  | "config"
  | "not_permitted"
  | "io";

export interface ApiError {
  code: ErrorCode;
  message: string;
}

export class TransportError extends Error {
  code: ErrorCode;
  constructor(e: ApiError) {
    super(e.message);
    this.name = "TransportError";
    this.code = e.code;
  }
}
