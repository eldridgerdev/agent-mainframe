import { invoke } from "@tauri-apps/api/core";
import { LibraryScope } from "./api";
export interface FileSelection { scope: LibraryScope; source: string; ordinal: number }
export interface DeclarativeEditorData {
  selection: FileSelection; context_revision: string; revision: string;
  name: string; body: string; tags: string; description: string | null; configured_fields: number;
  original_id: string | null; original_name: string; source_path: string; destination_path: string;
}
export type ExportDestination = {kind:"global"} | {kind:"project";project_id:string} | {kind:"worktree";project_id:string;feature_id:string};
export interface ExportRequest {scope:LibraryScope;entry_key:string;destination:ExportDestination}
export interface ExportPreview {revision:string;name:string;body:string;description:string|null;configured_fields:number;source_path:string|null;destination_path:string;replacing:boolean;no_op:boolean}
export const promptFileEdit = (scope:LibraryScope,entry_key:string):Promise<DeclarativeEditorData> => invoke("prompt_file_edit",{scope,entryKey:entry_key});
export const promptFileReload = (original:DeclarativeEditorData):Promise<DeclarativeEditorData> => invoke("prompt_file_reload",{request:{selection:original.selection,context_revision:original.context_revision,original_name:original.original_name,original_id:original.original_id}});
export const promptFileSave = (original:DeclarativeEditorData,name:string,body:string,tags:string):Promise<void> => invoke("prompt_file_save",{request:{selection:original.selection,context_revision:original.context_revision,revision:original.revision,name,body,tags}});
export const promptExportPreview = (request:ExportRequest):Promise<ExportPreview> => invoke("prompt_export_preview",{request});
export const promptExportCommit = (request:ExportRequest,revision:string,replace_confirmed:boolean):Promise<void> => invoke("prompt_export_commit",{request:{request,revision,replace_confirmed}});
