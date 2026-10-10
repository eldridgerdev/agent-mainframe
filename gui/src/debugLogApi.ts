import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "DEBUG" | "INFO" | "WARN" | "ERROR";
export interface DebugLogEntry {
  timestamp: string;
  level: LogLevel;
  context: string;
  message: string;
}
export interface DebugLogView {
  entries: DebugLogEntry[];
  limit: number;
  shared_history: boolean;
}
export const debugLogLoad = () => invoke<DebugLogView>("debug_log_load");
