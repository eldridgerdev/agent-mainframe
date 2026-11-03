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

/** AMF passes `--chrome` to Claude Code launches only. */
export const CHROME_HINT = "Lets the agent use a Chrome browser. Claude Code only.";
