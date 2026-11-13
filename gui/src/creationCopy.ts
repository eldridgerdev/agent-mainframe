/** One wording for the options every feature-creation form offers, so New
 * feature, a TODO's new feature and presets describe them the same way. */

export const PLAN_HINTS = {
  none: "Start the agent right away.",
  quick: "A short triage: a question or two, then a plan only if the task needs one.",
  full: "A guided interview that produces a reviewed plan before the agent starts.",
} as const;

/** `review` keeps per-file notes (AMF's review instructions ask the agent to
 * append to .claude/review-notes.md). */
export const REVIEW_HINT = "The agent keeps short notes on each file it changes, for you to read when you review.";

/** Shown wherever Design Review is offered for a harness that cannot run it.
 * Mirrors `DESIGN_REVIEW_CLAUDE_ONLY` in src/design_review/support.rs. */
export const DESIGN_REVIEW_CLAUDE_ONLY = "Claude only for now";

export const DESIGN_REVIEW_HINT = "The agent records each design decision as it works, so you can review why the code is built this way. Not with Review.";

/** Why Design Review cannot be turned on for `agent`, or null when it can. */
export function designReviewUnavailableReason(agent: string | null | undefined): string | null {
  return agent === "claude" ? null : DESIGN_REVIEW_CLAUDE_ONLY;
}

/** AMF passes `--chrome` to Claude Code launches only. */
export const CHROME_HINT = "Lets the agent use a Chrome browser. Claude Code only.";
