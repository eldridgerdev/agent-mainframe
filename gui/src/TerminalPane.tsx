import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { asGuiError } from "./api";
import { Icon } from "./ui";
import {
  MAX_WHEEL_STEPS, SCROLLBACK_LINES, WheelAccumulator, scrollKeyAction,
  type TerminalFrame, type TerminalHistory,
} from "./terminalScroll";

// Matches the app's dark surface so the terminal reads as part of the window
// in either colour scheme.
const TERMINAL_THEME = {
  background: "#0d1014",
  foreground: "#d7dce2",
  cursor: "#9aa7ff",
  selectionBackground: "#2c3550",
  black: "#1b1f25",
  red: "#f47067",
  green: "#57c27a",
  yellow: "#d9b34c",
  blue: "#6cb6ff",
  magenta: "#c79bf5",
  cyan: "#56c5d0",
  white: "#c9d1d9",
  brightBlack: "#636e7b",
  brightRed: "#ff938a",
  brightGreen: "#78d796",
  brightYellow: "#e8c66b",
  brightBlue: "#8ecbff",
  brightMagenta: "#dcbdfb",
  brightCyan: "#7fdbe3",
  brightWhite: "#f0f3f6",
};

// Installed Nerd Fonts first, then common coding fonts. The bundled
// "AMF Symbols" fonts (styles.css) supply Nerd Font icons, powerline and
// technical symbols to whichever of those lacks them, so agent status lines
// and tool output don't render as boxes.
const SYMBOL_FONTS = ["AMF Symbols", "AMF Symbols 2", "AMF Symbols 3"];
const SYMBOL_SAMPLE = "\ue0b0\u23bf\u23f5";

const TERMINAL_FONT = [
  '"JetBrainsMono Nerd Font Mono"',
  '"FiraCode Nerd Font Mono"',
  '"JetBrains Mono"',
  '"Cascadia Code"',
  '"Fira Code"',
  "Menlo",
  "Consolas",
  '"DejaVu Sans Mono"',
  '"Ubuntu Mono"',
  ...SYMBOL_FONTS.map((family) => `"${family}"`),
  "monospace",
].join(", ");

// The browser only fetches a web font when layout asks it for one of the
// font's glyphs, and xterm's rows never trigger that for these fallbacks,
// so they stayed unloaded and icons rendered as boxes. Request them
// explicitly; the returned promise settles once every one is usable.
function loadSymbolFonts(): Promise<unknown> {
  if (typeof document === "undefined" || !document.fonts?.load) return Promise.resolve();
  return Promise.all(SYMBOL_FONTS.map((family) =>
    document.fonts.load(`13px "${family}"`, SYMBOL_SAMPLE).catch(() => undefined)));
}

interface SessionTarget {
  project_id: string;
  feature_id: string;
  session_id: string;
}

interface AttachTerminalResponse {
  key: string;
  // Identifies this attachment among any others for the same session, so
  // our detach can never remove a newer pane's handle.
  generation: number;
  initial: TerminalFrame;
}

/** What the scroll-back overlay shows; the effect keeps the authoritative state. */
type ScrollView =
  | { mode: "live" }
  | { mode: "loading" }
  | { mode: "history"; newOutput: boolean };

const FULL_SCREEN_NOTICE =
  "This program fills the screen and keeps no scrollback here. Use its own keys to scroll.";
const NOTICE_MS = 4000;

// Task 6 ("Implement the GUI terminal transport"): the actual transport
// correctness (Unicode, escape sequences, high output volume, cleanup) is
// verified against real tmux sessions in `src/gui_terminal.rs`'s own tests,
// not here -- this component's job is proving the wiring works end to end
// through xterm.js and Tauri's IPC/event boundary, which those Rust-only
// tests cannot exercise.
//
// Scrolling: every live update replaces the whole screen, so xterm never
// builds scrollback of its own. Scrolling up (wheel, trackpad, Shift+PageUp)
// instead loads a read-only snapshot of tmux's history into xterm's
// scrollback and freezes there -- live updates are set aside, not drawn --
// until the user scrolls back to the bottom, presses Esc/Shift+End or
// "Jump to latest", or types. Nothing reaches the program except, for a
// full-screen program that asked for mouse reporting, the wheel reports a
// native terminal would send it. See `src/gui_terminal.rs`'s module docs.
export default function TerminalPane({ target, onReadyChange }: {
  target: SessionTarget;
  onReadyChange?: (ready: boolean) => void;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);
  const [scroll, setScroll] = useState<ScrollView>({ mode: "live" });
  const [notice, setNotice] = useState<string | null>(null);
  const jumpToLatest = useRef<() => void>(() => {});

  useEffect(() => {
    setError(null);
    setScroll({ mode: "live" });
    setNotice(null);
    onReadyChange?.(false);
    // Computed the same way the backend's `terminal_key` does, so the event
    // listener can be registered *before* `attach_terminal` returns -- if we
    // instead waited for the response to tell us the key, an output event
    // for a pane that starts busy could arrive and be missed in between.
    const key = `${target.feature_id}:${target.session_id}`;

    const term = new Terminal({
      convertEol: false,
      cursorBlink: true,
      fontFamily: TERMINAL_FONT,
      fontSize: 13,
      lineHeight: 1.15,
      scrollback: SCROLLBACK_LINES,
      theme: TERMINAL_THEME,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    if (containerRef.current) {
      term.open(containerRef.current);
      fit.fit();
    }

    let disposed = false;
    void loadSymbolFonts().then(() => {
      if (!disposed) term.refresh(0, term.rows - 1);
    });
    let attached = false;
    let generation: number | null = null;
    let unlisten: (() => void) | undefined;
    let newestBeforeInitial: TerminalFrame | null = null;

    // Scroll state. `phase` is authoritative; `setScroll` only mirrors it
    // for the overlay.
    let phase: "live" | "loading" | "history" = "live";
    let modes = { alternate: false, mouse: false };
    // Lines to scroll up once a requested history snapshot has loaded.
    let pendingLines = 0;
    // The newest live frame set aside while the user reads history.
    let stashed: TerminalFrame | null = null;
    // Set while a history snapshot is being written, whose own scrolling
    // must not read as the user reaching the bottom.
    let seeding = false;
    const wheel = new WheelAccumulator();
    let noticeTimer: ReturnType<typeof setTimeout> | undefined;

    const showNotice = (text: string) => {
      if (disposed) return;
      setNotice(text);
      clearTimeout(noticeTimer);
      noticeTimer = setTimeout(() => {
        if (!disposed) setNotice(null);
      }, NOTICE_MS);
    };

    const renderReplay = (replay: string) => {
      term.reset();
      term.write(replay);
    };

    const renderFrame = (frame: TerminalFrame) => {
      modes = { alternate: frame.alternate_screen, mouse: frame.mouse_reporting };
      if (phase === "live") {
        renderReplay(frame.replay);
        return;
      }
      // Reading earlier output: keep the user's position and say there is
      // more, rather than redrawing underneath them.
      stashed = frame;
      if (phase === "history") {
        setScroll((view) => view.mode === "history" && !view.newOutput
          ? { mode: "history", newOutput: true } : view);
      }
    };

    const resumeLive = () => {
      if (phase === "live") return;
      phase = "live";
      pendingLines = 0;
      const frame = stashed;
      stashed = null;
      if (frame) renderReplay(frame.replay);
      else term.scrollToBottom();
      if (!disposed) setScroll({ mode: "live" });
      // The set-aside frame may predate the history snapshot; ask for the
      // current screen rather than trusting it.
      if (attached) void invoke("terminal_refresh", { key }).catch(() => {});
    };
    jumpToLatest.current = resumeLive;

    const scrollBack = (lines: number) => {
      if (phase === "history") {
        term.scrollLines(-lines);
        return;
      }
      if (phase === "loading") {
        pendingLines += lines;
        return;
      }
      if (!attached) return;
      if (modes.alternate) {
        showNotice(FULL_SCREEN_NOTICE);
        return;
      }
      phase = "loading";
      pendingLines = lines;
      setScroll({ mode: "loading" });
      invoke<TerminalHistory>("terminal_history", { key }).then((history) => {
        if (disposed || phase !== "loading") return;
        if (history.alternate_screen || history.earlier_lines === 0) {
          resumeLive();
          showNotice(history.alternate_screen ? FULL_SCREEN_NOTICE : "There is no earlier output yet.");
          return;
        }
        seeding = true;
        term.reset();
        term.write(history.replay, () => {
          seeding = false;
          if (disposed || phase !== "loading") return;
          phase = "history";
          term.scrollLines(-pendingLines);
          pendingLines = 0;
          setScroll({ mode: "history", newOutput: stashed !== null });
        });
      }, (err) => {
        if (disposed || phase !== "loading") return;
        resumeLive();
        showNotice(`Couldn't load earlier output: ${asGuiError(err).message}`);
      });
    };

    // Tauri's `listen` registration is async. Wait for it before attaching,
    // then retain the newest full-pane frame received before the attach
    // response so an older initial capture cannot overwrite newer output.
    void (async () => {
      try {
        unlisten = await listen<TerminalFrame>(`terminal-output:${key}`, (event) => {
          if (attached) renderFrame(event.payload);
          else newestBeforeInitial = event.payload;
        });
        if (disposed) {
          unlisten();
          unlisten = undefined;
          return;
        }
        const response = await invoke<AttachTerminalResponse>("attach_terminal", {
          target,
          size: { cols: term.cols, rows: term.rows },
        });
        if (disposed) {
          await invoke("detach_terminal", { key: response.key, generation: response.generation });
          return;
        }
        generation = response.generation;
        renderFrame(response.initial);
        if (newestBeforeInitial !== null) renderFrame(newestBeforeInitial);
        newestBeforeInitial = null;
        attached = true;
        onReadyChange?.(true);
      } catch (err) {
        if (!disposed) setError(asGuiError(err).message);
        unlisten?.();
        unlisten = undefined;
      }
    })();

    const onData = term.onData((data) => {
      if (attached) {
        // Typing means the user is back with the program: show it live.
        resumeLive();
        void invoke("terminal_input", { key, text: data }).catch((err) => {
          if (!disposed) setError(asGuiError(err).message);
        });
      }
    });

    const onResize = term.onResize(({ cols, rows }) => {
      if (attached) {
        void invoke("resize_terminal", { key, size: { cols, rows } }).catch((err) => {
          if (!disposed) setError(asGuiError(err).message);
        });
      }
    });

    // Reaching the bottom of the history by any route (wheel, scrollbar,
    // keys) returns to the live view.
    const onScroll = term.onScroll(() => {
      if (seeding || phase !== "history") return;
      const buffer = term.buffer.active;
      if (buffer.viewportY >= buffer.baseY) resumeLive();
    });

    term.attachCustomKeyEventHandler((event) => {
      const action = scrollKeyAction(event, phase !== "live");
      if (!action) return true;
      if (event.type === "keydown") {
        const page = Math.max(1, term.rows - 1);
        if (action === "pageUp") scrollBack(page);
        else if (action === "top") {
          if (phase === "history") term.scrollToTop();
          else scrollBack(SCROLLBACK_LINES);
        } else if (action === "latest") resumeLive();
        else if (phase === "history") {
          const buffer = term.buffer.active;
          if (buffer.viewportY + page >= buffer.baseY) resumeLive();
          else term.scrollLines(page);
        }
      }
      // Handled here, so never sent to the program.
      return false;
    });

    // Captured on the container, before xterm's own wheel handling. While
    // reading history xterm scrolls its scrollback natively; otherwise the
    // gesture is routed here and xterm never sees it -- in particular it
    // never gets to turn a wheel into arrow keys for the program.
    const onWheel = (event: WheelEvent) => {
      if (!attached || phase === "history") return;
      event.preventDefault();
      event.stopPropagation();
      const surface = term.element ?? containerRef.current;
      const rect = surface?.getBoundingClientRect();
      const cellHeight = rect && term.rows > 0 ? rect.height / term.rows : 0;
      const steps = wheel.steps(event, cellHeight, term.rows);
      if (steps === 0) return;
      if (modes.alternate) {
        if (!modes.mouse) {
          showNotice(FULL_SCREEN_NOTICE);
          return;
        }
        const cellWidth = rect && term.cols > 0 ? rect.width / term.cols : 0;
        const cell = (offset: number, size: number, count: number) =>
          size > 0 ? Math.min(count - 1, Math.max(0, Math.floor(offset / size))) : 0;
        void invoke<boolean>("terminal_wheel", {
          key,
          direction: steps < 0 ? "up" : "down",
          steps: Math.min(Math.abs(steps), MAX_WHEEL_STEPS),
          col: cell(event.clientX - (rect?.left ?? 0), cellWidth, term.cols),
          row: cell(event.clientY - (rect?.top ?? 0), cellHeight, term.rows),
        }).catch((err) => showNotice(`Couldn't scroll: ${asGuiError(err).message}`));
        return;
      }
      if (steps < 0) scrollBack(-steps);
    };
    const container = containerRef.current;
    container?.addEventListener("wheel", onWheel, { capture: true, passive: false });

    // Refit on any container size change (window resize, sidebar, the draft
    // composer opening), not only window resizes.
    const observer = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        // The container can be momentarily zero-sized while unmounting.
      }
    });
    if (containerRef.current) observer.observe(containerRef.current);

    return () => {
      disposed = true;
      jumpToLatest.current = () => {};
      clearTimeout(noticeTimer);
      onReadyChange?.(false);
      observer.disconnect();
      container?.removeEventListener("wheel", onWheel, { capture: true });
      onData.dispose();
      onResize.dispose();
      onScroll.dispose();
      unlisten?.();
      unlisten = undefined;
      if (attached) void invoke("detach_terminal", { key, generation }).catch(() => {});
      term.dispose();
    };
  }, [target.project_id, target.feature_id, target.session_id, onReadyChange]);

  return (
    <div className="term-frame">
      {error && (
        <p role="alert" className="term-error">Terminal connection failed: {error}</p>
      )}
      <div ref={containerRef} className="term-surface" />
      {scroll.mode !== "live" ? (
        <div className="term-scrollback" role="status">
          <Icon name="arrowUp" size={13} />
          <span>
            {scroll.mode === "loading" ? "Loading earlier output…" : "Viewing earlier output"}
            {scroll.mode === "history" && scroll.newOutput && (
              <strong className="term-scrollback-new"> · New output below</strong>
            )}
          </span>
          {scroll.mode === "history" && (<>
            <button
              type="button"
              className="btn btn-primary btn-sm"
              title="Return to live output (Esc or Shift+End in the terminal)"
              aria-keyshortcuts="Escape Shift+End"
              // Keep focus where it was -- the terminal or the composer.
              onMouseDown={(event) => event.preventDefault()}
              onClick={() => jumpToLatest.current()}
            >
              <Icon name="arrowDown" size={12} />
              Jump to latest
            </button>
            <span className="term-scrollback-keys">or <kbd>Esc</kbd></span>
          </>)}
        </div>
      ) : notice ? (
        <div className="term-scrollback term-scrollback-notice" role="status">{notice}</div>
      ) : null}
    </div>
  );
}
