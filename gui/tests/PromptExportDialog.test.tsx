// @vitest-environment jsdom
import {afterEach,expect,it,vi} from "vitest";
import {act,cleanup,fireEvent,render,screen,waitFor} from "@testing-library/react";
import {invoke} from "@tauri-apps/api/core";
import PromptExportDialog from "../src/PromptExportDialog";
import type {LibraryEntry,Project} from "../src/api";
vi.mock("@tauri-apps/api/core",()=>({invoke:vi.fn()}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
const entry:LibraryEntry={key:"source",name:"Repair",body:"Fix {{area}}",description:null,tags:[],source:"User",slots:[]};
const project:Project={id:"p",name:"Project",repo:"/repo",is_git:true,features:[]};
const preview={revision:"snapshot",name:"Repair",body:"Fix {{area}}",description:null,configured_fields:2,destination_path:"/repo/amf.json",source_path:null,replacing:true,no_op:false};
const destination=JSON.stringify({kind:"project",project_id:"p"});
function mount(){const onClose=vi.fn(),onSaved=vi.fn();const result=render(<PromptExportDialog scope={{kind:"global"}} entry={entry} projects={[project]} onClose={onClose} onSaved={onSaved}/>);return {...result,onClose,onSaved};}
async function load(){fireEvent.change(screen.getByLabelText("Export destination"),{target:{value:destination}});fireEvent.click(screen.getByRole("button",{name:"Refresh export preview"}));await screen.findByText("/repo/amf.json");}
it("requires explicit replacement and refreshes stale destinations without losing the preview",async()=>{
 vi.mocked(invoke).mockResolvedValue(preview);const {onSaved}=mount();await load();
 const save=screen.getByRole("button",{name:"Export template"});expect((save as HTMLButtonElement).disabled).toBe(true);
 fireEvent.click(screen.getByRole("checkbox"));
 vi.mocked(invoke).mockRejectedValueOnce({kind:"conflict",message:"Destination changed. Refresh the preview."});fireEvent.click(save);
 await screen.findByText("Destination changed. Refresh the preview.");expect(onSaved).not.toHaveBeenCalled();expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);
 expect(screen.getByText("/repo/amf.json")).toBeTruthy();
 vi.mocked(invoke).mockResolvedValue({...preview,revision:"fresh"});fireEvent.click(screen.getByRole("button",{name:"Refresh export preview"}));await waitFor(()=>expect(screen.queryByRole("alert")).toBeNull());
 fireEvent.click(screen.getByRole("checkbox"));vi.mocked(invoke).mockResolvedValue(undefined);fireEvent.click(save);await waitFor(()=>expect(onSaved).toHaveBeenCalledTimes(1));
 expect(invoke).toHaveBeenLastCalledWith("prompt_export_commit",{request:{request:{scope:{kind:"global"},entry_key:"source",destination:{kind:"project",project_id:"p"}},revision:"fresh",replace_confirmed:true}});
});
it("clears preview and replacement intent when destination changes, including removed scope",async()=>{
 vi.mocked(invoke).mockResolvedValue(preview);const result=mount();await load();fireEvent.click(screen.getByRole("checkbox"));
 fireEvent.change(screen.getByLabelText("Export destination"),{target:{value:JSON.stringify({kind:"global"})}});expect(screen.queryByRole("checkbox")).toBeNull();expect(screen.queryByText("/repo/amf.json")).toBeNull();
 fireEvent.change(screen.getByLabelText("Export destination"),{target:{value:destination}});fireEvent.click(screen.getByRole("button",{name:"Refresh export preview"}));await screen.findByText("/repo/amf.json");
 result.rerender(<PromptExportDialog scope={{kind:"global"}} entry={entry} projects={[]} onClose={result.onClose} onSaved={result.onSaved}/>);
 expect((screen.getByRole("button",{name:"Export template"}) as HTMLButtonElement).disabled).toBe(true);expect(screen.getByText("Destination no longer available")).toBeTruthy();
});
it("blocks duplicate writes and dismissal while pending and makes no-op explicit",async()=>{
 vi.mocked(invoke).mockResolvedValue({...preview,no_op:true});const {onClose,onSaved}=mount();await load();expect(screen.queryByRole("checkbox")).toBeNull();
 let reject!:(error:unknown)=>void;vi.mocked(invoke).mockImplementation(()=>new Promise((_resolve,fail)=>{reject=fail;}));
 const save=screen.getByRole("button",{name:"Confirm unchanged destination"});fireEvent.click(save);fireEvent.click(save);fireEvent.keyDown(document,{key:"Escape"});fireEvent.click(screen.getByRole("button",{name:"Cancel"}));
 expect(onClose).not.toHaveBeenCalled();expect(invoke).toHaveBeenCalledTimes(2);
 await act(async()=>reject({kind:"operation_failed",message:"File busy"}));await screen.findByText("File busy");expect(onSaved).not.toHaveBeenCalled();
});
it("refuses synchronous destination changes while preview is pending and binds commit to that target",async()=>{
 let resolve!:(value:unknown)=>void;vi.mocked(invoke).mockImplementation(()=>new Promise(done=>{resolve=done;}));mount();
 const select=screen.getByLabelText("Export destination") as HTMLSelectElement;
 fireEvent.change(select,{target:{value:destination}});
 act(()=>{fireEvent.click(screen.getByRole("button",{name:"Refresh export preview"}));fireEvent.change(select,{target:{value:JSON.stringify({kind:"global"})}});});
 await act(async()=>resolve(preview));expect(select.value).toBe(destination);
 fireEvent.click(screen.getByRole("checkbox"));vi.mocked(invoke).mockResolvedValue(undefined);fireEvent.click(screen.getByRole("button",{name:"Export template"}));
 await waitFor(()=>expect(invoke).toHaveBeenCalledWith("prompt_export_commit",{request:{request:{scope:{kind:"global"},entry_key:"source",destination:{kind:"project",project_id:"p"}},revision:"snapshot",replace_confirmed:true}}));
});
