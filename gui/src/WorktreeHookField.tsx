import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { asGuiError, worktreeHookPrompt } from "./api";
import { Field, Spinner } from "./ui";

export function useWorktreeHookChoice(projectId: string | undefined, enabled: boolean) {
  const [selection, setSelection] = useState<{ projectId: string; choice: string } | null>(null);
  const query = useQuery({
    queryKey: ["worktree-hook-prompt", projectId],
    queryFn: () => worktreeHookPrompt(projectId!),
    enabled: enabled && !!projectId,
    retry: false,
  });
  const choice = enabled && selection !== null && selection.projectId === projectId
    && query.data?.options.includes(selection.choice) ? selection.choice : "";
  return {
    query, choice, enabled,
    ready: !enabled || (query.isSuccess && !query.isFetching && (query.data === null || !!choice)),
    setChoice: (value: string) => setSelection(projectId ? { projectId, choice: value } : null),
  };
}

export default function WorktreeHookField({ hook, disabled }: {
  hook: ReturnType<typeof useWorktreeHookChoice>;
  disabled: boolean;
}) {
  if (!hook.enabled) return null;
  if (hook.query.isError) return (
    <div className="callout callout-warning" role="alert">
      <p>Could not load worktree setup options: {asGuiError(hook.query.error).message}</p>
      <button type="button" className="btn btn-secondary" disabled={disabled || hook.query.isFetching}
        onClick={() => void hook.query.refetch()}>Retry</button>
    </div>
  );
  if (hook.query.isPending) return <p className="field-hint"><Spinner /> Loading worktree setup…</p>;
  if (!hook.query.data) return null;
  return (
    <Field label={hook.query.data.title} hint="Runs when the worktree is created.">
      <select aria-label={hook.query.data.title} value={hook.choice} onChange={(event) => hook.setChoice(event.target.value)} required disabled={disabled}>
        <option value="" disabled>Choose an option</option>
        {hook.query.data.options.map((option) => <option key={option} value={option}>{option}</option>)}
      </select>
    </Field>
  );
}
