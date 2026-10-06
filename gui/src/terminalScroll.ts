// Scroll-gesture routing for TerminalPane. Pure helpers, so the rules for
// which keys scroll and how wheel deltas become line steps are testable
// without a real xterm.js (see `src/gui_terminal.rs`'s module docs for the
// backend half of the design).

/** One full-pane update from `gui_terminal::TerminalFrame`. */
export interface TerminalFrame {
  replay: string;
  /** The program is on the alternate screen: tmux has no history of it. */
  alternate_screen: boolean;
  /** The program asked for mouse reporting and scrolls its own view. */
  mouse_reporting: boolean;
}

/** A read-only history snapshot from `gui_terminal::TerminalHistory`. */
export interface TerminalHistory {
  replay: string;
  earlier_lines: number;
  alternate_screen: boolean;
}

/**
 * Lines xterm keeps above the screen. At least `SCROLLBACK_HISTORY_LINES`
 * (`src/tmux.rs`), so loading a history snapshot never trims its top.
 */
export const SCROLLBACK_LINES = 10_000;

/** Matches `gui_terminal::MAX_WHEEL_STEPS`. */
export const MAX_WHEEL_STEPS = 10;

export type ScrollKeyAction = "pageUp" | "pageDown" | "top" | "latest";

/**
 * The scroll action a key press means, or `null` for a key that belongs to
 * the program. From the live view only the Shift-modified keys scroll -- the
 * terminal convention -- so PageUp, Home, End and Esc still reach programs
 * that use them (Neovim, OpenCode, an agent's Esc-to-interrupt). While the
 * user is reading earlier output those keys scroll instead, and Esc returns
 * to live output rather than interrupting the agent.
 */
export function scrollKeyAction(event: KeyboardEvent, reading: boolean): ScrollKeyAction | null {
  if (event.ctrlKey || event.altKey || event.metaKey) return null;
  const scrolls = event.shiftKey || reading;
  switch (event.key) {
    case "PageUp":
      return scrolls ? "pageUp" : null;
    case "PageDown":
      return scrolls ? "pageDown" : null;
    case "Home":
      return scrolls ? "top" : null;
    case "End":
      return scrolls ? "latest" : null;
    case "Escape":
      return reading && !event.shiftKey ? "latest" : null;
    default:
      return null;
  }
}

/**
 * Turns wheel deltas into whole line steps (negative is up), carrying the
 * fractional remainder so a trackpad's many small pixel deltas add up the
 * way a mouse wheel's notches do. A change of direction drops the remainder.
 */
export class WheelAccumulator {
  private remainder = 0;

  steps(event: Pick<WheelEvent, "deltaY" | "deltaMode">, cellHeight: number, rows: number): number {
    const lines = event.deltaMode === 1
      ? event.deltaY
      : event.deltaMode === 2
        ? event.deltaY * rows
        : event.deltaY / (cellHeight > 0 ? cellHeight : 16);
    if (lines === 0) return 0;
    if (Math.sign(lines) !== Math.sign(this.remainder)) this.remainder = 0;
    this.remainder += lines;
    const steps = Math.trunc(this.remainder);
    this.remainder -= steps;
    // `Math.trunc` of a small negative is -0; callers compare with 0.
    return steps === 0 ? 0 : steps;
  }
}
