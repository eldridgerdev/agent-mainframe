import {
  CSSProperties,
  ReactNode,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { FeatureStatus } from "./api";

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
  inbox: "M22 12h-6l-2 3h-4l-2-3H2M5.5 5h13L22 12v6a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2v-6z",
  file: "M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9zM14 3v6h6",
} as const;

export type IconName = keyof typeof ICONS;

export function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  return (
    <svg
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

/** Centered dialog. `onClose` runs on Esc and on a backdrop click unless
 * `dismissable` is false (used where closing would lose an answer). */
export function Modal({
  label,
  title,
  subtitle,
  onClose,
  children,
  footer,
  size = "md",
  dismissable = true,
  headerActions,
  onSubmit,
}: {
  label: string;
  title: ReactNode;
  subtitle?: ReactNode;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  size?: "sm" | "md" | "lg";
  dismissable?: boolean;
  headerActions?: ReactNode;
  /** Makes the body and footer one form, so Enter submits. */
  onSubmit?: () => void;
}) {
  useEffect(() => {
    if (!dismissable) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [dismissable, onClose]);

  const content = (
    <>
      <div className="modal-body">{children}</div>
      {footer && <footer className="modal-footer">{footer}</footer>}
    </>
  );

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(event) => {
        if (dismissable && event.target === event.currentTarget) onClose();
      }}
    >
      <div role="dialog" aria-label={label} aria-modal="true" className={`modal modal-${size}`}>
        <header className="modal-header">
          <div>
            <h2>{title}</h2>
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

/** Confirmation for AMF's resource gate ("needs_approval" responses). */
export function ApprovalDialog({
  label,
  title,
  message,
  confirmLabel,
  cancelLabel = "Cancel",
  busy,
  onConfirm,
  onCancel,
}: {
  label: string;
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  busy?: boolean;
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
          <button className="btn btn-warning" disabled={busy} onClick={onConfirm}>
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
}

/** A button that opens a small fixed-position menu. Fixed positioning keeps
 * the menu from being clipped by the scrolling panels it sits inside. */
export function Menu({
  label,
  icon = "more",
  text,
  items,
  className = "btn btn-icon btn-ghost",
}: {
  label: string;
  icon?: IconName;
  text?: string;
  items: MenuItem[];
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<CSSProperties>({});
  const buttonRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (!open || !buttonRef.current) return;
    const rect = buttonRef.current.getBoundingClientRect();
    const estimated = items.length * 34 + 12;
    const below = rect.bottom + estimated + 8 < window.innerHeight;
    setPosition({
      right: Math.max(8, window.innerWidth - rect.right),
      ...(below
        ? { top: rect.bottom + 4 }
        : { bottom: window.innerHeight - rect.top + 4 }),
    });
  }, [open, items.length]);

  useEffect(() => {
    if (!open) return;
    const close = (event: Event) => {
      const target = event.target as Node | null;
      if (target && (menuRef.current?.contains(target) || buttonRef.current?.contains(target))) return;
      setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    const closeNow = () => setOpen(false);
    document.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", closeNow);
    document.addEventListener("scroll", closeNow, true);
    return () => {
      document.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", closeNow);
      document.removeEventListener("scroll", closeNow, true);
    };
  }, [open]);

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className={className}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <Icon name={icon} />
        {text && <span>{text}</span>}
        {text && <Icon name="chevronDown" size={14} />}
      </button>
      {open && (
        <div ref={menuRef} role="menu" className="menu" style={position}>
          {items.map((item) => (
            <button
              key={item.label}
              role="menuitem"
              type="button"
              className={`menu-item${item.danger ? " menu-item-danger" : ""}`}
              disabled={item.disabled}
              onClick={() => {
                setOpen(false);
                item.onSelect();
              }}
            >
              {item.icon && <Icon name={item.icon} />}
              <span className="menu-item-label">{item.label}</span>
              {item.hint && <span className="menu-item-hint">{item.hint}</span>}
            </button>
          ))}
        </div>
      )}
    </>
  );
}

/** Mutually-exclusive choice rendered as a row of pills. */
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
  return (
    <div className="segmented" role="radiogroup" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={value === option.value}
          title={option.hint}
          className={value === option.value ? "segment segment-active" : "segment"}
          onClick={() => onChange(option.value)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
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

export function Switch({
  checked,
  onChange,
  label,
  hint,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  hint?: string;
}) {
  return (
    <label className="switch-row">
      <input
        type="checkbox"
        className="switch"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span>
        <span className="switch-label">{label}</span>
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
}

export function Toasts({
  toasts,
  onDismiss,
}: {
  toasts: Toast[];
  onDismiss: (id: number) => void;
}) {
  return (
    <div className="toasts">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          role={toast.tone === "error" ? "alert" : "status"}
          className={`toast toast-${toast.tone}`}
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
      ))}
    </div>
  );
}
