import { invoke } from "@tauri-apps/api/core";
import type {
  AddSessionResponse,
  EditorCleanup,
  FeatureTarget,
  NewSessionKind,
  NewSessionOption,
  SessionTarget,
} from "./api";
import { addSession } from "./api";

// Mirrors `src/gui_sessions.rs`: the TUI session picker's VS Code, custom
// session and TODOs entries. Kept beside api.ts so the picker contract can
// grow without touching the shared type module.

export interface CustomSessionOption {
  name: string;
  description: string | null;
  icon: string | null;
  /** `icon_nerd` resolved to its glyph (the window bundles the symbols font). */
  icon_nerd: string | null;
  command: string | null;
  working_dir: string | null;
  window_name: string | null;
  pre_check: string | null;
  on_stop: string | null;
  autolaunch: boolean;
  source: "project" | "global";
  revision: string;
}

export interface NewSessionOptions {
  builtin: NewSessionOption[];
  custom: CustomSessionOption[];
  config_warning: string | null;
  feature_stopped: boolean;
}

export type AddCustomSessionResponse =
  | { status: "added"; target: SessionTarget; label: string; autolaunch: boolean; message: string }
  | { status: "pre_check_failed"; name: string; pre_check: string; output: string };

export interface OpenVscodeResponse {
  feature_id: string;
  workdir: string;
  editor_id: string | null;
  started_feature: boolean;
  message: string;
}

export type FeatureEditorState = "opening" | "open" | "not_owned";

export interface FeatureEditor {
  id: string;
  name: string;
  state: FeatureEditorState;
  closes_with_feature: boolean;
  started_at: string;
}

export interface CloseEditorsResponse {
  already_closed: boolean;
  editors: EditorCleanup;
  message: string;
}

export const newSessionOptions = (target: FeatureTarget): Promise<NewSessionOptions> =>
  invoke("new_session_options", { target });

export const addCustomSession = (request: {
  target: FeatureTarget;
  name: string;
  revision: string;
  label: string | null;
  approved: boolean;
}): Promise<AddCustomSessionResponse> => invoke("add_custom_session", { request });

export const openVscode = (target: FeatureTarget, approved: boolean): Promise<OpenVscodeResponse> =>
  invoke("open_vscode", { target, approved });

export const closeEditors = (target: FeatureTarget, seen: string[]): Promise<CloseEditorsResponse> =>
  invoke("close_editors", { target, seen });

/** What the New session dialog asked for. */
export type SessionChoice =
  | { type: "builtin"; kind: NewSessionKind }
  | { type: "custom"; name: string; revision: string };

/** One create request, whichever engine it goes to, normalized for App. */
export type SessionCreated =
  /** A tab to show; `focus` false keeps the current tab (a custom session
   *  without `autolaunch`, as the TUI stays on its dashboard). */
  | { outcome: "session"; target: SessionTarget; label: string; kind: NewSessionKind | "custom"; focus: boolean }
  | { outcome: "vscode"; response: OpenVscodeResponse }
  | { outcome: "pre_check_failed"; name: string; preCheck: string; output: string };

export async function createSession(
  target: FeatureTarget,
  choice: SessionChoice,
  label: string | null,
  approved: boolean,
): Promise<SessionCreated> {
  if (choice.type === "custom") {
    const response = await addCustomSession({
      target, name: choice.name, revision: choice.revision, label, approved,
    });
    return response.status === "added"
      ? { outcome: "session", target: response.target, label: response.label, kind: "custom", focus: response.autolaunch }
      : { outcome: "pre_check_failed", name: response.name, preCheck: response.pre_check, output: response.output };
  }
  if (choice.kind === "vscode") {
    return { outcome: "vscode", response: await openVscode(target, approved) };
  }
  const response: AddSessionResponse = await addSession(target, choice.kind, label, approved);
  return { outcome: "session", target: response.target, label: response.label, kind: choice.kind, focus: true };
}
