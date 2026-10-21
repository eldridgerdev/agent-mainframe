import { useRef, useState } from "react";
import { asGuiError, LibraryEntry, LibraryScope, Project } from "./api";
import { ExportDestination, ExportPreview, ExportRequest, promptExportCommit, promptExportPreview } from "./promptFilesApi";
import { Field, Modal, Spinner } from "./ui";

export default function PromptExportDialog({scope,entry,projects,onClose,onSaved}:{scope:LibraryScope;entry:LibraryEntry;projects:Project[];onClose:()=>void;onSaved:()=>void}) {
  const [destination,setDestination]=useState("");
  const [preview,setPreview]=useState<{request:ExportRequest;data:ExportPreview}|null>(null);
  const [confirmed,setConfirmed]=useState(false);
  const [busy,setBusy]=useState(false);
  const pending=useRef(false);
  const [error,setError]=useState<string|null>(null);
  const choices:{label:string;value:ExportDestination}[]=[{label:"Global configuration",value:{kind:"global"}}];
  for(const project of projects){
    choices.push({label:`Project: ${project.name}`,value:{kind:"project",project_id:project.id}});
    for(const feature of project.features) if(feature.workdir !== project.repo) choices.push({label:`Worktree: ${project.name} / ${feature.name}`,value:{kind:"worktree",project_id:project.id,feature_id:feature.id}});
  }
  const available=choices.some(choice=>JSON.stringify(choice.value)===destination);
  const currentPreview = preview && JSON.stringify(preview.request.destination)===destination ? preview : null;
  function close(){if(!pending.current)onClose();}
  async function load(){
    if(pending.current||!available)return;
    pending.current=true;setBusy(true);setError(null);setPreview(null);setConfirmed(false);
    const request:ExportRequest={scope,entry_key:entry.key,destination:JSON.parse(destination) as ExportDestination};
    try{const data=await promptExportPreview(request);setPreview({request,data});}
    catch(err){setError(asGuiError(err).message);}
    finally{pending.current=false;setBusy(false);}
  }
  return <Modal label="Export prompt template" title="Export prompt template" size="lg" onClose={close} dismissable={!busy}
    subtitle="Copy the full template into an explicit configuration destination. The source and composer draft are kept."
    footer={<><button className="btn btn-secondary" disabled={busy} onClick={close}>Cancel</button>
      <button className="btn btn-secondary" disabled={busy||!available} onClick={()=>void load()}>Refresh export preview</button>
      <button className="btn btn-primary" disabled={busy||!available||!currentPreview||(currentPreview.data.replacing&&!currentPreview.data.no_op&&!confirmed)} onClick={async()=>{
        if(pending.current||!preview||JSON.stringify(preview.request.destination)!==destination||!available||(preview.data.replacing&&!preview.data.no_op&&!confirmed))return;
        pending.current=true;setBusy(true);setError(null);
        try{await promptExportCommit(preview.request,preview.data.revision,confirmed);onSaved();}
        catch(err){setError(asGuiError(err).message);setConfirmed(false);}
        finally{pending.current=false;setBusy(false);}
      }}>{busy&&<Spinner/>}{preview?.data.no_op?"Confirm unchanged destination":"Export template"}</button></>}>
    <Field label="Export destination"><select value={destination} disabled={busy} onChange={event=>{if(pending.current)return;setDestination(event.target.value);setPreview(null);setConfirmed(false);setError(null);}}>
      <option value="">Choose a destination…</option>
      {destination&&!available&&<option value={destination}>Destination no longer available</option>}
      {choices.map(choice=><option key={JSON.stringify(choice.value)} value={JSON.stringify(choice.value)}>{choice.label}</option>)}
    </select></Field>
    {error&&<p role="alert" className="callout callout-danger">{error}</p>}
    {preview&&<>
      <p>Write to: <code>{preview.data.destination_path}</code></p>
      {preview.data.source_path&&<p className="muted small">Source: <code>{preview.data.source_path}</code></p>}
      <h3>{preview.data.name}</h3><pre className="library-preview">{preview.data.body}</pre>
      <p className="muted small">Description, configured placeholder definitions and other template metadata are copied. Unrelated destination fields are kept.</p>
      {preview.data.no_op?<p role="status">This destination already contains the same template. No file will be written.</p>:preview.data.replacing&&<div className="form-stack"><span className="field-label">Confirm replacement</span><label className={`option ${confirmed ? "option-selected" : ""}`}><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)}/>Replace the same-name destination template</label></div>}
    </>}
  </Modal>;
}
