import { useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { CreateFeatureRequest, HarnessInfo, ModeInfo, ModeSlug, asGuiError, featurePresetsChoices } from "./api";
import { useCreationHarness } from "./useCreationHarness";
import { CreationHarnessNotices, CreationHarnessSelect } from "./CreationHarnessField";
import { CHROME_HINT, DESIGN_REVIEW_CLAUDE_ONLY, DESIGN_REVIEW_HINT, PLAN_HINTS, REVIEW_HINT, designReviewUnavailableReason } from "./creationCopy";
import WorktreeHookField, { useWorktreeHookChoice } from "./WorktreeHookField";
import { ExperimentalBadge, Field, Modal, Segmented, Spinner, Switch } from "./ui";

import { SuperVibeConfirm, modeHint, useSuperVibeConfirm } from "./SuperVibeWarning";

type PlanKind = "none" | "quick" | "full";

export default function CreateFeatureForm({
  projectId,
  projectName,
  agents,
  modes,
  isGit,
  defaultUseWorktree,
  pending,
  hidden = false,
  onOpenAvailability,
  onCancel,
  onSubmit,
}: {
  projectId: string;
  projectName: string;
  agents: HarnessInfo[];
  modes: ModeInfo[];
  isGit: boolean;
  defaultUseWorktree: boolean;
  pending: boolean;
  /** Kept mounted, with its edits, while Agent availability is in front. */
  hidden?: boolean;
  onOpenAvailability?: () => void;
  onCancel: () => void;
  onSubmit: (request: Omit<CreateFeatureRequest, "project_name">, planKind: PlanKind, options?: { steering: boolean }) => void;
}) {
  const [branch, setBranch] = useState("");
  const [steering, setSteering] = useState(false);
  const harness = useCreationHarness(projectId);
  const agent = harness.agent;
  const setAgent = harness.choose;
  const [mode, setMode] = useState<ModeSlug>(modes[0]?.slug ?? "vibe");
  const [useWorktree, setUseWorktree] = useState(defaultUseWorktree);
  const [planKind, setPlanKind] = useState<PlanKind>("none");
  const [review, setReview] = useState(false);
  const [designReview, setDesignReview] = useState(false);
  const [chrome, setChrome] = useState(false);
  const [remoteControl, setRemoteControl] = useState(false);
  const [presetIndex, setPresetIndex] = useState("");
  // Prefix the last applied preset put on the branch, so the next one can swap it out.
  const [appliedPrefix, setAppliedPrefix] = useState<string | null>(null);
  const [presetNotice, setPresetNotice] = useState<string | null>(null);
  const presets = useQuery({ queryKey: ["feature-presets", projectId], queryFn: () => featurePresetsChoices(projectId), retry: false, staleTime: 0, refetchOnWindowFocus: false });
  // Design Review is Claude only for now: the switch is disabled with the
  // reason for any other harness, and a stale "on" is not sent.
  const designReviewReason = designReviewUnavailableReason(agent);
  const designReviewOn = designReview && !designReviewReason;
  const unsupportedMode = agent === "codex" && mode === "vibeless";
  const modeInfo = modes.find((candidate) => candidate.slug === mode);
  const selectedAgent = agent;
  const hook = useWorktreeHookChoice(projectId, isGit && useWorktree);
  const [confirmSuperVibe, setConfirmSuperVibe] = useSuperVibeConfirm(mode);
  const modeSelect = useRef<HTMLSelectElement>(null);

  function create(confirmed: boolean) {
    if (pending || !hook.ready || !branch.trim() || remoteControl || unsupportedMode || !harness.ready || !selectedAgent) return;
    // As in the terminal app, SuperVibe asks once more before creating.
    if (mode === "supervibe" && !confirmed) { setConfirmSuperVibe(true); return; }
    setConfirmSuperVibe(false);
    const request = {
      branch: branch.trim(),
      agent: selectedAgent,
      mode,
      review,
      design_review: designReviewOn,
      plan_mode: planKind !== "none",
      create_terminal: false,
      use_worktree: useWorktree,
      enable_chrome: chrome,
      hook_choice: hook.choice || null,
      dry_run: false,
    };
    if (steering && planKind === "none") onSubmit(request, planKind, { steering: true });
    else onSubmit(request, planKind);
  }

  return (
    <Modal
      size="lg"
      label="New feature"
      title="New feature"
      subtitle={`in ${projectName}. A feature is a branch with its own agent sessions, so work stays separate.`}
      onClose={onCancel}
      dismissable={!pending}
      hidden={hidden}
      onSubmit={() => create(false)}
      footer={
        <>
          <button type="button" className="btn btn-ghost" onClick={onCancel} disabled={pending}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={pending || !hook.ready || !branch.trim() || remoteControl || unsupportedMode || !harness.ready}>
            {pending && <Spinner />}
            {pending ? "Creating…" : planKind === "none" ? "Create feature" : "Create and plan"}
          </button>
        </>
      }
    >
      <div className="form-stack">
        <Field label="Feature preset" hint="A preset fills in the fields below; you can still edit them. Project presets override global ones with the same name.">
          <select aria-label="Feature preset" value={presetIndex} disabled={pending || presets.isPending} onChange={(event) => {
            const value = event.target.value; setPresetIndex(value); setPresetNotice(null);
            if (!value) return;
            const preset = presets.data?.[Number(value)];
            if (!preset) return;
            setAgent(preset.agent);
            const presetDesignReview = !!preset.design_review && !preset.review;
            const ignoredDesignReview = presetDesignReview && designReviewUnavailableReason(preset.agent) !== null;
            if (!harness.offered.includes(preset.agent)) setPresetNotice(`${preset.name} uses ${preset.agent}, which is not offered for this project. Choose an available agent.`);
            else if (ignoredDesignReview) setPresetNotice(`${preset.name}: Design Review ignored (${DESIGN_REVIEW_CLAUDE_ONLY}).`);
            setMode(preset.mode); setReview(preset.review); setDesignReview(presetDesignReview && !ignoredDesignReview); setChrome(preset.enable_chrome);
            setRemoteControl(preset.remote_control); setPlanKind(!preset.plan_mode ? "none" : preset.quick_plan ? "quick" : "full"); if (preset.plan_mode) setSteering(false);
            // Keep the name the user typed; only the preset's prefix changes.
            const prefix = preset.branch_prefix?.trim() || null;
            setBranch((current) => {
              const base = appliedPrefix && current.startsWith(`${appliedPrefix}/`) ? current.slice(appliedPrefix.length + 1) : current;
              return prefix ? `${prefix}/${base}` : base;
            });
            setAppliedPrefix(prefix);
          }}>
            <option value="">Custom settings</option>
            {(presets.data ?? []).map((preset, index) => <option key={`${index}:${preset.name}`} value={String(index)}>{preset.name}</option>)}
          </select>
        </Field>
        {presets.error && <p className="callout callout-danger" role="alert">Could not load presets: {asGuiError(presets.error).message}</p>}
        {presetNotice && <p className="callout callout-warning" role="alert">{presetNotice}</p>}
        {remoteControl && <p className="callout callout-warning" role="alert">Remote Control presets can be created in AMF's terminal app. The desktop app does not support Remote Control yet. Choose another preset or explicitly clear Remote Control below.</p>}
        {unsupportedMode && <p className="callout callout-warning" role="alert">Codex does not support Vibeless. Choose Vibe or SuperVibe, or another agent.</p>}
        <Field label="Branch / feature name">
          <input
            className="mono"
            value={branch}
            onChange={(event) => setBranch(event.target.value)}
            placeholder="fix-login-redirect"
            required
            autoFocus
          />
        </Field>
        <div className="form-row creation-agent-mode">
          <CreationHarnessSelect harness={harness} supported={agents} disabled={pending} />
          <Field label="Mode" hint={modeHint(modeInfo?.description, mode)}>
            <select ref={modeSelect} value={mode} onChange={(event) => setMode(event.target.value as ModeSlug)}>
              {modes.map((m) => (
                <option key={m.slug} value={m.slug}>{m.display_name}</option>
              ))}
            </select>
          </Field>
        </div>
        {confirmSuperVibe && (
          <SuperVibeConfirm
            busy={pending}
            onSafer={() => { modeSelect.current?.focus(); setConfirmSuperVibe(false); }}
            onConfirm={() => create(true)}
          />
        )}
        <CreationHarnessNotices harness={harness} disabled={pending} onOpenAvailability={onOpenAvailability} />
        <Field label="Planning" hint={PLAN_HINTS[planKind]}>
          <Segmented
            label="Planning"
            value={planKind}
            onChange={(next) => {
              // Planning owns the startup prompt, so the coach goes back to its
              // default (off) instead of silently returning when planning is cleared.
              if (next !== "none") setSteering(false);
              setPlanKind(next);
            }}
            options={[
              { value: "none", label: "Start directly" },
              { value: "quick", label: "Quick Plan" },
              { value: "full", label: "Full Plan" },
            ]}
          />
        </Field>
        <Switch
          checked={steering && planKind === "none"}
          disabled={planKind !== "none"}
          onChange={setSteering}
          label="Steering coach"
          badge={<ExperimentalBadge />}
          hint={planKind !== "none"
            ? "Planning already shapes the startup prompt, so the coach is off while Quick or Full Plan is chosen."
            : "After the agent starts, get feedback on your first message: is the goal clear, how will you know it is done, what could go wrong. Nothing is sent until you send it."}
        />
        {isGit && (
          <Switch
            checked={useWorktree}
            onChange={setUseWorktree}
            label="Use a separate checkout (worktree)"
            hint="Gives the feature its own folder and branch so agents don't collide."
          />
        )}
        <div className="form-row">
          <Switch checked={review} onChange={(value) => { setReview(value); if (value) setDesignReview(false); }} label="Review" hint={REVIEW_HINT} />
          <Switch checked={chrome} onChange={setChrome} label="Chrome" hint={CHROME_HINT} />
        </div>
        <Switch
          checked={designReviewOn}
          onChange={(value) => { setDesignReview(value); if (value) setReview(false); }}
          disabled={designReviewReason !== null}
          label={`Design review (${DESIGN_REVIEW_CLAUDE_ONLY})`}
          hint={designReviewReason ? `Design Review: ${designReviewReason}` : DESIGN_REVIEW_HINT}
        />
        {remoteControl && <Switch checked={remoteControl} onChange={setRemoteControl} label="Remote Control" hint="Clear explicitly to create without Remote Control in the desktop." />}
        <WorktreeHookField hook={hook} disabled={pending} />
      </div>
    </Modal>
  );
}
