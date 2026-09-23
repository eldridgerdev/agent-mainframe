import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { asGuiError } from "./api";

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
  initial: string;
}

// Task 6 ("Implement the GUI terminal transport"): the actual transport
// correctness (Unicode, escape sequences, high output volume, cleanup) is
// verified against real tmux sessions in `src/gui_terminal.rs`'s own tests,
// not here -- this component's job is proving the wiring works end to end
// through xterm.js and Tauri's IPC/event boundary, which those Rust-only
// tests cannot exercise.
export default function TerminalPane({ target }: { target: SessionTarget }) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
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
    let newestBeforeInitial: string | null = null;

    const renderReplay = (replay: string) => {
      term.reset();
      term.write(replay);
    };

    // Tauri's `listen` registration is async. Wait for it before attaching,
    // then retain the newest full-pane replay received before the attach
    // response so an older initial capture cannot overwrite newer output.
    void (async () => {
      try {
        unlisten = await listen<string>(`terminal-output:${key}`, (event) => {
          if (attached) renderReplay(event.payload);
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
        renderReplay(response.initial);
        if (newestBeforeInitial !== null) renderReplay(newestBeforeInitial);
        newestBeforeInitial = null;
        attached = true;
      } catch (err) {
        if (!disposed) setError(asGuiError(err).message);
        unlisten?.();
        unlisten = undefined;
      }
    })();

    const onData = term.onData((data) => {
      if (attached) {
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
      observer.disconnect();
      onData.dispose();
      onResize.dispose();
      unlisten?.();
      unlisten = undefined;
      if (attached) void invoke("detach_terminal", { key, generation }).catch(() => {});
      term.dispose();
    };
  }, [target.project_id, target.feature_id, target.session_id]);

  return (
    <div className="term-frame">
      {error && (
        <p role="alert" className="term-error">Terminal connection failed: {error}</p>
      )}
      <div ref={containerRef} className="term-surface" />
    </div>
  );
}
