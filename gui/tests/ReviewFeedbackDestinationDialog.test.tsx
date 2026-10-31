// @vitest-environment jsdom
import { StrictMode } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import ReviewFeedbackDestinationDialog from "../src/ReviewFeedbackDestinationDialog";
import { reviewDestinationCatalog, reviewDestinationPreview, type ReviewDestinationCatalog, type ReviewDestinationPreview, type ReviewView } from "../src/api";
vi.mock("../src/api", async importOriginal => ({ ...await importOriginal<object>(), reviewDestinationCatalog: vi.fn(), reviewDestinationPreview: vi.fn() }));
afterEach(() => { cleanup(); vi.resetAllMocks(); });
const view = { workflow_id: "review", revision: 3 } as ReviewView;
const target = { project_id: "p", feature_id: "f", session_id: "s" };
const catalog: ReviewDestinationCatalog = { sessions: [{ target, label: "Source / Claude", current: true, stopped: true, unavailable: null }], agents: [{ agent: "claude", label: "Claude", unavailable: null }], source_path: "/source", source_branch: "feature", source_head: "abc123", default_branch: "feature-review-fixes", companion_available: true, hook_prompt: { title: "Setup", options: ["offline", "full"] }, selected: null };
const preview: ReviewDestinationPreview = { revision: "stamp", label: "Source / Claude", source_path: "/source", destination_path: "/source", base_branch: null, base_sha: null, creates_session: false, creates_companion: false, resource_warning: null, hook_prompt: null };
function mount(onAct = vi.fn(async () => true), onClose = vi.fn()) {
  vi.mocked(reviewDestinationCatalog).mockResolvedValue(catalog);
  vi.mocked(reviewDestinationPreview).mockResolvedValue(preview);
  const component = render(<ReviewFeedbackDestinationDialog view={view} error={null} onAct={onAct} onClose={onClose} />);
  return { onAct, onClose, update: (next: ReviewView) => component.rerender(<ReviewFeedbackDestinationDialog view={next} error={null} onAct={onAct} onClose={onClose} />) };
}
async function ready() { await screen.findByRole("combobox", { name: "Destination type" }); }
it("only prepares after an explicit fresh preview", async () => {
  const { onAct, onClose } = mount(); await ready();
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Preview destination" }));
  await screen.findByRole("button", { name: "Prepare destination" });
  expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Prepare destination" }));
  await waitFor(() => expect(onAct).toHaveBeenCalledWith({ kind: "prepare_destination", choice: { kind: "existing", target }, preview_revision: "stamp", approved: false }));
  expect(onClose).toHaveBeenCalledTimes(1);
});
it("keeps companion edits behind discard confirmation without creating resources", async () => {
  const { onAct, onClose } = mount(); await ready();
  fireEvent.change(screen.getByRole("combobox", { name: "Destination type" }), { target: { value: "companion" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Companion branch" }), { target: { value: "my-fixes" } });
  fireEvent.click(screen.getByRole("button", { name: "Cancel destination selection" }));
  expect(screen.getByRole("alertdialog", { name: "Discard destination choices" })).toBeTruthy();
  expect(onClose).not.toHaveBeenCalled(); expect(onAct).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  expect((screen.getByRole("textbox", { name: "Companion branch" }) as HTMLInputElement).value).toBe("my-fixes");
});
it("guards delayed previews, destination switches, closing and double submit synchronously", async () => {
  let resolve!: (value: ReviewDestinationPreview) => void;
  const { onAct, onClose } = mount(); await ready();
  vi.mocked(reviewDestinationPreview).mockImplementation(() => new Promise(done => { resolve = done; }));
  fireEvent.click(screen.getByRole("button", { name: "Preview destination" }));
  fireEvent.change(screen.getByRole("combobox", { name: "Destination type" }), { target: { value: "existing" } });
  fireEvent.keyDown(window, { key: "Escape" });
  expect((screen.getByRole("combobox", { name: "Destination type" }) as HTMLSelectElement).value).toBe("current");
  expect(onClose).not.toHaveBeenCalled();
  resolve(preview); await screen.findByRole("button", { name: "Prepare destination" });
  let done!: (value: boolean) => void;
  onAct.mockImplementation(() => new Promise(resolve => { done = resolve; }));
  fireEvent.click(screen.getByRole("button", { name: "Prepare destination" }));
  fireEvent.click(screen.getByRole("button", { name: "Prepare destination" }));
  expect(onAct).toHaveBeenCalledTimes(1);
  done(false); await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("retained"));
});
it("requires resource approval and invalidates a preview after a review revision change", async () => {
  const { onAct, update } = mount(); await ready();
  vi.mocked(reviewDestinationPreview).mockResolvedValue({ ...preview, creates_session: true, resource_warning: "Low available memory" });
  fireEvent.click(screen.getByRole("button", { name: "Preview destination" }));
  const button = await screen.findByRole("button", { name: "Prepare destination" });
  expect((button as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("checkbox", { name: "Proceed despite this resource warning" }));
  expect((button as HTMLButtonElement).disabled).toBe(false);
  update({ ...view, revision: 4 });
  expect((screen.getByRole("button", { name: "Prepare destination" }) as HTMLButtonElement).disabled).toBe(true);
  expect(onAct).not.toHaveBeenCalled();
});
it("retains edited choices after loading failure", async () => {
  mount(); await ready();
  fireEvent.change(screen.getByRole("combobox", { name: "Destination type" }), { target: { value: "companion" } });
  fireEvent.change(screen.getByRole("textbox", { name: "Companion branch" }), { target: { value: "retained" } });
  vi.mocked(reviewDestinationCatalog).mockRejectedValueOnce({ message: "Project disappeared" });
  fireEvent.click(screen.getByRole("button", { name: "Refresh destinations" }));
  await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Project disappeared"));
  expect((screen.getByRole("textbox", { name: "Companion branch" }) as HTMLInputElement).value).toBe("retained");
});

it("replays initial deferred catalog loading safely in StrictMode", async () => {
  let stale!: (value: ReviewDestinationCatalog) => void;
  let fresh!: (value: ReviewDestinationCatalog) => void;
  vi.mocked(reviewDestinationCatalog).mockImplementationOnce(() => new Promise(done => { stale = done; }))
    .mockImplementationOnce(() => new Promise(done => { fresh = done; }));
  render(<StrictMode><ReviewFeedbackDestinationDialog view={view} error={null} onAct={vi.fn()} onClose={vi.fn()} /></StrictMode>);
  expect(reviewDestinationCatalog).toHaveBeenCalledTimes(2);
  fresh(catalog); await ready();
  expect((screen.getByRole("combobox", { name: "Destination type" }) as HTMLSelectElement).disabled).toBe(false);
  stale({ ...catalog, source_path: "/stale", default_branch: "stale-branch" });
  fireEvent.change(screen.getByRole("combobox", { name: "Destination type" }), { target: { value: "companion" } });
  await waitFor(() => expect((screen.getByRole("textbox", { name: "Companion branch" }) as HTMLInputElement).value).toBe(catalog.default_branch));
});
