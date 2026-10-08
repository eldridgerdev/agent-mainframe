// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { act as flush, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import PrTriagePanel from "../src/PrTriagePanel";
import type { PrComment, PrTriageAction, PrTriageView } from "../src/prTriageApi";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
afterEach(() => { cleanup(); vi.clearAllMocks(); vi.useRealTimers(); });

const target = { project_id: "project", feature_id: "feature" };
const base: PrTriageView = {
  workflow_id: "triage", revision: 1, target, feature_name: "Round totals", branch: "round-totals",
  stage: "pick", loading_pr: null, review: null, precall: null, reply: null, write_confirm: null,
  fix_targets: [], fix_draft: null, handoff: null,
  harnesses: ["claude", "codex"], default_harness: "claude", error: null, notice: null,
  picker: { include_closed: false, error: null, branch_pr: 12, loading: false, entries: [
    { number: 12, title: "Round invoice totals", author: "dev", head_ref: "round-totals", updated_at: "", is_draft: false, state: "OPEN", mine: true },
    { number: 9, title: "Currency formatting", author: "aria", head_ref: "currency", updated_at: "", is_draft: true, state: "OPEN", mine: false },
  ] },
};
const comment = (over: Partial<PrComment>): PrComment => ({
  id: 1, kind: "inline", review_state: null, author: "aria", is_bot: false, path: "invoice.ts", line: 8, side: "RIGHT",
  outdated: false, file_level: false, body: "Does this handle **negative** totals?", snippet: "Does this handle negative totals?",
  hunk: "@@ -6,2 +6,3 @@\n const tax = 1;\n+return round(total);", resolved: false, can_resolve: true, triage: "untriaged",
  local_note: null, actionable: true, local_finding: false, replies: [], investigation: null, ...over,
});
const review: PrTriageView = {
  ...base, stage: "review", picker: null, revision: 3,
  review: { number: 12, url: "", head_sha: "abcdef1234567", head_ref: "round-totals", branch_mismatch: null, fetched_at: "2026-10-05 12:00",
    open_count: 1, total: 2, hide_resolved: false, sort: "fetch_order", hidden_resolved: 0, conversation_start: null,
    investigating: null, investigating_harness: null,
    comments: [
      comment({ replies: [{ id: 3, author: "dev", body: "Looking.\n\n— posted via AMF", via_amf: true }] }),
      comment({ id: 2, author: "bot", is_bot: true, resolved: true, body: "Style nit", snippet: "Style nit", line: 12 }),
    ] },
};

type Handler = (action: PrTriageAction) => PrTriageView | null | Promise<PrTriageView | null>;
function backend(begin: PrTriageView, onAct: Handler, snapshot: () => PrTriageView = () => begin) {
  vi.mocked(invoke).mockImplementation(async (command: string, args?: unknown) => {
    if (command === "pr_triage_begin") return begin;
    if (command === "pr_triage_snapshot") return snapshot();
    if (command === "pr_triage_act") return onAct((args as { action: PrTriageAction }).action);
    throw new Error(command);
  });
}
const actions = () => vi.mocked(invoke).mock.calls
  .filter(([command]) => command === "pr_triage_act")
  .map(([, args]) => (args as { action: PrTriageAction; revision: number }));

it("lists pull requests, opens one, polls the comment fetch and browses threads", async () => {
  let snapshots = 0;
  const loading = { ...base, picker: null, stage: "loading" as const, loading_pr: 12, revision: 2 };
  backend(base, (action) => action.kind === "open" ? loading : base, () => ++snapshots < 2 ? loading : review);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  const branchPr = await screen.findByRole("button", { name: /Open PR #12 Round invoice totals/ });
  expect(within(branchPr).getByText("This branch")).toBeTruthy();
  expect(screen.getByText("Draft")).toBeTruthy();
  fireEvent.click(branchPr);
  expect(await screen.findByText(/Loading review comments for PR #12/)).toBeTruthy();
  await screen.findByRole("navigation", { name: "Review comments" }, { timeout: 3000 });
  expect(actions()[0]).toMatchObject({ action: { kind: "open", number: 12 }, revision: 1 });
  expect(screen.getByText("1 reply")).toBeTruthy();
  const detail = screen.getByRole("article", { name: "Selected comment" });
  expect(within(detail).getByText("negative")).toBeTruthy();
  expect(within(detail).getByText("via AMF")).toBeTruthy();
  expect(within(detail).getByLabelText("Diff context").textContent).toContain("+return round(total);");
  fireEvent.click(screen.getByRole("button", { name: /bot/ }));
  expect(within(screen.getByRole("article", { name: "Selected comment" })).getByText("Style nit")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Reopen thread…" })).toBeTruthy();
});

it("opens a pull request typed by number and shows list errors inline", async () => {
  backend({ ...base, picker: { ...base.picker!, entries: [], error: "gh: not logged in" } }, () => base);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  expect((await screen.findByRole("alert")).textContent).toContain("gh: not logged in");
  fireEvent.change(screen.getByRole("textbox", { name: "PR number" }), { target: { value: "65x4" } });
  fireEvent.click(screen.getByRole("button", { name: "Open by number" }));
  await waitFor(() => expect(actions()[0]?.action).toEqual({ kind: "open", number: 654 }));
});

it("polls a pull request list that is still being read", async () => {
  const reading = { ...base, picker: { ...base.picker!, entries: [], error: null, loading: true } };
  let snapshots = 0;
  backend(reading, () => base, () => ++snapshots < 2 ? reading : base);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  expect(await screen.findByText(/Loading pull requests/)).toBeTruthy();
  expect(screen.queryByText(/No open pull requests/)).toBeNull();
  expect(await screen.findByRole("button", { name: /Open PR #12/ }, { timeout: 3000 })).toBeTruthy();
});

it("keeps an investigation draft with its comment and says so elsewhere", async () => {
  backend(review, () => review);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Investigate…" }));
  // An untouched draft is dropped when the selection moves.
  fireEvent.click(screen.getByRole("button", { name: /bot/ }));
  expect((screen.getByRole("button", { name: "Investigate…" }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByText(/investigation request for another comment/)).toBeNull();

  fireEvent.click(screen.getByRole("button", { name: /aria/ }));
  fireEvent.click(screen.getByRole("button", { name: "Investigate…" }));
  fireEvent.change(screen.getByRole("textbox", { name: "What do you suspect? (optional)" }), { target: { value: "Rounds the wrong way" } });
  fireEvent.click(screen.getByRole("button", { name: /bot/ }));
  expect(screen.getByText(/investigation request for another comment is open/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Investigate…" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Go to it" }));
  expect((screen.getByRole("textbox", { name: "What do you suspect? (optional)" }) as HTMLTextAreaElement).value).toBe("Rounds the wrong way");
  fireEvent.click(screen.getByRole("button", { name: /bot/ }));
  fireEvent.click(screen.getByRole("button", { name: "Discard it" }));
  expect(screen.queryByText(/investigation request for another comment/)).toBeNull();
  expect((screen.getByRole("button", { name: "Investigate…" }) as HTMLButtonElement).disabled).toBe(false);
});

it("keeps the investigation text when continuing the AI call is refused", async () => {
  const precall = { ...review, revision: 4, precall: { title: "PR Triage: read-only investigation", harness: "Claude", preview: "Investigate.", viewing: false } };
  let state: PrTriageView = review;
  backend(review, (action) => {
    if (action.kind === "investigate") return state = precall;
    if (action.kind === "precall_confirm") throw { kind: "conflict", message: "Couldn't load PR #12: rate limited" };
    return state;
  }, () => state);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Investigate…" }));
  fireEvent.change(screen.getByRole("textbox", { name: "What do you suspect? (optional)" }), { target: { value: "Negative totals round up" } });
  fireEvent.click(screen.getByRole("button", { name: "Preview AI call" }));
  await screen.findByRole("alertdialog", { name: "Investigation AI call" });
  const draft = screen.getByRole("textbox", { name: "What do you suspect? (optional)" }) as HTMLTextAreaElement;
  expect(draft.disabled).toBe(true);
  fireEvent.click(screen.getByRole("button", { name: "Continue AI call" }));
  expect((await screen.findByRole("alert")).textContent).toContain("rate limited");
  expect(screen.getByRole("alertdialog", { name: "Investigation AI call" })).toBeTruthy();
  expect((screen.getByRole("textbox", { name: "What do you suspect? (optional)" }) as HTMLTextAreaElement).value).toBe("Negative totals round up");
});

it("investigates only after the pre-call notice is continued, once", async () => {
  const precall = { ...review, revision: 4, precall: { title: "PR Triage: read-only investigation", harness: "Codex", preview: "Investigate this PR review comment.", viewing: false } };
  let release: (v: PrTriageView) => void = () => {};
  backend(review, (action) => {
    if (action.kind === "investigate") return precall;
    if (action.kind === "precall_toggle_view") return { ...precall, precall: { ...precall.precall!, viewing: true } };
    if (action.kind === "precall_confirm") return new Promise((resolve) => { release = resolve; });
    return review;
  });
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Investigate…" }));
  fireEvent.change(screen.getByRole("combobox", { name: "Investigating harness" }), { target: { value: "codex" } });
  fireEvent.change(screen.getByRole("textbox", { name: "What do you suspect? (optional)" }), { target: { value: "Negative totals round up" } });
  fireEvent.click(screen.getByRole("button", { name: "Preview AI call" }));
  const notice = await screen.findByRole("alertdialog", { name: "Investigation AI call" });
  expect(notice.textContent).toContain("PR Triage: read-only investigation · Codex");
  expect(actions()[0].action).toEqual({ kind: "investigate", comment_id: 1, harness: "codex", note: "Negative totals round up", follow_up: null });
  fireEvent.click(within(notice).getByRole("button", { name: "View prompt" }));
  expect(await screen.findByText("Investigate this PR review comment.")).toBeTruthy();
  const cont = screen.getByRole("button", { name: "Continue AI call" });
  fireEvent.click(cont);
  fireEvent.click(cont);
  await flush(async () => { release({ ...review, revision: 6 }); });
  expect(actions().filter((a) => a.action.kind === "precall_confirm")).toHaveLength(1);
});

it("keeps the reply draft local, shows the exact posted text and posts only on confirmation", async () => {
  const replying = { ...review, revision: 4, reply: { comment_id: 1, kind: "not_needed" as const, seed: "", agent_drafted: false } };
  const confirming = { ...replying, revision: 5, write_confirm: { kind: "reply" as const, comment_id: 1, destination: "Reply in the inline review thread on PR #12", body: "Guarded upstream.\n\n— posted via AMF" } };
  backend(review, (action) => {
    if (action.kind === "start_reply") return replying;
    if (action.kind === "prepare_reply") return confirming;
    if (action.kind === "cancel_write") return replying;
    if (action.kind === "confirm_write") return { ...review, revision: 7 };
    return replying;
  });
  const onClose = vi.fn();
  render(<PrTriagePanel target={target} onClose={onClose} />);
  fireEvent.click(await screen.findByRole("button", { name: "Reply: not needed" }));
  const editor = await screen.findByRole("textbox", { name: /^Reply: not needed/ });
  fireEvent.change(editor, { target: { value: "Guarded upstream." } });
  // Closing with an unsent draft asks first.
  fireEvent.click(screen.getByRole("button", { name: "Close" }));
  expect(screen.getByText(/Discard your unsent fix, reply/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Keep editing" }));
  fireEvent.click(screen.getByRole("button", { name: "Review reply…" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Confirm GitHub write" });
  expect(confirm.textContent).toContain("Guarded upstream.\n\n— posted via AMF");
  expect(actions().some((a) => a.action.kind === "confirm_write")).toBe(false);
  fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect((screen.getByRole("textbox", { name: /^Reply: not needed/ }) as HTMLTextAreaElement).value).toBe("Guarded upstream.");
  fireEvent.click(screen.getByRole("button", { name: "Review reply…" }));
  fireEvent.click(await screen.findByRole("button", { name: "Post reply to GitHub" }));
  await waitFor(() => expect(actions().filter((a) => a.action.kind === "confirm_write")).toHaveLength(1));
  expect(actions().find((a) => a.action.kind === "prepare_reply")!.action).toEqual({ kind: "prepare_reply", comment_id: 1, body: "Guarded upstream." });
  expect(onClose).not.toHaveBeenCalled();
});

it("shows a refused write and reloads the current state without losing the draft", async () => {
  const replying = { ...review, revision: 4, reply: { comment_id: 1, kind: "done" as const, seed: "Done in `abc`.", agent_drafted: false } };
  const confirming = { ...replying, revision: 5, write_confirm: { kind: "reply" as const, comment_id: 1, destination: "Reply", body: "Done" } };
  let state: PrTriageView = review;
  backend(review, (action) => {
    if (action.kind === "start_reply") return state = replying;
    if (action.kind === "prepare_reply") return state = confirming;
    if (action.kind === "confirm_write") { state = { ...replying, revision: 6 }; throw { kind: "conflict", message: "PR #12 has new commits since these comments were loaded" }; }
    return state;
  }, () => state);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Reply: fixed" }));
  const editor = await screen.findByRole("textbox", { name: /^Reply: fixed/ }) as HTMLTextAreaElement;
  expect(editor.value).toBe("Done in `abc`.");
  fireEvent.change(editor, { target: { value: "Done in `abc`, with a test." } });
  fireEvent.click(screen.getByRole("button", { name: "Review reply…" }));
  fireEvent.click(await screen.findByRole("button", { name: "Post reply to GitHub" }));
  expect((await screen.findByRole("alert")).textContent).toContain("new commits");
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  expect((screen.getByRole("textbox", { name: /^Reply: fixed/ }) as HTMLTextAreaElement).value).toBe("Done in `abc`, with a test.");
});

it("asks before resolving a thread and offers cancelling a running investigation", async () => {
  const running = { ...review, revision: 4, review: { ...review.review!, investigating: 1, investigating_harness: "claude" as const } };
  backend(review, (action) => {
    if (action.kind === "request_resolve") return { ...review, revision: 4, write_confirm: { kind: "resolve" as const, comment_id: 1, destination: "Resolve the review thread on PR #12", body: null } };
    if (action.kind === "cancel_write") return running;
    return review;
  }, () => running);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  fireEvent.click(await screen.findByRole("button", { name: "Resolve thread…" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Confirm GitHub write" });
  expect(within(confirm).getByRole("button", { name: "Resolve thread on GitHub" })).toBeTruthy();
  fireEvent.click(within(confirm).getByRole("button", { name: "Cancel" }));
  expect(await screen.findByText(/Investigating comment read-only with claude/)).toBeTruthy();
  expect((screen.getByRole("button", { name: "Mark done (local)" }) as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByRole("button", { name: "Cancel investigation" })).toBeTruthy();
});

const fixTargets = [
  { target: { ...target, session_id: "claude-1" }, label: "Claude 1", harness: "claude" as const, stopped: true },
  { target: { ...target, session_id: "codex-1" }, label: "Codex 1", harness: "codex" as const, stopped: false },
];
const fixView: PrTriageView = { ...review, fix_targets: fixTargets };
const fixDraft: PrTriageView = { ...fixView, revision: 4,
  fix_draft: { comment_id: 1, target: fixTargets[1].target, prompt: "Fix the negative rounding concern. Verify the read-only findings." },
};

it("chooses an agent and confirms an edited fix as one unsent composer handoff", async () => {
  const handoff = vi.fn(); const close = vi.fn();
  backend(fixView, (action) => action.kind === "start_fix_draft" ? fixDraft : action.kind === "confirm_fix_draft"
    ? { ...fixView, handoff: { target: fixTargets[1].target, draft_prompt: action.prompt } } : fixView);
  render(<PrTriagePanel target={target} onClose={close} onHandoff={handoff} />);
  fireEvent.change(await screen.findByRole("combobox", { name: "Fix agent" }), { target: { value: "codex-1" } });
  fireEvent.click(screen.getByRole("button", { name: "Prepare fix…" }));
  const prompt = await screen.findByRole("textbox", { name: "Fix prompt" });
  expect(actions()[0].action).toEqual({ kind: "start_fix_draft", comment_id: 1, session_id: "codex-1" });
  expect(handoff).not.toHaveBeenCalled();
  fireEvent.change(prompt, { target: { value: "Edited fix instructions" } });
  const confirm = screen.getByRole("button", { name: "Open in agent composer" });
  fireEvent.click(confirm); fireEvent.click(confirm);
  await waitFor(() => expect(handoff).toHaveBeenCalledExactlyOnceWith({ target: fixTargets[1].target, draft_prompt: "Edited fix instructions" }));
  expect(close).toHaveBeenCalledTimes(1);
  expect(actions()[1]).toMatchObject({ revision: 4, action: { kind: "confirm_fix_draft", prompt: "Edited fix instructions" } });
});

it("keeps edited fix text after a stale-head refusal and guards cancellation", async () => {
  backend(fixDraft, (action) => { if (action.kind === "confirm_fix_draft") throw { kind: "conflict", message: "PR has new commits; your draft is kept" }; return fixView; }, () => ({ ...fixDraft, revision: 5 }));
  const handoff = vi.fn();
  render(<PrTriagePanel target={target} onClose={vi.fn()} onHandoff={handoff} />);
  const prompt = await screen.findByRole("textbox", { name: "Fix prompt" });
  fireEvent.change(prompt, { target: { value: "My edited instructions" } });
  fireEvent.click(screen.getByRole("button", { name: "Open in agent composer" }));
  await screen.findByText("PR has new commits; your draft is kept");
  expect((screen.getByRole("textbox", { name: "Fix prompt" }) as HTMLTextAreaElement).value).toBe("My edited instructions");
  expect(handoff).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Cancel fix draft" }));
  const discard = screen.getByRole("alertdialog", { name: "Discard edited fix prompt" });
  fireEvent.click(within(discard).getByRole("button", { name: "Keep editing" }));
  expect(screen.queryByRole("alertdialog", { name: "Discard edited fix prompt" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Cancel fix draft" }));
  fireEvent.click(screen.getByRole("button", { name: "Discard fix prompt" }));
  await waitFor(() => expect(screen.queryByRole("textbox", { name: "Fix prompt" })).toBeNull());
  expect(actions().at(-1)?.action).toEqual({ kind: "cancel_fix_draft" });
});

it("blocks other triage actions during a fix draft and protects edited text on close", async () => {
  backend(fixDraft, () => fixView);
  const close = vi.fn();
  render(<PrTriagePanel target={target} onClose={close} onHandoff={vi.fn()} />);
  const prompt = await screen.findByRole("textbox", { name: "Fix prompt" });
  expect((screen.getByRole("button", { name: "Refresh comments" }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Investigate…" }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(prompt, { target: { value: "Unsaved fix" } });
  fireEvent.keyDown(document, { key: "Escape" });
  expect(await screen.findByText(/Discard your unsent fix, reply or investigation text/)).toBeTruthy();
  expect(close).not.toHaveBeenCalled();
});

it("explains how to get a fix target when a feature has no agent sessions", async () => {
  backend(review, () => review);
  render(<PrTriagePanel target={target} onClose={vi.fn()} onHandoff={vi.fn()} />);
  expect(await screen.findByText("Add an agent session to this feature to prepare a fix draft.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Prepare fix…" })).toBeNull();
});

it("previews the receipt-bearing prompt and requires explicit send without closing triage", async () => {
  const close = vi.fn(); const handoff = vi.fn();
  const preview = { ...fixDraft, revision: 5, fix_draft: { ...fixDraft.fix_draft!, submission_prompt: "Edited instruction\n\namf reply-draft --request-id current" } };
  backend(fixDraft, action => action.kind === "prepare_fix_submission" ? preview : { ...fixView, revision: 6, notice: "Fix sent to the agent." });
  render(<PrTriagePanel target={target} onClose={close} onHandoff={handoff} />);
  const prompt = await screen.findByRole("textbox", { name: "Fix prompt" });
  fireEvent.change(prompt, { target: { value: "Edited instruction" } });
  fireEvent.click(screen.getByRole("button", { name: "Preview send to agent…" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Send fix to agent" });
  expect(confirm.textContent).toContain("amf reply-draft --request-id current");
  expect(actions()[0].action).toEqual({ kind: "prepare_fix_submission", prompt: "Edited instruction" });
  expect((prompt as HTMLTextAreaElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: "Open in agent composer" }) as HTMLButtonElement).disabled).toBe(true);
  const send = within(confirm).getByRole("button", { name: "Send fix to agent" });
  fireEvent.click(send); fireEvent.click(send);
  await screen.findByText("Fix sent to the agent.");
  expect(actions().filter(a => a.action.kind === "confirm_fix_submission")).toHaveLength(1);
  expect(close).not.toHaveBeenCalled(); expect(handoff).not.toHaveBeenCalled();
});

it("cancels submission preview while retaining the edited fix prompt", async () => {
  const preview = { ...fixDraft, fix_draft: { ...fixDraft.fix_draft!, submission_prompt: "Exact prompt with receipt" } };
  backend(fixDraft, action => action.kind === "prepare_fix_submission" ? preview : fixDraft);
  render(<PrTriagePanel target={target} onClose={vi.fn()} />);
  const prompt = await screen.findByRole("textbox", { name: "Fix prompt" });
  fireEvent.change(prompt, { target: { value: "Keep my edited instruction" } });
  fireEvent.click(screen.getByRole("button", { name: "Preview send to agent…" }));
  fireEvent.click(await screen.findByRole("button", { name: "Back to fix prompt" }));
  await waitFor(() => expect(screen.queryByRole("alertdialog", { name: "Send fix to agent" })).toBeNull());
  expect((prompt as HTMLTextAreaElement).value).toBe("Keep my edited instruction");
  expect((prompt as HTMLTextAreaElement).disabled).toBe(false);
  expect(actions().at(-1)?.action.kind).toBe("cancel_fix_submission");
});
