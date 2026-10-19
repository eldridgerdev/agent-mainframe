// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { useCreationHarness } from "../src/useCreationHarness";
import { creationChoices } from "./creationChoices";
import type { ProjectHarnessChoices } from "../src/api";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const clients: QueryClient[] = [];
afterEach(() => { cleanup(); clients.splice(0).forEach(client => client.clear()); vi.resetAllMocks(); });
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: Infinity } } });
  clients.push(client);
  const hook = renderHook(({ project }) => useCreationHarness(project), { initialProps: { project: "one" },
    wrapper: ({ children }) => <QueryClientProvider client={client}>{children}</QueryClientProvider> });
  return { ...hook, client };
}

it("uses the backend preferred default and retains it across refreshes", async () => {
  vi.mocked(invoke).mockResolvedValue(creationChoices(["claude", "codex"], "codex"));
  const { result, client } = mount(); expect(result.current.ready).toBe(false);
  await waitFor(() => expect(result.current.agent).toBe("codex"));
  vi.mocked(invoke).mockResolvedValue(creationChoices(["claude", "codex"], "claude"));
  await act(async () => { await client.invalidateQueries({ queryKey: ["project-harness-choices"] }); });
  expect(result.current.agent).toBe("codex"); expect(result.current.explicit).toBe(false);
});

it("never overwrites manual or preset intent with a late default response", async () => {
  let complete!: (view: ProjectHarnessChoices) => void;
  vi.mocked(invoke).mockImplementation(() => new Promise(resolve => { complete = resolve; }));
  const { result } = mount();
  act(() => result.current.choose("opencode"));
  await act(async () => complete(creationChoices(["claude", "opencode"], "claude")));
  await waitFor(() => expect(result.current.ready).toBe(true));
  expect(result.current.agent).toBe("opencode");
});

it("initializes an untouched new destination from an immediately cached successful response", async () => {
  vi.mocked(invoke).mockImplementation(async (_, args) => creationChoices(["claude", "pi"], (args as { projectId: string }).projectId === "one" ? "claude" : "pi"));
  const { result, client, rerender } = mount(); await waitFor(() => expect(result.current.ready).toBe(true));
  client.setQueryData(["project-harness-choices", "two"], creationChoices(["pi"], "pi"));
  rerender({ project: "two" }); await waitFor(() => expect(result.current.agent).toBe("pi"));
  expect(result.current.explicit).toBe(false);
});

it("keeps explicit unavailable intent visible and blocked across project changes and refreshes", async () => {
  vi.mocked(invoke).mockImplementation(async (_, args) => creationChoices((args as { projectId: string }).projectId === "one" ? ["claude", "codex"] : ["pi"]));
  const { result, rerender, client } = mount(); await waitFor(() => expect(result.current.ready).toBe(true));
  act(() => result.current.choose("codex")); rerender({ project: "two" });
  await waitFor(() => expect(result.current.offered).toEqual(["pi"]));
  expect(result.current.agent).toBe("codex"); expect(result.current.ready).toBe(false);
  await act(async () => { await client.invalidateQueries({ queryKey: ["project-harness-choices"] }); });
  expect(result.current.agent).toBe("codex"); expect(result.current.ready).toBe(false);
  act(() => result.current.choose("pi")); expect(result.current.ready).toBe(true);
});

it("keeps a default that becomes unavailable blocked rather than silently substituting", async () => {
  vi.mocked(invoke).mockResolvedValue(creationChoices(["claude", "codex"], "codex"));
  const { result, client } = mount(); await waitFor(() => expect(result.current.ready).toBe(true));
  vi.mocked(invoke).mockResolvedValue(creationChoices(["claude"]));
  await act(async () => { await client.invalidateQueries({ queryKey: ["project-harness-choices"] }); });
  await waitFor(() => expect(result.current.offered).toEqual(["claude"]));
  expect(result.current.agent).toBe("codex"); expect(result.current.ready).toBe(false);
});
