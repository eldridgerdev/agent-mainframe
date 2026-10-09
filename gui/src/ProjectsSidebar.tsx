import type { ReactNode } from "react";
import { useSidebarCollapsed, useSidebarShortcut } from "./sidebarPrefs";
import { Icon } from "./ui";

export default function ProjectsSidebar({ children }: { children: ReactNode }) {
  const [collapsed, setCollapsed] = useSidebarCollapsed("projectsSidebar");
  useSidebarShortcut("projectsSidebar", () => setCollapsed(!collapsed));
  const label = collapsed ? "Show projects sidebar" : "Hide projects sidebar";
  return (
    <aside className={`sidebar${collapsed ? " sidebar-collapsed" : ""}`} aria-label="Projects sidebar">
      <header className="brand">
        {!collapsed && <><span className="brand-mark">A</span><span className="brand-name">Agent Mainframe</span></>}
        <button type="button" className="btn btn-ghost btn-icon btn-sm projects-sidebar-toggle"
          aria-label={label} aria-expanded={!collapsed}
          title={`${label} (Alt+Shift+P outside terminal and text inputs)`}
          onClick={() => setCollapsed(!collapsed)}>
          <span style={{ display: "flex", transform: collapsed ? undefined : "rotate(180deg)" }}><Icon name="chevronRight" size={14} /></span>
        </button>
      </header>
      <div className="sidebar-content" hidden={collapsed}>{children}</div>
    </aside>
  );
}
