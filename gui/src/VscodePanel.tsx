import { useRef, useState } from "react";
import type { EditorCleanup, FeatureTarget } from "./api";
import { EditorReport } from "./DormancyPanel";
import { closeEditors, FeatureEditor, FeatureEditorState } from "./sessionsApi";
import { EmptyState, Icon, Spinner } from "./ui";
import "./sessions.css";

const STATE_LABEL: Record<FeatureEditorState, string> = {
  open: "Open",
  opening: "Opening",
  not_owned: "Not AMF's",
};

function explain(editor: FeatureEditor): string {
  switch (editor.state) {
    case "open":
      return editor.closes_with_feature
        ? "AMF opened this window and closes it, with its language servers, when the feature stops."
        : "AMF opened this window. Closing editors on stop is off (kill_editor_on_stop), so stopping the feature leaves it open.";
    case "opening":
      return "Waiting for the window to appear so AMF can identify it. A window that never appears on this machine (a remote one, say) is left as not AMF's.";
    case "not_owned":
      return "VS Code handed the folder to a window AMF did not open, or the window is not on this machine. AMF never closes it.";
  }
}

const when = (iso: string) => new Date(iso).toLocaleString(undefined, {
  month: "short", day: "numeric", hour: "2-digit", minute: "2-digit",
});

/** The VS Code tab: the windows AMF launched for this feature, from the
 *  shared `launched_editors` record. There is no tmux pane to attach. */
export default function VscodePanel({
  target,
  workdir,
  editors,
  opening,
  onOpen,
  onClosed,
  onError,
}: {
  target: FeatureTarget;
  workdir: string;
  editors: FeatureEditor[];
  /** An open request from this window is in flight. */
  opening: boolean;
  onOpen: () => void;
  onClosed: (message: string) => void;
  onError: (err: unknown) => void;
}) {
  // The rows the user confirmed against; a window opened after this is
  // refused by the backend rather than closed unseen.
  const [confirming, setConfirming] = useState<string[] | null>(null);
  const [closing, setClosing] = useState(false);
  const [report, setReport] = useState<EditorCleanup | null>(null);
  const inFlight = useRef(false);
  const closable = editors.filter((editor) => editor.state !== "not_owned");

  async function close(seen: string[]) {
    if (inFlight.current) return;
    inFlight.current = true;
    setClosing(true);
    try {
      const response = await closeEditors(target, seen);
      setReport(response.editors);
      onClosed(response.message);
    } catch (err) {
      onError(err);
    } finally {
      inFlight.current = false;
      setClosing(false);
      setConfirming(null);
    }
  }

  return (
    <div className="vscode-panel">
      <div className="vscode-panel-actions">
        <button className="btn btn-secondary" onClick={onOpen} disabled={opening}>
          {opening ? <Spinner /> : <Icon name="plus" size={12} />} Open another VS Code window
        </button>
        <button className="btn btn-secondary" disabled={closable.length === 0 || closing || confirming !== null}
          onClick={() => { setReport(null); setConfirming(editors.map((editor) => editor.id)); }}>
          <Icon name="x" size={12} /> Close windows AMF opened
        </button>
      </div>

      {confirming && (
        <div className="callout callout-warning" role="alert">
          <Icon name="alert" size={14} />
          <div>
            <p><strong>Close {closable.length === 1 ? "this VS Code window" : `these ${closable.length} VS Code windows`}?</strong></p>
            <p>
              Unsaved changes in them are lost. A window AMF did not open, one whose process now belongs
              to something else, or one sharing its VS Code instance with other windows is left running.
            </p>
            <div className="row">
              <button className="btn btn-ghost" onClick={() => setConfirming(null)} disabled={closing}>Cancel</button>
              <button className="btn btn-danger" onClick={() => void close(confirming)} disabled={closing}>
                {closing && <Spinner />}Close windows
              </button>
            </div>
          </div>
        </div>
      )}

      {report && (
        <section aria-label="Close result">
          <EditorReport editors={report} />
        </section>
      )}

      {editors.length === 0 ? (
        <EmptyState icon="file" title="No VS Code window is open for this feature">
          Opens <span className="mono">{workdir}</span> with <span className="mono">code --new-window</span>.
        </EmptyState>
      ) : (
        <ul className="vscode-editors" aria-label="VS Code windows">
          {editors.map((editor) => (
            <li key={editor.id} className="vscode-editor">
              <span className="vscode-editor-glyph" aria-hidden="true">{""}</span>
              <span className="vscode-editor-body">
                <span>
                  <strong>{editor.name}</strong>{" "}
                  <span className={`vscode-editor-state vscode-state-${editor.state}`}>{STATE_LABEL[editor.state]}</span>
                </span>
                <span className="muted">{explain(editor)}</span>
                <span className="muted">Launched {when(editor.started_at)} on <span className="mono">{workdir}</span></span>
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
