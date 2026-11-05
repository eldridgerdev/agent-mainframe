import { useEffect, useState } from "react";
import type { ModeSlug, Project } from "./api";

export function useLaunchedModeState(projects: Project[] | undefined) {
  const [launchedModes, setLaunchedModes] = useState<Record<string, ModeSlug>>({});

  // Session mode overrides matter only while their feature is running.
  useEffect(() => {
    const running = new Set((projects ?? []).flatMap((project) =>
      project.features.filter((feature) => feature.status !== "stopped").map((feature) => feature.id)));
    setLaunchedModes((previous) =>
      Object.keys(previous).every((id) => running.has(id))
        ? previous
        : Object.fromEntries(Object.entries(previous).filter(([id]) => running.has(id))));
  }, [projects]);

  return { launchedModes, setLaunchedModes };
}
