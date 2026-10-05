import { useEffect, useId, useRef } from "react";
import { Icon, Spinner } from "./ui";

export default function PromptComposer({
  text, sending, ready, focusRequested, onFocusHandled, onChange, onClear, onSend,
}: {
  text: string;
  sending: boolean;
  ready: boolean;
  /** A handoff just seeded this draft; take focus once, then report it handled. */
  focusRequested: boolean;
  onFocusHandled: () => void;
  onChange: (text: string) => void;
  onClear: () => void;
  onSend: () => void;
}) {
  const canSend = ready && !sending && text.trim().length > 0;
  const textarea = useRef<HTMLTextAreaElement>(null);
  const shortcutsId = useId();
  const isMac = navigator.platform.startsWith("Mac");
  const sendModifier = isMac ? "Cmd" : "Ctrl";

  // Focus only on an explicit handoff. Switching tabs or returning to a
  // feature must leave focus where the user put it -- often the terminal.
  useEffect(() => {
    if (!focusRequested || !textarea.current) return;
    textarea.current.focus();
    textarea.current.setSelectionRange(textarea.current.value.length, textarea.current.value.length);
    onFocusHandled();
  }, [focusRequested, onFocusHandled]);

  return (
    <div className="composer">
      <div className="composer-head">
        <Icon name="sparkles" />
        <strong>Compose prompt</strong>
        <span className="muted small">Draft here, then send to the agent.</span>
        <span className="tabs-spacer" />
        <button className="btn btn-sm btn-ghost" disabled={sending || !text} onClick={onClear}>
          Clear
        </button>
      </div>
      <textarea
        ref={textarea}
        aria-label="Draft prompt"
        aria-describedby={shortcutsId}
        placeholder="Write a message or paste a prompt…"
        value={text}
        readOnly={sending}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
            // Enter used to confirm an IME candidate must never send a draft.
            if (event.nativeEvent.isComposing || event.keyCode === 229) return;
            event.preventDefault();
            if (canSend) onSend();
          }
        }}
        rows={4}
      />
      <div className="composer-foot">
        <div className="composer-help">
          <div id={shortcutsId} className="composer-shortcuts small">
            <span><kbd>{sendModifier}</kbd> + <kbd>Enter</kbd> to send</span>
            <span><kbd>Enter</kbd> for a new line</span>
          </div>
          {!ready && <span className="muted small">Waiting for terminal connection…</span>}
        </div>
        <button className="btn btn-primary" disabled={!canSend} onClick={onSend}
          aria-keyshortcuts={isMac ? "Meta+Enter" : "Control+Enter"}
          title={`Send prompt (${sendModifier}+Enter)`}>
          {sending ? <Spinner /> : <Icon name="send" />}
          {sending ? "Sending…" : "Send prompt"}
        </button>
      </div>
    </div>
  );
}
