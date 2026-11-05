import type { ReactNode } from "react";
import type { Feature } from "./api";
import { Icon, Spinner, type IconName } from "./ui";

export function PageHeader({
  icon,
  title,
  subtitle,
  badge,
  actions,
  crumb,
}: {
  icon?: IconName;
  title: string;
  subtitle?: ReactNode;
  badge?: ReactNode;
  actions?: ReactNode;
  crumb?: ReactNode;
}) {
  return (
    <header className="page-header">
      <div className="page-title">
        {crumb && <div className="crumb">{crumb}</div>}
        <div className="page-title-row">
          {icon && <span className="page-icon"><Icon name={icon} size={18} /></span>}
          <h1 title={title}>{title}</h1>
          {badge}
        </div>
        {subtitle && <div className="page-subtitle">{subtitle}</div>}
      </div>
      {actions && <div className="page-actions">{actions}</div>}
    </header>
  );
}

export interface Lifecycle {
  starting: boolean;
  stopping: boolean;
  onStart: () => void;
  onStop: () => void;
}

export function StartStopButton({ status, starting, stopping, onStart, onStop, size, title }: Lifecycle & {
  status: Feature["status"];
  size?: "sm";
  /** Tooltip on the Stop button, e.g. how many sessions it ends. */
  title?: string;
}) {
  // In lists every row has one of these, so they stay quiet; the feature
  // header's single Start is the primary action on its page.
  const list = size === "sm";
  return status === "stopped" ? (
    <button className={`btn btn-sm ${list ? "btn-secondary" : "btn-primary"}`} onClick={onStart} disabled={starting}>
      {starting ? <Spinner /> : <Icon name="play" size={14} />}
      {starting ? "Starting…" : "Start"}
    </button>
  ) : (
    <button className={`btn btn-sm ${list ? "btn-ghost" : "btn-secondary"}`} onClick={onStop} disabled={stopping} title={title}>
      {stopping ? <Spinner /> : <Icon name="stop" size={14} />}
      {stopping ? "Stopping…" : "Stop"}
    </button>
  );
}
