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
  operation_id?: number;
  action?: "install" | "repair" | "uninstall" | null;
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

export interface SyntaxLanguageRow {
  key: string;
  title: string;
  state: "available" | "installed" | "broken";
  revision: string;
}
export interface SyntaxLanguagesView { languages: SyntaxLanguageRow[]; operation: SyntaxInstallView }
export type SyntaxLanguageAction = "install" | "repair" | "uninstall";
export const syntaxLanguages = (): Promise<SyntaxLanguagesView> => invoke("syntax_languages", { refresh: true });
export const syntaxLanguagesApply = (language: string, action: SyntaxLanguageAction, revision: string): Promise<SyntaxInstallView> =>
  invoke("syntax_languages_apply", { request: { language, action, revision } });

export const OPEN_SYNTAX_LANGUAGES = "amf:open-syntax-languages";
export const SYNTAX_OPERATION_STARTED = "amf:syntax-operation-started";
export const SYNTAX_LANGUAGES_CHANGED = "amf:syntax-languages-changed";
export function openSyntaxLanguages() { window.dispatchEvent(new Event(OPEN_SYNTAX_LANGUAGES)); }
export function syntaxOperationStarted(view: SyntaxInstallView) {
  window.dispatchEvent(new CustomEvent(SYNTAX_OPERATION_STARTED, { detail: view }));
}
export function syntaxLanguagesChanged(view?: SyntaxInstallView) {
  window.dispatchEvent(new CustomEvent(SYNTAX_LANGUAGES_CHANGED, { detail: view }));
}
