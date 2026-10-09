// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import PrMarkdown, { PrDescription } from "../src/PrMarkdown";
import type { GithubAccessCheck, ImageData } from "../src/screenshotsApi";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); });
const image = { data_url: "data:image/png;base64,AA==", width: 4, height: 3 };
const props = { workflowId: "triage", identity: "triage:12:head", onOpenImage: vi.fn() };

it("renders Markdown/reference/HTML images where they occur and sanitizes active HTML", async () => {
  vi.mocked(invoke).mockResolvedValue(image);
  const source = `Before ![Inline](./ready.png) after\n\n![Reference][picture]\n\n[picture]: https://github.com/user-attachments/assets/reference\n\n<img alt="HTML upload" src="https://github.com/user-attachments/assets/html" onerror="alert(1)" />\n\n\`![Code](https://example.com/code.png)\`\n\n\`\`\`\n<img src="https://example.com/fenced.png">\n\`\`\`\n\n<!-- <img src="https://example.com/comment.png"> -->\n<script><img src="https://example.com/script.png"></script>\n<iframe src="https://example.com/frame"></iframe>`;
  const { container } = render(<PrMarkdown {...props} source={source} />);
  expect(await screen.findByRole("img", { name: "HTML upload" })).toBeTruthy();
  expect(screen.getByRole("img", { name: "Reference" })).toBeTruthy();
  expect(screen.getByRole("img", { name: "Inline" })).toBeTruthy();
  expect(container.querySelector("p")?.textContent).toBe("Before  after");
  expect(container.querySelectorAll("img")).toHaveLength(3);
  expect(container.querySelector("script,iframe,[onerror]")).toBeNull();
  expect(vi.mocked(invoke).mock.calls.every(([command, args]) => command === "screenshots_inline_image"
    && !(args as { source: string }).source.includes("example.com"))).toBe(true);
});

it("rejects delayed images from a previous PR and retains neighboring image reads", async () => {
  let complete!: (value: ImageData) => void;
  const pending = new Promise<ImageData>((resolve) => { complete = resolve; });
  vi.mocked(invoke).mockImplementation((_, args) => (args as { workflowId: string }).workflowId === "old"
    ? pending : Promise.resolve(image));
  const view = render(<PrMarkdown {...props} workflowId="old" identity="old" source="![Old](./old.png)" />);
  view.rerender(<PrMarkdown {...props} source="![New](./one.png) ![Neighbor](./two.png)" />);
  expect(await screen.findByRole("img", { name: "New" })).toBeTruthy();
  expect(await screen.findByRole("img", { name: "Neighbor" })).toBeTruthy();
  complete(image);
  await waitFor(() => expect(screen.queryByRole("img", { name: "Old" })).toBeNull());
});

it("retries a failed image in place", async () => {
  vi.mocked(invoke).mockRejectedValueOnce({ message: "Attachment access expired" }).mockResolvedValue(image);
  render(<PrMarkdown {...props} source="![Upload](https://github.com/user-attachments/assets/file)" />);
  expect(await screen.findByRole("alert")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Retry image" }));
  expect(await screen.findByRole("img", { name: "Upload" })).toBeTruthy();
});

it("checks the account, PR and image independently without treating an image failure as a login failure", async () => {
  const checks: GithubAccessCheck[] = [
    { name: "GitHub account", passed: true, detail: "Authenticated as coworker" },
    { name: "PR access", passed: true, detail: "Can read this PR" },
    { name: "Image access", passed: false, detail: "Authentication and PR access passed, but image retrieval failed" },
  ];
  vi.mocked(invoke).mockImplementation((command) => command === "screenshots_check_access"
    ? Promise.resolve(checks) : Promise.reject({ message: "Image source returned HTML" }));
  render(<PrMarkdown {...props} source="![Upload](https://github.com/user-attachments/assets/file)" />);
  fireEvent.click(await screen.findByRole("button", { name: "Check access" }));
  expect(await screen.findByText(/Passed — GitHub account: Authenticated as coworker/)).toBeTruthy();
  expect(screen.getByText(/Passed — PR access/)).toBeTruthy();
  expect(screen.getByText(/Failed — Image access/)).toBeTruthy();
  expect(vi.mocked(invoke)).toHaveBeenCalledWith("screenshots_check_access", {
    workflowId: "triage", source: "https://github.com/user-attachments/assets/file",
  });
});

it("rejects delayed access checks after changing PR and clears their results on image retry", async () => {
  let complete!: (value: GithubAccessCheck[]) => void;
  const pending = new Promise<GithubAccessCheck[]>((resolve) => { complete = resolve; });
  vi.mocked(invoke).mockImplementation((command, args) => command === "screenshots_check_access"
    ? (args as { workflowId: string }).workflowId === "old" ? pending : Promise.reject({ kind: "internal", message: "Check timed out" })
    : Promise.reject({ message: "Image source returned HTML" }));
  const view = render(<PrMarkdown {...props} workflowId="old" identity="old" source="![Old](./old.png)" />);
  fireEvent.click(await screen.findByRole("button", { name: "Check access" }));
  expect(screen.getByRole("button", { name: "Checking access…" }).hasAttribute("disabled")).toBe(true);
  view.rerender(<PrMarkdown {...props} source="![New](./new.png)" />);
  fireEvent.click(await screen.findByRole("button", { name: "Check access" }));
  expect(await screen.findByText(/Failed — Access check: Check timed out/)).toBeTruthy();
  complete([{ name: "GitHub account", passed: true, detail: "Old account" }]);
  await waitFor(() => expect(screen.queryByText(/Old account/)).toBeNull());
  fireEvent.click(screen.getByRole("button", { name: "Retry image" }));
  await waitFor(() => expect(screen.queryByText(/Check timed out/)).toBeNull());
});

it("does not apply an old description after switching PRs", async () => {
  let complete!: (value: string) => void;
  const old = new Promise<string>((resolve) => { complete = resolve; });
  vi.mocked(invoke).mockImplementation((command, args) => command === "screenshots_pr_document"
    ? (args as { workflowId: string }).workflowId === "old" ? old : Promise.resolve("New description")
    : Promise.resolve(image));
  const view = render(<PrDescription {...props} workflowId="old" identity="old" />);
  view.rerender(<PrDescription {...props} />);
  expect(await screen.findByText("New description")).toBeTruthy();
  complete("Old description");
  await waitFor(() => expect(screen.queryByText("Old description")).toBeNull());
});
