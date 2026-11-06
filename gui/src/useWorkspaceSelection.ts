import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { Feature, Project, WorkspaceSnapshot } from "./api";
import { SNAPSHOT_KEY } from "./workspaceSnapshot";

export type WorkspaceView =
  | { kind: "todos" }
  | { kind: "welcome" }
  | { kind: "project"; projectId: string }
  | { kind: "feature"; projectId: string; featureId: string };

export function useWorkspaceSelection(workspace: WorkspaceSnapshot | undefined) {
  const queryClient = useQueryClient();
  const [view, setView] = useState<WorkspaceView | null>(null);
  const projects = workspace?.projects ?? [];
  const selectedProject: Project | undefined = view && (view.kind === "project" || view.kind === "feature")
    ? projects.find((project) => project.id === view.projectId)
    : undefined;
  const selectedFeature: Feature | undefined = view?.kind === "feature"
    ? selectedProject?.features.find((feature) => feature.id === view.featureId)
    : undefined;

  // Fall back if another window or the TUI removes the current selection.
  useEffect(() => {
    if (!workspace) return;
    // Ignore query notifications that predate a newer cache write/navigation.
    if (workspace !== queryClient.getQueryData(SNAPSHOT_KEY)) return;
    if (!view || ((view.kind === "project" || view.kind === "feature") && !selectedProject)) {
      setView(projects[0] ? { kind: "project", projectId: projects[0].id } : null);
    } else if (view.kind === "feature" && !selectedFeature) {
      setView({ kind: "project", projectId: view.projectId });
    }
  }, [workspace, view, selectedProject, selectedFeature, projects, queryClient]);

  return { view, setView, projects, selectedProject, selectedFeature };
}
