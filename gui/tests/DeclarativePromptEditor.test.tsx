// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import DeclarativePromptEditor from "../src/DeclarativePromptEditor";
import type { DeclarativeEditorData } from "../src/promptFilesApi";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const snapshot: DeclarativeEditorData = { selection:{scope:{kind:"project",project_id:"p"},source:"Project",ordinal:0},context_revision:"checkout",revision:"raw-revision",name:"Repair",body:"Fix {{area}}",tags:"bug",description:"Keep description",configured_fields:2,original_id:null,original_name:"Repair",source_path:"/repo/.amf/config.json",destination_path:"/repo/amf.json" };
function mount() {
  const onClose=vi.fn(),onSaved=vi.fn();
  render(<DeclarativePromptEditor snapshot={snapshot} onClose={onClose} onSaved={onSaved}/>);
  return {onClose,onSaved};
}
it("retains edited fields after stale save and failed reload, then explicitly discards for a fresh revision", async () => {
  const { onClose, onSaved } = mount();
  vi.mocked(invoke).mockRejectedValue({ kind: "conflict", message: "Changed elsewhere. Keep your draft." });
  fireEvent.change(screen.getByRole("textbox", { name: /^Template body/ }), { target: { value: "Local unsaved body" } });
  fireEvent.click(screen.getByRole("button", { name: "Save template" }));
  await screen.findByText("Changed elsewhere. Keep your draft.");
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Local unsaved body");
  expect(invoke).toHaveBeenCalledWith("prompt_file_save", { request: { selection:snapshot.selection,context_revision:"checkout", revision:"raw-revision", name:"Repair",body:"Local unsaved body",tags:"bug" } });
  expect(onSaved).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Reload template" }));
  expect(screen.getByRole("dialog", { name: "Discard prompt edits" })).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  await screen.findByText("Changed elsewhere. Keep your draft.");
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Local unsaved body");
  vi.mocked(invoke).mockResolvedValue({ ...snapshot, revision: "new-revision", body: "External version" });
  fireEvent.click(screen.getByRole("button", { name: "Reload template" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  await waitFor(() => expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("External version"));
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(onClose).toHaveBeenCalledTimes(1);
});
it("protects close and never changes the supplied composer body during cancellation", () => {
  const { onClose } = mount();
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Fix {{area}}");
  fireEvent.change(screen.getByLabelText("Template name"), { target: { value: "My prompt" } });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.keyDown(document, { key: "Escape" });
  expect(onClose).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard changes" }));
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(invoke).not.toHaveBeenCalled();
});
it("locks pending saves against double submission, Escape and cancellation, keeping failures retryable", async () => {
  let reject!: (reason: unknown) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise((_resolve, fail) => { reject = fail; }));
  const { onClose, onSaved } = mount();
  fireEvent.change(screen.getByLabelText("Template name"), { target: { value: "Composer" } });
  const button = screen.getByRole("button", { name: "Save template" });
  fireEvent.click(button); fireEvent.click(button);
  fireEvent.keyDown(document, { key: "Escape" });
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(invoke).toHaveBeenCalledTimes(1); expect(onClose).not.toHaveBeenCalled();
  await act(async () => reject({ kind: "operation_failed", message: "Database busy" }));
  await screen.findByText("Database busy");
  expect((screen.getByRole("textbox", { name: /^Template body/ }) as HTMLTextAreaElement).value).toBe("Fix {{area}}");
  vi.mocked(invoke).mockResolvedValue("new-id"); fireEvent.click(button);
  await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1));
  expect(invoke).toHaveBeenCalledTimes(2);
});
it("does not open a reload discard overlay during the synchronous start of a save",async()=>{
 let reject!:(reason:unknown)=>void;vi.mocked(invoke).mockImplementation(()=>new Promise((_resolve,fail)=>{reject=fail;}));mount();
 fireEvent.change(screen.getByLabelText("Template name"),{target:{value:"Local name"}});
 act(()=>{fireEvent.click(screen.getByRole("button",{name:"Save template"}));fireEvent.click(screen.getByRole("button",{name:"Reload template"}));});
 expect(screen.queryByRole("dialog",{name:"Discard prompt edits"})).toBeNull();expect(invoke).toHaveBeenCalledTimes(1);
 await act(async()=>reject({kind:"conflict",message:"Keep draft"}));await screen.findByText("Keep draft");
});
