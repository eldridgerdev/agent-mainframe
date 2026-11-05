import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getSnapshot, type WorkspaceSnapshot } from "./api";

export const SNAPSHOT_KEY = ["workspace-snapshot"];

export function useWorkspaceSnapshot() {
  const queryClient = useQueryClient();
  const workspace = useQuery({
    queryKey: SNAPSHOT_KEY,
    queryFn: getSnapshot,
    refetchInterval: 2_000,
  });

  // Tauri mutations publish the committed snapshot; other processes are covered by polling.
  useEffect(() => {
    const unlistenPromise = listen<WorkspaceSnapshot>("workspace-changed", (event) => {
      queryClient.setQueryData(SNAPSHOT_KEY, event.payload);
    });
    return () => void unlistenPromise.then((unlisten) => unlisten());
  }, [queryClient]);

  return workspace;
}
