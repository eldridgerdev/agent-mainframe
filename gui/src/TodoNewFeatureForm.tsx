import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  CreateFeatureRequest, HarnessInfo, ModeInfo, ModeSlug, Project,
  asGuiError, featurePresetsChoices,
} from "./api";
import WorktreeHookField, { useWorktreeHookChoice } from "./WorktreeHookField";
import { Field, Modal, Segmented, Spinner, Switch } from "./ui";
import { useCreationHarness } from "./useCreationHarness";
import CreationHarnessField from "./CreationHarnessField";

type PlanKind = "none" | "full" | "quick";

export default function TodoNewFeatureForm({ kind, todoTitle, projects, agents, modes, pending, onCancel, onSubmit }: {
  kind: "plan" | "launch";
  todoTitle: string;
  projects: Project[];
  agents: HarnessInfo[];
  modes: (Pick<ModeInfo, "slug" | "display_name"> & Partial<Pick<ModeInfo, "unsupported_agents">>)[];
  pending: boolean;
  onCancel: () => void;
  onSubmit: (request: CreateFeatureRequest, planKind: PlanKind) => void | Promise<unknown>;
}) {
  const suggested = todoTitle.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 40) || "todo-work";
  const initial = useRef({ projectId: projects[0]?.id ?? "", branch: suggested,
    agent: agents[0]?.slug ?? "claude", mode: modes[0]?.slug ?? "vibe",
    planKind: kind === "plan" ? "full" as const : "none" as const, review: false, chrome: false, remoteControl: false }).current;
  const [projectId, setProjectId] = useState(initial.projectId);
  const [branch, setBranch] = useState(initial.branch);
  const [mode, setMode] = useState<ModeSlug>(initial.mode);
  const [planKind, setPlanKind] = useState<PlanKind>(initial.planKind);
  const [review, setReview] = useState(false);
  const [chrome, setChrome] = useState(false);
  const [remoteControl, setRemoteControl] = useState(false);
  const [selection, setSelection] = useState<{ projectId: string; index: string } | null>(null);
  const [appliedPrefix, setAppliedPrefix] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [discard, setDiscard] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const inFlight = useRef(false);
  // A deleted destination must never silently fall back to another project.
  const project = projects.find((candidate) => candidate.id === projectId);
  const harness = useCreationHarness(projectId, !!project);
  const { choices, agent } = harness;
  const setAgent = harness.choose;
  const presets = useQuery({ queryKey: ["feature-presets", projectId], queryFn: () => featurePresetsChoices(projectId),
    enabled: !!project, retry: false, staleTime: 0, refetchOnWindowFocus: false });
  const offered = agents.filter(candidate => harness.offered.includes(candidate.slug));
  const selectedAgent = agent;
  const selectedMode = modes.find((candidate) => candidate.slug === mode);
  const unsupported = !!selectedAgent && !!selectedMode?.unsupported_agents?.includes(selectedAgent);
  const hook = useWorktreeHookChoice(project?.id, !!project);
  const busy = pending || submitting;
  const ready = !!project?.is_git && harness.ready && !!selectedAgent && !!selectedMode
    && presets.isSuccess && !presets.isFetching && hook.ready && !!branch.trim() && !remoteControl && !unsupported;
  const dirty = JSON.stringify({ projectId, branch, agent: initial.agent, mode, planKind, review, chrome, remoteControl }) !== JSON.stringify(initial) || harness.explicit || !!hook.choice;
  function close() {
    if (busy) return;
    if (dirty) setDiscard(true); else onCancel();
  }
  const planning = planKind !== "none";

  return <>
    <Modal size="lg" label={planning ? "Plan TODO in a new feature" : "Start TODO in a new feature"}
      title={planning ? "Plan in a new feature" : "Start in a new feature"} subtitle={todoTitle}
      hidden={discard} dismissable={!busy} onClose={close}
      onSubmit={async () => {
        if (busy || inFlight.current || !ready || !project || !selectedAgent) return;
        inFlight.current = true; setSubmitting(true);
        try {
          await onSubmit({ project_name: project.name, branch: branch.trim(), agent: selectedAgent, mode,
            review, plan_mode: planning, create_terminal: false, use_worktree: true,
            enable_chrome: chrome, hook_choice: hook.choice || null, dry_run: false }, planKind);
        } finally { inFlight.current = false; setSubmitting(false); }
      }}
      footer={<>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={close}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={busy || !ready}>
          {busy && <Spinner />}{planning ? planKind === "quick" ? "Start Quick Plan" : "Start plan" : "Create and start"}
        </button>
      </>}>
      <p className="muted small modal-lead">{planning
        ? "Creates the worktree and runs its setup before the interview. The agent starts after you accept the plan."
        : "Creates a worktree and starts its agent. The TODO prompt opens as an editable draft."}</p>
      {!project?.is_git && <p role="alert" className="callout callout-danger">Choose an available git project before creating a TODO feature.</p>}
      {modes.length === 0 && <p role="alert" className="callout callout-danger">No launch modes are available. Close and retry after refreshing the workspace.</p>}
      {presets.isError && <p role="alert" className="callout callout-danger">Could not load presets: {asGuiError(presets.error).message} <button type="button" disabled={busy || presets.isFetching} onClick={() => void presets.refetch()}>Retry presets</button></p>}
      {notice && <p role="alert" className="callout callout-warning">{notice}</p>}
      {remoteControl && <p role="alert" className="callout callout-warning">Remote Control presets can be created in the TUI. Desktop creation does not support Remote Control yet. Choose another preset or explicitly clear Remote Control below.</p>}
      {unsupported && <p role="alert" className="callout callout-warning">{selectedMode?.display_name} is unavailable with {offered.find((candidate) => candidate.slug === selectedAgent)?.display_name ?? selectedAgent}. Choose another mode or harness.</p>}
      <fieldset disabled={busy} className="form-stack" style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}>
        {(projects.length > 1 || !project) && <Field label="Project"><select value={projectId} disabled={busy || !projects.length} onChange={(event) => { setProjectId(event.target.value); setSelection(null); setNotice(null); }}>
          {!project && <option value={projectId} disabled>Choose a project…</option>}
          {projects.map((candidate) => <option key={candidate.id} value={candidate.id}>{candidate.name}</option>)}
        </select></Field>}
        <Field label="Feature preset" hint="Project templates override global templates by name. Apply a template, then edit any setting before creating.">
          <select aria-label="Feature preset" value={selection?.projectId === projectId ? selection.index : ""}
            disabled={busy || !presets.isSuccess || presets.isFetching || !choices.isSuccess || choices.isFetching}
            onChange={(event) => {
              const index = event.target.value; setSelection({ projectId, index }); setNotice(null);
              if (!index) return; // Custom settings retain every field, including remote intent.
              const preset = presets.data?.[Number(index)]; if (!preset) return;
              setAgent(preset.agent);
              if (!harness.offered.includes(preset.agent)) setNotice(`${preset.name} uses ${preset.agent}, which is not offered for this project. Choose an available harness explicitly.`);
              setMode(preset.mode); setReview(preset.review); setChrome(preset.enable_chrome);
              setRemoteControl(preset.remote_control); setPlanKind(!preset.plan_mode ? "none" : preset.quick_plan ? "quick" : "full");
              const prefix = preset.branch_prefix?.trim() || null;
              setBranch((current) => {
                const base = appliedPrefix && current.startsWith(`${appliedPrefix}/`) ? current.slice(appliedPrefix.length + 1) : current;
                return prefix ? `${prefix}/${base}` : base;
              });
              setAppliedPrefix(prefix);
            }}>
            <option value="">{presets.isPending ? "Loading presets…" : "Custom settings"}</option>
            {(presets.data ?? []).map((preset, index) => <option key={`${index}:${preset.name}`} value={String(index)}>{preset.name}</option>)}
          </select>
        </Field>
        <Field label="Branch"><input className="mono" value={branch} onChange={(event) => setBranch(event.target.value)} required autoFocus /></Field>
        <div className="form-row">
          <CreationHarnessField harness={harness} supported={agents} disabled={busy} />
          <Field label="Mode"><select value={mode} onChange={(event) => setMode(event.target.value as ModeSlug)}>
            {modes.map((candidate) => <option key={candidate.slug} value={candidate.slug}>{candidate.display_name}</option>)}
          </select></Field>
        </div>
        <div className="field"><span className="field-label">Planning</span><Segmented label="Planning" value={planKind} onChange={setPlanKind} options={[
          { value: "none", label: "Start directly" }, { value: "quick", label: "Quick Plan" }, { value: "full", label: "Full Plan" },
        ]} /></div>
        <div className="form-row"><Switch checked={review} onChange={setReview} label="Review" /><Switch checked={chrome} onChange={setChrome} label="Chrome" /></div>
        {remoteControl && <Switch checked={remoteControl} onChange={setRemoteControl} label="Remote Control" hint="Clear explicitly to create without Remote Control in the desktop." />}
        <WorktreeHookField hook={hook} disabled={busy} />
      </fieldset>
    </Modal>
    {discard && <Modal label="Discard TODO feature changes" title="Discard unsaved TODO feature settings?" onClose={() => setDiscard(false)}
      footer={<><button type="button" className="btn btn-secondary" onClick={() => setDiscard(false)}>Keep editing</button>
        <button type="button" className="btn btn-danger" onClick={onCancel}>Discard changes</button></>}>
      <p>The TODO remains unchanged. Your edited launch settings will be lost.</p>
    </Modal>}
  </>;
}
