import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { AgentSlug, projectHarnessChoices } from "./api";

/** Initialize once per untouched destination. Explicit choices survive async
 * responses, refreshes and destination changes; unavailable intent blocks. */
export function useCreationHarness(projectId: string, enabled = true) {
  const choices = useQuery({ queryKey: ["project-harness-choices", projectId],
    queryFn: () => projectHarnessChoices(projectId), enabled, retry: false, staleTime: 0,
    refetchOnWindowFocus: false, refetchOnReconnect: false });
  const [selection, setSelection] = useState<{ projectId: string; agent: AgentSlug | null; explicit: boolean }>({ projectId, agent: null, explicit: false });
  useEffect(() => {
    setSelection(current => {
      const next = current.projectId !== projectId
        ? { projectId, agent: current.explicit ? current.agent : null, explicit: current.explicit } : current;
      if (!next.explicit && next.agent === null && choices.isSuccess && !choices.isFetching)
        return { ...next, agent: choices.data.default_agent };
      return next;
    });
  }, [projectId, choices.data, choices.isSuccess, choices.isFetching]);
  const agent = selection.explicit || selection.projectId === projectId ? selection.agent : null;
  const offered = choices.data?.agents ?? [];
  return { choices, agent, offered, explicit: selection.explicit,
    ready: choices.isSuccess && !choices.isFetching && agent !== null && offered.includes(agent),
    choose: (agent: AgentSlug) => setSelection({ projectId, agent, explicit: true }) };
}
