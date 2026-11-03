import {
  CSSProperties,
  KeyboardEvent as ReactKeyboardEvent,
  ReactNode,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { FeatureStatus } from "./api";
import { shortcutAria, shortcutLabel, type ShortcutId } from "./shortcuts";

// Shared presentation primitives. Everything here is stateless or owns only
// its own open/closed state; workflow state stays in App and the panels.

const ICONS = {
  plus: "M12 5v14M5 12h14",
  play: "M7 4.5v15l12-7.5z",
  stop: "M6 6h12v12H6z",
  chevronRight: "M9 6l6 6-6 6",
  chevronDown: "M6 9l6 6 6-6",
  terminal: "M4 17l6-5-6-5M12 19h8",
  list: "M9 6h11M9 12h11M9 18h11M4 6h.01M4 12h.01M4 18h.01",
  more: "M5 12h.01M12 12h.01M19 12h.01",
  x: "M6 6l12 12M18 6L6 18",
  folder: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z",
  branch: "M6 3v12M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9",
  sparkles: "M12 3l1.9 5.1L19 10l-5.1 1.9L12 17l-1.9-5.1L5 10l5.1-1.9zM19 16l.8 2.2L22 19l-2.2.8L19 22l-.8-2.2L16 19l2.2-.8z",
  zap: "M13 2L4 14h7l-1 8 9-12h-7z",
  arrowUp: "M12 19V5M5 12l7-7 7 7",
  arrowDown: "M12 5v14M19 12l-7 7-7-7",
  trash: "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6",
  swap: "M7 7h13M16 3l4 4-4 4M17 17H4M8 13l-4 4 4 4",
  send: "M22 2L11 13M22 2l-7 20-4-9-9-4z",
  minimize: "M5 12h14",
  alert: "M12 9v4M12 17h.01M10.3 3.9L1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  check: "M5 12l5 5L20 7",
  file: "M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6",
  search: "M11 18a7 7 0 1 0 0-14 7 7 0 0 0 0 14zM21 21l-5-5",
  collapseAll: "M7 20l5-5 5 5M7 4l5 5 5-5",
  expandAll: "M7 15l5 5 5-5M7 9l5-5 5 5",
  chevronLeft: "M15 6l-6 6 6 6",
  bell: "M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.94 1.94 0 0 0 3.4 0",
  checkList: "M3 6l2 2 3-3M3 13l2 2 3-3M12 6.5h9M12 13.5h9M4 20h.01M12 20h9",
  listPlus: "M3 6h12M3 12h12M3 18h7M18 14v8M14 18h8",
  palette: "M12 21a9 9 0 1 1 9-9c0 2.5-2 3.5-4 3.5h-2a2 2 0 0 0-1.5 3.3A1.4 1.4 0 0 1 12 21zM7.5 11.5h.01M10 7.5h.01M15 7.5h.01",
  sliders: "M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M2 14h4M10 8h4M18 16h4",
  layers: "M12 3l9 4.5-9 4.5-9-4.5zM3 12l9 4.5 9-4.5M3 16.5l9 4.5 9-4.5",
  gauge: "M12 14l4-4M3.3 19a10 10 0 1 1 17.4 0",
  code: "M16 18l6-6-6-6M8 6l-6 6 6 6",
  plug: "M12 22v-5M9 8V2M15 8V2M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8z",
  moon: "M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9z",
  bookmark: "M19 21l-7-4-7 4V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z",
  bug: "M8 2l1.9 1.9M16 2l-1.9 1.9M9 7.1V6a3 3 0 0 1 6 0v1.1M12 20v-9M6 13H2M22 13h-4M3 21c0-2.1 1.7-3.9 3.8-4M21 21c0-2.1-1.7-3.9-3.8-4M20.97 5c0 2.1-1.6 3.8-3.5 4M3 5c0 2.1 1.6 3.8 3.5 4M6 13a6 6 0 0 0 12 0v-3a4 4 0 0 0-4-4h-4a4 4 0 0 0-4 4z",
  issue: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM12 13a1 1 0 1 0 0-2 1 1 0 0 0 0 2z",
  book: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20V2H6.5A2.5 2.5 0 0 0 4 4.5zM4 19.5A2.5 2.5 0 0 0 6.5 22H20v-5",
  bookOpen: "M2 4h6a4 4 0 0 1 4 4v13a3 3 0 0 0-3-3H2zM22 4h-6a4 4 0 0 0-4 4v13a3 3 0 0 1 3-3h7z",
  camera: "M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2zM12 17a4 4 0 1 0 0-8 4 4 0 0 0 0 8z",
  clipboardCheck: "M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2M9 2h6v4H9zM9 14l2 2 4-4",
  pencil: "M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5z",
  shieldCheck: "M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10zM9 12l2 2 4-4",
  compass: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM16.2 7.8l-2.1 6.3-6.3 2.1 2.1-6.3z",
  star: "M12 2l3.1 6.3 6.9 1-5 4.9 1.2 6.8L12 17.8 5.8 21l1.2-6.8-5-4.9 6.9-1z",
  appWindow: "M3 6a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2zM3 9h18M7 6.5h.01M10 6.5h.01",
  gitPullRequest: "M6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 9v6M18 15V9a3 3 0 0 0-3-3h-4M13 3l-3 3 3 3",
  gitCompare: "M6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 15V9a3 3 0 0 0-3-3h-4M13 3l-3 3 3 3M6 9v6a3 3 0 0 0 3 3h4M11 15l3 3-3 3",
  help: "M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20zM9.1 9a3 3 0 0 1 5.8 1c0 2-3 3-3 3M12 17h.01",
  keyboard: "M2 6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2zM6 8h.01M10 8h.01M14 8h.01M18 8h.01M6 12h.01M10 12h.01M14 12h.01M18 12h.01M7 16h10",
  flag: "M4 22V4M4 4h13l-2 4 2 4H4",
  gitPullRequestDraft: "M6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 9v6M18 5v.01M18 9v.01M18 13v.01",
} as const;

export type IconName = keyof typeof ICONS;

/** One glyph per meaning. Sizes: 14px inside buttons, 16px (the default)
 * in navigation and menus, 12px only in chips and disclosure toggles.
 *
 * | Icon | Means |
 * |---|---|
 * | bell | Needs attention |
 * | checkList / list / listPlus | Global TODOs / a feature's TODOs / quick TODO |
 * | palette | Appearance |
 * | sliders | Prompt overrides |
 * | layers | Feature presets |
 * | gauge / code / plug | Context settings / syntax languages / agent availability |
 * | moon | Dormant features |
 * | bookmark | Session bookmarks |
 * | bug / issue | Debug log / GitHub issues |
 * | book / bookOpen | Prompt library / Learning |
 * | camera | Screenshots |
 * | clipboardCheck | Final review |
 * | pencil | Rename and edit |
 * | shieldCheck | Supervised edits |
 * | compass | Steering coach |
 * | star | Preferred agent |
 * | appWindow | VS Code windows |
 * | gitPullRequest / gitCompare / gitPullRequestDraft | PR triage / PR review / PR workspace |
 * | sparkles / zap | AI planning and vibe modes / quick plan |
 * | help / keyboard / flag | Help menu / keyboard shortcuts / first-run steps |
 * | terminal | Agent and terminal sessions |
 * | file | A document or file |
 */
export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg
      data-icon={name}
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill={name === "play" || name === "stop" ? "currentColor" : "none"}
      stroke="currentColor"
      strokeWidth={name === "more" ? 3 : 2}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d={ICONS[name]} />
    </svg>
  );
}

export function StatusDot({ status }: { status: FeatureStatus }) {
  return <span className={`status-dot status-${status}`} aria-hidden="true" />;
}

const STATUS_TEXT: Record<FeatureStatus, string> = {
  active: "Running",
  idle: "Idle",
  stopped: "Stopped",
};

export function StatusBadge({ status }: { status: FeatureStatus }) {
  return (
    <span className={`badge badge-status status-${status}`}>
      <StatusDot status={status} />
      {STATUS_TEXT[status]}
    </span>
  );
}

export function Spinner() {
  return <span className="spinner" aria-hidden="true" />;
}

export function EmptyState({
  icon,
  title,
  children,
  action,
}: {
  icon: IconName;
  title: string;
  children?: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div className="empty-icon"><Icon name={icon} size={22} /></div>
      <h3>{title}</h3>
      {children && <p>{children}</p>}
      {action}
    </div>
  );
}

// ── Overlay stack ─────────────────────────────────────────
// One module-level registry for every open dialog and menu, so a single
// Escape reaches only the overlay in front and Tab stays inside it. "In
// front" is the last open overlay in document order: dialogs share one
// z-index, so the later one paints over the earlier, and a menu or confirm
// rendered inside a dialog follows it in the DOM. A modal layer also makes
// everything outside it `inert` until it closes.

interface OverlayLayer {
  el: HTMLElement;
  /** Modal layers make the rest of the page inert; menus do not. */
  modal: boolean;
  onEscape: () => void;
  /** Replaces the default Tab trap (menus close on Tab instead). */
  onTab?: (event: KeyboardEvent) => void;
}

const layers: OverlayLayer[] = [];
const inertByStack = new Set<Element>();

const FOCUSABLE = [
  "a[href]", "button:not(:disabled)", "input:not(:disabled):not([type=\"hidden\"])",
  "select:not(:disabled)", "textarea:not(:disabled)", "[tabindex]", "[contenteditable=\"true\"]",
].join(",");

/** True when `el` can take focus right now (connected, shown, not inert or disabled). */
function canFocus(el: Element | null): el is HTMLElement {
  return el instanceof HTMLElement && el.isConnected && !el.closest("[hidden],[inert]")
    && !(el as HTMLButtonElement).disabled;
}

/** Tab-order focusables inside `root`, skipping hidden, inert and tabindex=-1 ones. */
export function focusableWithin(root: HTMLElement): HTMLElement[] {
  return Array.from(root.querySelectorAll<HTMLElement>(FOCUSABLE))
    .filter((el) => el.tabIndex >= 0 && canFocus(el));
}

function topLayer(modalOnly = false): OverlayLayer | undefined {
  let top: OverlayLayer | undefined;
  for (const layer of layers) {
    // A layer inside a `hidden` ancestor (a minimized dialog's wrapper) is
    // not shown, so it neither owns keys nor makes the page inert.
    if (!layer.el.isConnected || (modalOnly && !layer.modal) || layer.el.closest("[hidden]")) continue;
    if (!top || top.el.compareDocumentPosition(layer.el) & Node.DOCUMENT_POSITION_FOLLOWING) top = layer;
  }
  return top;
}

/** Marks everything outside the front modal inert, undoing the previous pass.
 * Elements with `data-overlay-exempt` (the toasts) stay reachable. */
function applyInert() {
  for (const el of inertByStack) el.removeAttribute("inert");
  inertByStack.clear();
  const top = topLayer(true);
  if (!top) return;
  let node: Element = top.el;
  while (node.parentElement && node !== document.body) {
    for (const sibling of Array.from(node.parentElement.children)) {
      if (sibling === node || sibling.hasAttribute("inert") || sibling.hasAttribute("data-overlay-exempt")
        || sibling.tagName === "SCRIPT" || sibling.tagName === "STYLE") continue;
      sibling.setAttribute("inert", "");
      inertByStack.add(sibling);
    }
    node = node.parentElement;
  }
}

function trapTab(event: KeyboardEvent, root: HTMLElement) {
  const items = focusableWithin(root);
  const active = document.activeElement;
  if (items.length === 0) {
    event.preventDefault();
    root.focus();
    return;
  }
  const first = items[0];
  const last = items[items.length - 1];
  const index = items.indexOf(active as HTMLElement);
  if (index === -1) {
    if (active === root || !root.contains(active)) {
      event.preventDefault();
      (event.shiftKey ? last : first).focus();
    }
  } else if (!event.shiftKey && index === items.length - 1) {
    event.preventDefault();
    first.focus();
  } else if (event.shiftKey && index === 0) {
    event.preventDefault();
    last.focus();
  }
}

function onLayerKey(event: KeyboardEvent) {
  // A control that already handled the key (an input clearing itself, a
  // panel that claims Escape first) keeps it.
  if (event.defaultPrevented || event.isComposing) return;
  const top = topLayer();
  if (!top) return;
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    top.onEscape();
  } else if (event.key === "Tab") {
    if (top.onTab) top.onTab(event);
    else trapTab(event, top.el);
  }
}

// Showing or hiding an ancestor of an open layer changes which layer is in
// front without any layer registering, so re-derive inertness then too.
let hiddenObserver: MutationObserver | null = null;

/** Registers an open overlay; returns its unregister function. */
function registerLayer(layer: OverlayLayer): () => void {
  if (layers.length === 0) {
    window.addEventListener("keydown", onLayerKey);
    if (typeof MutationObserver !== "undefined") {
      hiddenObserver = new MutationObserver(applyInert);
      hiddenObserver.observe(document.body, { attributes: true, attributeFilter: ["hidden"], subtree: true });
    }
  }
  layers.push(layer);
  applyInert();
  return () => {
    const index = layers.indexOf(layer);
    if (index >= 0) layers.splice(index, 1);
    if (layers.length === 0) {
      window.removeEventListener("keydown", onLayerKey);
      hiddenObserver?.disconnect();
      hiddenObserver = null;
    }
    applyInert();
  };
}

const INITIAL_FOCUS = [
  "[data-autofocus]",
  ".modal-body input:not([type=checkbox]):not([type=radio]):not([type=hidden]):not(:disabled)",
  ".modal-body textarea:not(:disabled)",
  ".modal-body select:not(:disabled)",
];

/** Where focus lands when a dialog opens: an explicit autofocus target, else
 * the first text field, else the dialog itself (so long read-only dialogs do
 * not open with a ring on some toolbar button). */
function initialFocus(dialog: HTMLElement): HTMLElement {
  for (const selector of INITIAL_FOCUS) {
    const match = Array.from(dialog.querySelectorAll(selector)).find(canFocus);
    if (match) return match;
  }
  return dialog;
}

/** Centered dialog. `onClose` runs on Esc and on a backdrop click unless
 * `dismissable` is false (used where closing would lose an answer).
 *
 * While shown it moves focus inside (keeping an `autoFocus` field), traps
 * Tab, makes the page behind it inert, owns Escape only when it is the
 * overlay in front, and returns focus to whatever opened it on unmount. */
export function Modal({
  label,
  title,
  subtitle,
  onClose,
  children,
  footer,
  size = "md",
  dismissable = true,
  hidden = false,
  headerActions,
  onSubmit,
  inline = false,
}: {
  label: string;
  title: ReactNode;
  subtitle?: ReactNode;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: "sm" | "md" | "lg" | "xl" | "workspace";
  dismissable?: boolean;
  /** Keeps the modal mounted, with its state, while another dialog is in
   * front: not drawn, not in the accessibility tree, and deaf to Escape. */
  hidden?: boolean;
  headerActions?: ReactNode;
  /** Makes the body and footer one form, so Enter submits. */
  onSubmit?: () => void;
  /** Render a modal's titled content inside an existing workspace pane. */
  inline?: boolean;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  // Read during the first render, before an autoFocus child moves focus.
  const [opener] = useState(() => (typeof document === "undefined" ? null : document.activeElement));
  const resumeFocus = useRef<HTMLElement | null>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  const dismissableRef = useRef(dismissable);
  dismissableRef.current = dismissable;
  const active = !hidden && !inline;

  // Registered once per shown period (not per render) so its place in the
  // stack is stable. A non-dismissable dialog still swallows Escape, so the
  // key never falls through to the dialog underneath.
  useEffect(() => {
    const el = dialogRef.current;
    if (!active || !el) return;
    const unregister = registerLayer({
      el,
      modal: true,
      onEscape: () => { if (dismissableRef.current) closeRef.current(); },
    });
    if (!el.contains(document.activeElement)) {
      const resume = resumeFocus.current;
      (resume && el.contains(resume) && canFocus(resume) ? resume : initialFocus(el)).focus();
    }
    return () => {
      // Hidden behind another dialog: remember where focus was for later.
      const focused = document.activeElement;
      resumeFocus.current = focused instanceof HTMLElement && el.contains(focused) ? focused : null;
      unregister();
    };
  }, [active]);

  // Declared after the layer effect so its cleanup runs after the page is no
  // longer inert, and the opener can take focus again.
  useEffect(() => () => {
    const focused = document.activeElement;
    // Only take focus back when closing dropped it (focus inside a removed
    // dialog falls to the body); a close that focused something on purpose wins.
    const lost = !focused || focused === document.body || !focused.isConnected;
    if (lost && canFocus(opener)) opener.focus();
  }, [opener]);

  const labelledByTitle = typeof title === "string" && title === label;
  const content = (
    <>
      <div className="modal-body">{children}</div>
      {footer && <footer className="modal-footer">{footer}</footer>}
    </>
  );

  if (inline) return <section className="modal-inline" aria-label={label}>
    <header className="modal-header"><div><h2>{title}</h2>{subtitle && <p className="modal-subtitle">{subtitle}</p>}</div>
      <div className="modal-header-actions">{headerActions}{dismissable && <button type="button" className="btn btn-icon btn-ghost" aria-label="Close" onClick={onClose}><Icon name="x" /></button>}</div>
    </header>
    {content}
  </section>;

  return (
    <div
      className="modal-backdrop"
      hidden={hidden}
      onMouseDown={(event) => {
        if (dismissable && event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        role="dialog"
        tabIndex={-1}
        aria-label={labelledByTitle ? undefined : label}
        aria-labelledby={labelledByTitle ? titleId : undefined}
        aria-modal="true"
        className={`modal modal-${size}`}
      >
        <header className="modal-header">
          <div>
            <h2 id={titleId}>{title}</h2>
            {subtitle && <p className="modal-subtitle">{subtitle}</p>}
          </div>
          <div className="modal-header-actions">
            {headerActions}
            {dismissable && (
              <button type="button" className="btn btn-icon btn-ghost" aria-label="Close" onClick={onClose}>
                <Icon name="x" />
              </button>
            )}
          </div>
        </header>
        {onSubmit ? (
          <form
            className="modal-form"
            onSubmit={(event) => {
              event.preventDefault();
              onSubmit();
            }}
          >
            {content}
          </form>
        ) : content}
      </div>
    </div>
  );
}

// ── Button tones ──────────────────────────────────────────
// One rule for what a confirm button's color means:
// - `btn-danger`: deletes or discards something the user made or kept
//   (a draft, an edit, a list, a session, files). Also "Close session".
// - `btn-warning`: proceed past a warning without losing data — resource
//   approvals ("Start anyway") and ending work that stays resumable.
// - `btn-primary`: the ordinary, reversible confirm.
// The safe choice ("Keep editing", "Cancel") is `btn-secondary`/`btn-ghost`.

/** Marks a callout as an inline confirmation (see {@link ConfirmCallout}). */
export const CONFIRM_CALLOUT_SELECTOR = "[data-confirm-callout]";

/** An inline confirmation inside a page or dialog: a `role="alert"` region
 * (announced when it appears, not a second modal) that moves focus into
 * itself on mount. Focus lands on `[data-autofocus]`, else the primary
 * button, else the first control that is not `btn-danger` — for a discard
 * prompt that is the safe "Keep editing" choice, so Enter never destroys
 * work by accident. */
export function ConfirmCallout({
  label,
  className,
  as = "div",
  children,
}: {
  label: string;
  className?: string;
  as?: "div" | "section";
  children: ReactNode;
}) {
  const ref = useRef<HTMLElement | null>(null);
  // Captured once: StrictMode re-runs this effect after focus has already
  // moved inside, and the re-run must not mistake our own button for the opener.
  const openerRef = useRef<Element | null | undefined>(undefined);
  useEffect(() => {
    const root = ref.current;
    if (!root) return;
    if (openerRef.current === undefined) openerRef.current = document.activeElement;
    const opener = openerRef.current;
    if (!root.contains(document.activeElement)) {
      const controls = focusableWithin(root);
      const target = [root.querySelector("[data-autofocus]"), root.querySelector(".btn-primary")].find(canFocus)
        ?? controls.find((el) => !el.classList.contains("btn-danger")) ?? controls[0];
      target?.focus();
    }
    return () => {
      // Answering removes the callout and drops focus to the body; hand it
      // back to whatever had it before, as Modal does.
      const focused = document.activeElement;
      const lost = !focused || focused === document.body || !focused.isConnected;
      if (lost && opener !== root && canFocus(opener)) opener.focus();
    };
  }, []);
  const props = {
    ref: (el: HTMLElement | null) => { ref.current = el; },
    role: "alert",
    "aria-label": label,
    "data-confirm-callout": "",
    className,
    children,
  };
  return as === "section" ? <section {...props} /> : <div {...props} />;
}

/** Confirmation for AMF's resource gate ("needs_approval" responses), and
 * for other single-step confirms. `tone="danger"` when confirming loses
 * something (see the button-tone rule above). */
export function ApprovalDialog({
  label,
  title,
  message,
  confirmLabel,
  cancelLabel = "Cancel",
  busy,
  tone = "warning",
  onConfirm,
  onCancel,
}: {
  label: string;
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  busy?: boolean;
  tone?: "warning" | "danger";
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Modal
      label={label}
      title={title}
      size="sm"
      onClose={onCancel}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onCancel}>{cancelLabel}</button>
          <button className={`btn btn-${tone}`} disabled={busy} onClick={onConfirm}>
            {busy && <Spinner />}
            {confirmLabel}
          </button>
        </>
      }
    >
      <div className="callout callout-warning">
        <Icon name="alert" />
        <p>{message}</p>
      </div>
    </Modal>
  );
}

export interface MenuItem {
  label: string;
  icon?: IconName;
  onSelect: () => void;
  danger?: boolean;
  disabled?: boolean;
  hint?: string;
  /** Registry shortcut shown at the right edge (display only; the binding
   *  lives in `shortcuts.ts`, so the hint cannot drift from the key). */
  shortcut?: ShortcutId;
  /** Tooltip for the item (a disabled reason belongs in `hint`, which is visible). */
  title?: string;
}

/** A menu row: an item, a section heading that groups the items after it, or a divider. */
export type MenuEntry = MenuItem | { section: string } | "separator";

function isMenuItem(entry: MenuEntry): entry is MenuItem {
  return typeof entry === "object" && "label" in entry;
}

/** Height a menu will need before it is measured: rows are 30px plus padding
 * and gaps, a section heading about 24px, a separator 9px. */
export function menuHeightEstimate(entries: MenuEntry[]): number {
  let height = 12;
  for (const entry of entries) height += entry === "separator" ? 9 : isMenuItem(entry) ? 34 : 24;
  return height;
}

const MENU_GAP = 4;
const MENU_EDGE = 8;

/** A button that opens a small fixed-position menu. Fixed positioning keeps
 * the menu from being clipped by the scrolling panels it sits inside.
 *
 * Keyboard: Enter, Space or ArrowDown on the trigger open it on the first
 * enabled item (ArrowUp: the last); ArrowUp/ArrowDown/Home/End move; Enter
 * selects; Escape and Tab close it and return focus to the trigger. */
export function Menu({
  label,
  icon = "more",
  text,
  items,
  className = "btn btn-icon btn-ghost",
  title,
  indicator = false,
}: {
  label: string;
  /** Trigger icon; `null` for a text-only trigger. */
  icon?: IconName | null;
  text?: string;
  items: MenuEntry[];
  className?: string;
  /** Tooltip on the trigger. */
  title?: string;
  /** Small dot on the trigger, e.g. while an action inside is busy. */
  indicator?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({});
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const focusEnd = useRef<"first" | "last">("first");
  const menuId = useId();
  const estimate = menuHeightEstimate(items);

  const close = useCallback((returnFocus: boolean) => {
    setOpen(false);
    if (returnFocus) buttonRef.current?.focus();
  }, []);

  const enabledItems = () =>
    Array.from(menuRef.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not(:disabled)') ?? []);

  useLayoutEffect(() => {
    if (!open || !buttonRef.current) return;
    const rect = buttonRef.current.getBoundingClientRect();
    // Measure the rendered menu (scrollHeight ignores a previous max-height);
    // fall back to the estimate where layout is unavailable.
    const measured = menuRef.current?.scrollHeight ?? 0;
    const height = measured > 0 ? measured : estimate;
    const spaceBelow = window.innerHeight - rect.bottom - MENU_GAP - MENU_EDGE;
    const spaceAbove = rect.top - MENU_GAP - MENU_EDGE;
    const below = height <= spaceBelow || spaceBelow >= spaceAbove;
    setPosition({
      right: Math.max(MENU_EDGE, window.innerWidth - rect.right),
      ...(below
        ? { top: rect.bottom + MENU_GAP }
        : { bottom: window.innerHeight - rect.top + MENU_GAP }),
      ...(height > (below ? spaceBelow : spaceAbove)
        ? { maxHeight: Math.max(120, below ? spaceBelow : spaceAbove), overflowY: "auto" as const }
        : {}),
    });
  }, [open, estimate]);

  useEffect(() => {
    const menu = menuRef.current;
    if (!open || !menu) return;
    const unregister = registerLayer({
      el: menu,
      modal: false,
      onEscape: () => close(true),
      onTab: (event) => {
        event.preventDefault();
        close(true);
      },
    });
    const candidates = enabledItems();
    const start = focusEnd.current === "last" ? candidates[candidates.length - 1] : candidates[0];
    (start ?? menu).focus({ preventScroll: true });

    const onPointer = (event: Event) => {
      const target = event.target as Node | null;
      if (target && (menu.contains(target) || buttonRef.current?.contains(target))) return;
      setOpen(false);
    };
    const closeNow = () => setOpen(false);
    const onScroll = (event: Event) => {
      // Scrolling the menu's own overflow is not a reason to close it.
      if (event.target instanceof Node && menu.contains(event.target)) return;
      setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    window.addEventListener("resize", closeNow);
    document.addEventListener("scroll", onScroll, true);
    return () => {
      unregister();
      document.removeEventListener("mousedown", onPointer);
      window.removeEventListener("resize", closeNow);
      document.removeEventListener("scroll", onScroll, true);
    };
  }, [open, close]);

  const onMenuKey = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const candidates = enabledItems();
    if (candidates.length === 0) return;
    const index = candidates.indexOf(document.activeElement as HTMLElement);
    let next: number | null = null;
    if (event.key === "ArrowDown") next = index < 0 ? 0 : (index + 1) % candidates.length;
    else if (event.key === "ArrowUp") next = index < 0 ? candidates.length - 1 : (index - 1 + candidates.length) % candidates.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = candidates.length - 1;
    if (next === null) return;
    event.preventDefault();
    candidates[next].focus({ preventScroll: true });
  };

  const onTriggerKey = (event: ReactKeyboardEvent<HTMLButtonElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    focusEnd.current = event.key === "ArrowUp" ? "last" : "first";
    setOpen(true);
  };

  // The label names the item and the hint describes it, so a hint such as
  // "Experimental" or a disabled reason never becomes part of its name.
  const renderItem = (item: MenuItem, index: number) => (
    <button
      key={`item:${item.label}`}
      role="menuitem"
      type="button"
      tabIndex={-1}
      title={item.title}
      aria-labelledby={`${menuId}-label-${index}`}
      aria-describedby={item.hint ? `${menuId}-hint-${index}` : undefined}
      aria-keyshortcuts={item.shortcut ? shortcutAria(item.shortcut) : undefined}
      className={`menu-item${item.danger ? " menu-item-danger" : ""}`}
      disabled={item.disabled}
      onClick={() => {
        // Back on the trigger first, so a dialog the item opens returns
        // focus there when it closes.
        close(true);
        item.onSelect();
      }}
    >
      {item.icon && <Icon name={item.icon} />}
      <span id={`${menuId}-label-${index}`} className="menu-item-label">{item.label}</span>
      {item.hint && <span id={`${menuId}-hint-${index}`} className="menu-item-hint">{item.hint}</span>}
      {item.shortcut && <kbd className="menu-item-shortcut" aria-hidden="true">{shortcutLabel(item.shortcut)}</kbd>}
    </button>
  );

  // Section headings label a role="group" holding the items that follow,
  // up to the next heading or separator.
  const rows: ReactNode[] = [];
  let group: { key: string; heading: string; children: ReactNode[] } | null = null;
  const flush = () => {
    if (!group) return;
    const headingId = `${menuId}-${group.key}`;
    rows.push(
      <div key={group.key} role="group" aria-labelledby={headingId}>
        <div id={headingId} role="presentation" className="menu-section-label">{group.heading}</div>
        {group.children}
      </div>,
    );
    group = null;
  };
  items.forEach((entry, index) => {
    if (entry === "separator") {
      flush();
      rows.push(<div key={`sep:${index}`} role="separator" className="menu-sep" />);
    } else if (isMenuItem(entry)) {
      if (group) group.children.push(renderItem(entry, index));
      else rows.push(renderItem(entry, index));
    } else {
      flush();
      group = { key: `section:${index}`, heading: entry.section, children: [] };
    }
  });
  flush();

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className={className}
        aria-label={label}
        title={title}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => {
          focusEnd.current = "first";
          setOpen((value) => !value);
        }}
        onKeyDown={onTriggerKey}
      >
        {icon && <Icon name={icon} />}
        {text && <span>{text}</span>}
        {text && <Icon name="chevronDown" size={14} />}
        {indicator && <span className="menu-trigger-dot" aria-hidden="true" />}
      </button>
      {open && (
        <div
          ref={menuRef}
          id={menuId}
          role="menu"
          tabIndex={-1}
          aria-label={label}
          className="menu"
          style={position}
          onKeyDown={onMenuKey}
        >
          {rows}
        </div>
      )}
    </>
  );
}

/** Mutually-exclusive choice rendered as a row of pills. One Tab stop (the
 * chosen pill); arrow keys, Home and End move the choice like a radio group. */
export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string; hint?: string }[];
  onChange: (value: T) => void;
}) {
  const selected = options.findIndex((option) => option.value === value);
  const tabStop = selected >= 0 ? selected : 0;
  const onKey = (event: ReactKeyboardEvent<HTMLButtonElement>, index: number) => {
    const count = options.length;
    let next: number | null = null;
    if (event.key === "ArrowRight" || event.key === "ArrowDown") next = (index + 1) % count;
    else if (event.key === "ArrowLeft" || event.key === "ArrowUp") next = (index - 1 + count) % count;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = count - 1;
    if (next === null) return;
    event.preventDefault();
    const buttons = event.currentTarget.parentElement?.querySelectorAll<HTMLElement>('[role="radio"]');
    buttons?.[next]?.focus();
    if (options[next].value !== value) onChange(options[next].value);
  };
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map((option, index) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={value === option.value}
          tabIndex={index === tabStop ? 0 : -1}
          title={option.hint}
          className={value === option.value ? "segment segment-active" : "segment"}
          onClick={() => onChange(option.value)}
          onKeyDown={(event) => onKey(event, index)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** Keyboard handler for a `role="tablist"`: Left/Right (and Home/End) move
 * focus between its tabs without activating them; Enter or Space activates.
 * Pair it with a roving `tabIndex` (0 on the selected tab, -1 elsewhere). */
export function onTablistKeyDown(event: ReactKeyboardEvent<HTMLElement>) {
  const tabs = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="tab"]'))
    .filter((tab) => canFocus(tab));
  const index = tabs.indexOf(document.activeElement as HTMLElement);
  if (index < 0 || tabs.length === 0) return;
  let next: number | null = null;
  if (event.key === "ArrowRight") next = (index + 1) % tabs.length;
  else if (event.key === "ArrowLeft") next = (index - 1 + tabs.length) % tabs.length;
  else if (event.key === "Home") next = 0;
  else if (event.key === "End") next = tabs.length - 1;
  if (next === null) return;
  event.preventDefault();
  tabs[next].focus();
}

export function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: ReactNode;
  children: ReactNode;
}) {
  return (
    <label className="field">
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="field-hint">{hint}</span>}
    </label>
  );
}

/** The one "Experimental" marker for features that are new and still
 * changing. Its text is part of the surrounding control's name, so what a
 * screen reader announces matches what is on screen. */
export function ExperimentalBadge({ title = "New and still changing" }: { title?: string }) {
  return <span className="tag tag-accent badge-experimental" title={title}>Experimental</span>;
}

/** A compact list filter with a search icon and placeholder, the same
 * pattern as the sidebar's project filter. The label is visually hidden
 * but still names the field. */
export function FilterField({
  label,
  value,
  onChange,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
}) {
  return (
    <label className="nav-filter filter-field">
      <Icon name="search" size={14} />
      <span className="visually-hidden">{label}</span>
      <input type="text" placeholder={label} value={value} onChange={(event) => onChange(event.target.value)} />
    </label>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  badge,
  hint,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  /** Shown after the label, e.g. an {@link ExperimentalBadge}. */
  badge?: ReactNode;
  hint?: string;
  disabled?: boolean;
}) {
  return (
    <label className="switch-row">
      <input
        type="checkbox"
        role="switch"
        className="switch"
        checked={checked}
        disabled={disabled}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>
        <span className="switch-label">{label}{badge && <> {badge}</>}</span>
        {hint && <span className="field-hint">{hint}</span>}
      </span>
    </label>
  );
}

export interface Toast {
  id: number;
  tone: "error" | "info";
  title: string;
  message: string;
  action?: { label: string; onClick: () => void };
  /** The action this toast reports on. A newer toast with the same key
   * replaces it, and the action's later success clears it. */
  key?: string;
}

/** How long a toast stays when nobody hovers or focuses it. */
export const TOAST_INFO_MS = 6_000;
export const TOAST_ERROR_MS = 30_000;
/** Most toasts on screen at once. */
export const TOAST_CAP = 4;

/** Adds `toast`, replacing one with the same key. Over the cap, the oldest
 * info toast goes first: an error is only dropped for another error. */
export function addToast(current: Toast[], toast: Toast, cap = TOAST_CAP): Toast[] {
  const next = [...current.filter((existing) => !toast.key || existing.key !== toast.key), toast];
  while (next.length > cap) {
    const info = next.findIndex((existing) => existing.tone === "info");
    next.splice(info >= 0 ? info : 0, 1);
  }
  return next;
}

function ToastView({ toast, onDismiss }: { toast: Toast; onDismiss: (id: number) => void }) {
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const remaining = useRef(toast.tone === "error" ? TOAST_ERROR_MS : TOAST_INFO_MS);
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;
  const paused = hovered || focused;

  // The dismiss timer runs only while the toast is neither hovered nor
  // focused, and resumes with the time it had left.
  useEffect(() => {
    if (paused) return;
    const started = Date.now();
    const timer = window.setTimeout(() => dismissRef.current(toast.id), remaining.current);
    return () => {
      window.clearTimeout(timer);
      remaining.current = Math.max(0, remaining.current - (Date.now() - started));
    };
  }, [paused, toast.id]);

  return (
    <div
      role={toast.tone === "error" ? "alert" : "status"}
      className={`toast toast-${toast.tone}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onFocus={() => setFocused(true)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocused(false);
      }}
    >
      <Icon name={toast.tone === "error" ? "alert" : "check"} />
      <div className="toast-text">
        <strong>{toast.title}</strong>
        <span>{toast.message}</span>
      </div>
      {toast.action && (
        <button className="btn btn-sm btn-ghost" onClick={toast.action.onClick}>
          {toast.action.label}
        </button>
      )}
      <button
        className="btn btn-icon btn-ghost btn-sm"
        aria-label="Dismiss"
        onClick={() => onDismiss(toast.id)}
      >
        <Icon name="x" size={14} />
      </button>
    </div>
  );
}

/** The toast stack. It stays reachable while a dialog is open
 * (`data-overlay-exempt`), so an error can still be read and dismissed. */
export function Toasts({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className="toasts" aria-live="polite" data-overlay-exempt="">
      {toasts.map((toast) => (
        <ToastView key={toast.id} toast={toast} onDismiss={onDismiss} />
      ))}
    </div>
  );
}
