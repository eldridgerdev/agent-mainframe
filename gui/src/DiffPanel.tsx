import { Fragment, useEffect, useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { DiffHunk, DiffLine, DiffOptions, FeatureTarget, asGuiError, loadDiff, saveDiffLayout } from "./api";
import { Field, Modal, Spinner } from "./ui";
import { SyntaxBadge, SyntaxCode } from "./SyntaxCode";
import { DIFF_KEYS, applyFold, diffKeyCommand, edgeFile, effectiveCursor, moveCursor, rowKey } from "./diffKeys";
import { TreeRow, allDirs, amfGroupOpen, buildRows, selectFile, toggleAllFolds } from "./diffTree";

/** Align runs of removed/added rows; marker rows retain their own position. */
function splitRows(lines: DiffLine[]): [DiffLine | null, DiffLine | null][] {
  const rows: [DiffLine | null, DiffLine | null][] = [];
  for (let i = 0; i < lines.length;) {
    const line = lines[i];
    if (line.kind === "context" || line.kind === "marker") {
      rows.push([line, line.kind === "marker" ? null : line]); i++; continue;
    }
    const old: DiffLine[] = [], next: DiffLine[] = [];
    while (i < lines.length && lines[i].kind === "removed") old.push(lines[i++]);
    while (i < lines.length && lines[i].kind === "added") next.push(lines[i++]);
    for (let j = 0; j < Math.max(old.length, next.length); j++) rows.push([old[j] ?? null, next[j] ?? null]);
  }
  return rows;
}

export function Hunk({ hunk, split, selection, annotate }: { hunk: DiffHunk; split: boolean; selection?: {
  disabled: boolean; contains: (line: DiffLine) => boolean; select: (line: DiffLine, extend: boolean) => void;
}; /** Unified view only: content shown in a row under a line (saved review threads). */ annotate?: (line: DiffLine) => ReactNode }) {
  function number(line: DiffLine | null, side: "old_line" | "new_line") {
    const value = line?.[side];
    if (value == null || !line || !selection) return value;
    return <button type="button" className="diff-line-select" disabled={selection.disabled}
      aria-label={`Select ${side === "old_line" ? "base " : ""}line ${value}`} aria-pressed={selection.contains(line)}
      onClick={(event) => selection.select(line, event.shiftKey)}>{value}</button>;
  }
  const selected = (line: DiffLine | null) => line && selection?.contains(line) ? " review-line-selected" : "";
  return <section className="diff-hunk">
    <div className="diff-hunk-header">{hunk.header}</div>
    {split ? <table className="diff-lines diff-split" aria-label="Side-by-side hunk"><tbody>
      {splitRows(hunk.lines).map(([old, next], index) => old?.kind === "marker" ? <tr key={index}><td colSpan={4} className="diff-text diff-marker"><code>{old.text}</code></td></tr> : <tr key={index}>
        <td className="diff-number">{number(old, "old_line")}</td>
        <td className={`diff-text diff-${old?.kind ?? "empty"}${selected(old)}`}>{old && <SyntaxCode text={old.text} spans={old.syntax} emphasis={old.emphasis} change="removed" />}</td>
        <td className="diff-number">{number(next, "new_line")}</td>
        <td className={`diff-text diff-${next?.kind ?? "empty"}${selected(next)}`}>{next && <SyntaxCode text={next.text} spans={next.syntax} emphasis={next.emphasis} change="added" />}</td>
      </tr>)}
    </tbody></table> : <table className="diff-lines" aria-label="Unified hunk"><tbody>
      {hunk.lines.map((line, index) => {
        const note = annotate?.(line);
        return <Fragment key={index}><tr className={`diff-${line.kind}${selected(line)}`}>
          <td className="diff-number">{number(line, "old_line")}</td><td className="diff-number">{number(line, "new_line")}</td>
          <td className="diff-text"><SyntaxCode text={line.text} spans={line.syntax} emphasis={line.emphasis} change={line.kind === "removed" || line.kind === "added" ? line.kind : undefined} /></td>
        </tr>{note && <tr className="diff-annotation"><td colSpan={3}>{note}</td></tr>}</Fragment>;
      })}
    </tbody></table>}
  </section>;
}

export default function DiffPanel({ target, onClose, hidden = false }: { target: FeatureTarget; onClose: () => void; hidden?: boolean }) {
  const [options, setOptions] = useState<DiffOptions>({ commit: null, base_ref: null, ignore_whitespace: false, context: "standard" });
  const [baseDraft, setBaseDraft] = useState("");
  const [path, setPath] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  // Fold state is the reader's, keyed by directory path, so it survives a refresh.
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(new Set());
  const [amfChoice, setAmfChoice] = useState<boolean | null>(null);
  // null until the reader picks one: then the layout saved by the TUI (or an earlier session) applies.
  const [splitChoice, setSplitChoice] = useState<boolean | null>(null);
  const [layoutError, setLayoutError] = useState<string | null>(null);
  const [cursor, setCursor] = useState<string | null>(null);
  const [helpOpen, setHelpOpen] = useState(false);
  const [hunk, setHunk] = useState(0);
  const content = useRef<HTMLDivElement>(null);
  const query = useQuery({
    queryKey: ["feature-diff", target.project_id, target.feature_id, options],
    queryFn: () => loadDiff(target, options),
    retry: false,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  const view = query.data;
  const split = splitChoice ?? view?.layout === "side-by-side";
  const all = view?.files ?? [];
  const file = selectFile(all, path, filter);
  const rows = buildRows(all, { collapsed, amfChoice, filter });
  const amfCount = all.filter((item) => item.amf_generated).length;
  const filtering = filter.trim() !== "";
  const amfOpen = filtering || amfGroupOpen(amfChoice, all.length - amfCount);
  const dirs = new Set(allDirs(all));
  const anyFolded = [...collapsed].some((dir) => dirs.has(dir));
  const toggleDir = (key: string) => setCollapsed((prev) => { const next = new Set(prev); if (!next.delete(key)) next.add(key); return next; });
  const at = effectiveCursor(rows, cursor, file?.path);
  const cursorClass = (row: TreeRow) => rowKey(row) === at ? " diff-cursor" : "";
  const fileRow = (row: Extract<TreeRow, { kind: "file" }>) => <li key={row.file.path} style={{ paddingLeft: row.depth * 12 }}>
    <button className={`diff-file ${row.file.path === file?.path ? "diff-file-selected" : ""}${cursorClass(row)}`} aria-pressed={row.file.path === file?.path}
      aria-current={rowKey(row) === at ? "true" : undefined} title={row.file.path} onClick={() => { setPath(row.file.path); setCursor(rowKey(row)); }}>
      <span>{row.name}</span><small>{row.file.status} · +{row.file.additions} −{row.file.deletions}</small>
    </button></li>;
  useEffect(() => { setHunk(0); if (content.current) content.current.scrollTop = 0; }, [file?.path, view]);
  function jump(index: number) {
    setHunk(index);
    const element = content.current?.querySelectorAll<HTMLElement>(".diff-hunk")[index];
    if (element && content.current) content.current.scrollTop = element.offsetTop;
  }
  // Saves run one after another so the last choice is the one left on disk.
  const saveChain = useRef<Promise<unknown>>(Promise.resolve());
  function chooseLayout(next: boolean) {
    setSplitChoice(next);
    setLayoutError(null);
    saveChain.current = saveChain.current
      .then(() => saveDiffLayout(next ? "side-by-side" : "unified"))
      .catch((error) => setLayoutError(asGuiError(error).message));
  }
  // A filter forces every folder open, so say so instead of changing folds nobody can see.
  const [foldNotice, setFoldNotice] = useState<string | null>(null);
  useEffect(() => { if (!filtering) setFoldNotice(null); }, [filtering]);
  const foldBlocked = () => {
    if (!live.current.filtering) return false;
    setFoldNotice("Folding is off while a filter is active. Clear the filter to fold folders.");
    return true;
  };
  // The key handler reads the latest render's state through a ref, so it subscribes once.
  const live = useRef({ filtering, rows, at, file, hunk, amfCount, amfOpen, collapsed, amfChoice, all, split, fetching: query.isFetching });
  live.current = { filtering, rows, at, file, hunk, amfCount, amfOpen, collapsed, amfChoice, all, split, fetching: query.isFetching };
  const refetch = query.refetch;
  useEffect(() => {
    if (hidden) return;
    const onKey = (event: KeyboardEvent) => {
      const command = diffKeyCommand(event, content.current?.closest("[role='dialog']"));
      if (!command) return;
      const s = live.current;
      event.preventDefault();
      if (["fold", "fold-all", "collapse", "expand", "amf"].includes(command) && foldBlocked()) return;
      const visit = (key: string | null) => {
        if (!key) return;
        setCursor(key);
        if (key.startsWith("file:")) setPath(key.slice(5));
        requestAnimationFrame(() => document.querySelector(".diff-cursor")?.scrollIntoView?.({ block: "nearest" }));
      };
      switch (command) {
        case "next-row": visit(moveCursor(s.rows, s.at, 1)); break;
        case "prev-row": visit(moveCursor(s.rows, s.at, -1)); break;
        case "first-file": visit(edgeFile(s.rows, false)); break;
        case "last-file": visit(edgeFile(s.rows, true)); break;
        case "next-hunk": if (s.file && s.hunk + 1 < s.file.hunks.length) jump(s.hunk + 1); break;
        case "prev-hunk": if (s.hunk > 0) jump(s.hunk - 1); break;
        case "layout": chooseLayout(!s.split); break;
        case "refresh": if (!s.fetching) void refetch(); break;
        case "whitespace": setOptions((o) => ({ ...o, ignore_whitespace: !o.ignore_whitespace })); break;
        case "amf": if (s.amfCount > 0) setAmfChoice(!s.amfOpen); break;
        case "fold-all": setCollapsed(toggleAllFolds(s.all, s.collapsed)); break;
        case "help": setHelpOpen((open) => !open); break;
        default: {
          const next = applyFold(command, s.rows, s.at, { collapsed: s.collapsed, amfChoice: s.amfChoice });
          setCollapsed(next.collapsed); setAmfChoice(next.amfChoice); setCursor(next.cursor);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [hidden, refetch]);
  return <Modal hidden={hidden} label="Diff viewer" title={view ? `Changes · ${view.feature_name}` : "Changes"} size="xl" onClose={onClose}
    footer={<button className="btn btn-secondary" onClick={onClose}>Close</button>}>
    <div className="diff-controls">
      <Field label="Diff scope"><select value={options.commit ?? ""} onChange={(event) => setOptions({ ...options, commit: event.target.value || null })}>
        <option value="">All current changes</option>
        {options.commit && !view?.commits.some((commit) => commit.hash === options.commit) && <option value={options.commit}>Selected commit</option>}
        {view?.commits.map((commit) => <option key={commit.hash} value={commit.hash}>{commit.short_hash} {commit.subject}</option>)}
      </select></Field>
      <Field label="Layout"><select value={split ? "split" : "unified"} onChange={(event) => chooseLayout(event.target.value === "split")}>
        <option value="unified">Unified</option><option value="split">Side by side</option>
      </select></Field>
      <Field label="Context"><select value={options.context} onChange={(event) => setOptions({ ...options, context: event.target.value as DiffOptions["context"] })}>
        <option value="standard">3 lines</option><option value="expanded">10 lines</option><option value="full">Whole file</option>
      </select></Field>
      <label><input type="checkbox" checked={options.ignore_whitespace} onChange={(event) => setOptions({ ...options, ignore_whitespace: event.target.checked })} /> Ignore whitespace</label>
      <button className="btn btn-secondary btn-sm" onClick={() => void query.refetch()} disabled={query.isFetching}>Refresh</button>
      <button className="btn btn-ghost btn-sm" aria-expanded={helpOpen} aria-controls="diff-key-help" onClick={() => setHelpOpen(!helpOpen)}>Keyboard shortcuts</button>
    </div>
    <form className="diff-controls" onSubmit={(event) => { event.preventDefault(); setOptions({ ...options, base_ref: baseDraft.trim() || null }); }}>
      <Field label="Base ref"><input value={baseDraft} placeholder="Automatic" disabled={options.commit !== null} onChange={(event) => setBaseDraft(event.target.value)} /></Field>
      <button className="btn btn-ghost btn-sm" disabled={options.commit !== null}>Apply base</button>
      {options.base_ref && !options.commit && <button type="button" className="btn btn-ghost btn-sm" onClick={() => { setBaseDraft(""); setOptions({ ...options, base_ref: null }); }}>Use automatic base</button>}
    </form>
    {helpOpen && <section id="diff-key-help" className="diff-key-help" aria-label="Keyboard shortcuts">
      <dl>{DIFF_KEYS.map((item) => <div key={item.key}><dt><kbd>{item.key}</kbd></dt><dd>{item.help}</dd></div>)}</dl>
      <small>Shortcuts are off while typing in a field or while another dialog is open.</small>
    </section>}
    {foldNotice && <p role="status">{foldNotice}</p>}
    {layoutError && <p role="status">Layout not saved: {layoutError}</p>}
    {query.isFetching && <p role="status"><Spinner /> Loading changes…</p>}
    {query.error && <p role="alert">{asGuiError(query.error).message}</p>}
    {view?.commits_error && <p role="status">Commit picker: {view.commits_error}</p>}
    {view && !query.error && <>
      <p className="diff-summary">{view.branch} · {view.base_ref} ({view.base_commit.slice(0, 8)}) · {view.files.length} files · +{view.total_additions} −{view.total_deletions}</p>
      {view.files.length === 0 ? <p role="status">No changes in this scope.</p> : <>
        <div className="diff-controls">
          <Field label="Filter files"><input value={filter} onChange={(event) => setFilter(event.target.value)} /></Field>
          <button className="btn btn-secondary btn-sm" disabled={filtering} onClick={() => setCollapsed(toggleAllFolds(all, new Set([...collapsed].filter((dir) => dirs.has(dir)))))}>{anyFolded ? "Unfold folders" : "Fold folders"}</button>
          {amfCount > 0 && <button className="btn btn-secondary btn-sm" aria-pressed={amfOpen} disabled={filtering} onClick={() => setAmfChoice(!amfOpen)}>
            {amfOpen ? "Hide" : "Show"} AMF files ({amfCount})</button>}
        </div>
        <div className="diff-reader">
          <nav className="diff-files" aria-label="Changed files"><ul className="diff-tree">{rows.length === 0 && <li className="diff-tree-empty">No matching files.</li>}{rows.map((row) => row.kind === "file" ? fileRow(row)
            : row.kind === "dir" ? <li key={`dir:${row.key}`} style={{ paddingLeft: row.depth * 12 }}>
              <button className={`diff-dir${cursorClass(row)}`} aria-current={rowKey(row) === at ? "true" : undefined} aria-expanded={row.open} aria-label={`Folder ${row.key}`} disabled={filtering} onClick={() => { setCursor(rowKey(row)); toggleDir(row.key); }}>
                <span aria-hidden="true">{row.open ? "▾" : "▸"}</span> {row.name}/ <small>{row.files} {row.files === 1 ? "file" : "files"}</small></button></li>
            : <li key="amf-group">
              <button className={`diff-dir diff-amf-group${cursorClass(row)}`} aria-current={rowKey(row) === at ? "true" : undefined} aria-expanded={row.open} aria-label="AMF files group" disabled={filtering} onClick={() => { setCursor(rowKey(row)); setAmfChoice(!row.open); }}>
                <span aria-hidden="true">{row.open ? "▾" : "▸"}</span> AMF files <small>{row.count} generated by AMF</small></button></li>)}
          </ul></nav>
          <section className="diff-content" aria-label="File diff">
            {file ? <>
              <div className="diff-file-header"><strong>{file.old_path && file.old_path !== file.path ? `${file.old_path} → ${file.path}` : file.path}</strong>
                <span>{file.status}</span>
                <button className="btn btn-ghost btn-sm" disabled={hunk === 0} onClick={() => jump(hunk - 1)}>Previous hunk</button>
                <button className="btn btn-ghost btn-sm" disabled={hunk + 1 >= file.hunks.length} onClick={() => jump(hunk + 1)}>Next hunk</button>
                {file.hunks.length > 0 && <span>Hunk {hunk + 1} of {file.hunks.length}</span>}
                <SyntaxBadge info={file.syntax} onInstalled={() => void query.refetch()} />
              </div>
              <div ref={content} className="diff-code">
                {file.hunks.length > 0 && <details className="diff-metadata"><summary>Patch metadata</summary><pre>{file.patch.split("\n@@")[0]}</pre></details>}
                {file.is_binary ? <p>Binary file changed; no text diff is available.</p> : file.hunks.length === 0 ? <pre>{file.patch || "No textual changes."}</pre>
                  : file.hunks.map((item, index) => <Hunk key={index} hunk={item} split={split} />)}
              </div>
            </> : <p>No files match your filter.</p>}
          </section>
        </div>
      </>}
    </>}
  </Modal>;
}
