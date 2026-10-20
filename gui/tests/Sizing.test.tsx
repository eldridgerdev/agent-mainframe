// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import ThemePicker, { useGuiTheme } from "../src/ThemePicker";
import { readSizingPreference, resetSizingForTest, setSizingPreference } from "../src/sizing";

const zoom = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
vi.mock("@tauri-apps/api/webview", () => ({ getCurrentWebview: () => ({ setZoom: zoom }) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(async () => ({ themes: [], errors: [], directory: "" })) }));
let client: QueryClient;
function Harness() { return <ThemePicker appearance={useGuiTheme()} onClose={() => {}} />; }
function mount() {
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return render(<QueryClientProvider client={client}><Harness /></QueryClientProvider>);
}
beforeEach(() => {
  window.localStorage.clear(); resetSizingForTest();
  vi.stubGlobal("__TAURI_INTERNALS__", {});
  zoom.mockReset().mockResolvedValue(undefined);
});
afterEach(() => { cleanup(); client?.clear(); resetSizingForTest(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("applies desktop zoom immediately, restores saved sizes on mount and resets them", async () => {
  setSizingPreference({ interfaceScale: 150, terminalFontSize: 19 });
  resetSizingForTest();
  mount();
  await waitFor(() => expect(zoom).toHaveBeenLastCalledWith(1.5));
  expect((screen.getByLabelText("Terminal text size") as HTMLSelectElement).value).toBe("19");
  fireEvent.change(screen.getByLabelText("Interface size"), { target: { value: "125" } });
  await waitFor(() => expect(zoom).toHaveBeenLastCalledWith(1.25));
  fireEvent.change(screen.getByLabelText("Terminal text size"), { target: { value: "22" } });
  resetSizingForTest();
  expect(readSizingPreference()).toEqual({ interfaceScale: 125, terminalFontSize: 22 });
  fireEvent.click(screen.getByText("Reset sizes"));
  await waitFor(() => expect(zoom).toHaveBeenLastCalledWith(1));
  expect(readSizingPreference()).toEqual({ interfaceScale: 100, terminalFontSize: 13 });
});

it("adopts another window's sizing without echoing writes, including storage clear", async () => {
  mount();
  await waitFor(() => expect(zoom).toHaveBeenCalledWith(1));
  const setItem = vi.spyOn(Storage.prototype, "setItem");
  act(() => window.dispatchEvent(new StorageEvent("storage", {
    key: "amf.gui.sizing", newValue: JSON.stringify({ interfaceScale: 175, terminalFontSize: 21 }),
  })));
  await waitFor(() => expect(zoom).toHaveBeenLastCalledWith(1.75));
  expect((screen.getByLabelText("Terminal text size") as HTMLSelectElement).value).toBe("21");
  expect(setItem).not.toHaveBeenCalled();
  act(() => window.dispatchEvent(new StorageEvent("storage", { key: null })));
  await waitFor(() => expect(zoom).toHaveBeenLastCalledWith(1));
});

it("uses safe defaults for corrupt or out-of-range settings and works without storage", () => {
  for (const raw of ["broken", "null", '{"interfaceScale":900,"terminalFontSize":-4}']) {
    window.localStorage.setItem("amf.gui.sizing", raw);
    resetSizingForTest();
    expect(readSizingPreference()).toEqual({ interfaceScale: 100, terminalFontSize: 13 });
  }
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("disabled"); });
  setSizingPreference({ interfaceScale: 125, terminalFontSize: 17 });
  expect(readSizingPreference()).toEqual({ interfaceScale: 125, terminalFontSize: 17 });
});

it("reports native zoom failures and recovers on the next change", async () => {
  zoom.mockRejectedValueOnce(new Error("unavailable"));
  mount();
  expect((await screen.findByRole("alert")).textContent).toContain("Could not apply interface size");
  fireEvent.change(screen.getByLabelText("Interface size"), { target: { value: "150" } });
  await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
  expect(zoom).toHaveBeenLastCalledWith(1.5);
});

it("keeps the newest size when native zoom requests are still pending", async () => {
  let complete!: () => void;
  zoom.mockImplementationOnce(() => new Promise<void>((resolve) => { complete = resolve; }));
  mount();
  await waitFor(() => expect(zoom).toHaveBeenCalledWith(1));
  fireEvent.change(screen.getByLabelText("Interface size"), { target: { value: "125" } });
  fireEvent.change(screen.getByLabelText("Interface size"), { target: { value: "200" } });
  expect(zoom).toHaveBeenCalledTimes(1);
  await act(async () => complete());
  await waitFor(() => expect(zoom).toHaveBeenLastCalledWith(2));
  expect(zoom.mock.calls.map(([factor]) => factor)).toEqual([1, 2]);
});
