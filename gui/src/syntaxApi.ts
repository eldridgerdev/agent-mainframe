import { invoke } from "@tauri-apps/api/core";

// Mirrors `src/gui_syntax.rs`. Kept beside, not inside, api.ts so the
// highlighting contract can grow without touching the shared type module.

/** `[tokenClass, text]`; an empty class is plain text. A line's spans
 * concatenate to exactly its text. */
export type SyntaxSpan = [string, string];

export type SyntaxStatus =
  | "highlighted" | "unsupported" | "not_installed" | "broken" | "too_large" | "over_budget" | "binary";

export interface SyntaxInfo {
  /** Display title, e.g. "Rust"; null when no language was detected. */
  language: string | null;
  /** Parser key an install names, e.g. "rust". */
  language_key: string | null;
  status: SyntaxStatus;
}

export interface SyntaxInstallView {
  language: string | null;
  language_key: string | null;
  running: boolean;
  output: string | null;
  message: string | null;
  error: string | null;
  /** Increments as each install finishes. */
  completed: number;
}

export const syntaxInstallStatus = (): Promise<SyntaxInstallView> => invoke("syntax_install_status");
export const syntaxInstall = (language: string): Promise<SyntaxInstallView> => invoke("syntax_install", { language });
