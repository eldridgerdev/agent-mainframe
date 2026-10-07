import { ReactNode } from "react";
import type {
  CollapseTarget,
  Feature,
  FeatureSession,
  Project,
  SessionTarget,
  SidebarFeature,
  SidebarPr,
  SidebarSession,
  SidebarSnapshot,
  WorkspaceSnapshot,
} from "./api";
import { sessionRunning, stoppedSessionCount } from "./SessionControls";
import { Icon, Spinner, StatusDot } from "./ui";

// The workspace tree: the TUI dashboard tree (`src/ui/list.rs::draw`) in the
// sidebar. Every row shows what the TUI row shows; the compact line carries
// glyphs and badges, the full text lives in tooltips, and an expanded
// feature adds a detail line (its workdir) above its session rows.

/** The status glyph, in the TUI's precedence order. The TUI's hook-running
 *  spinner and attention reason are TUI-process state, so the GUI has no
 *  source for them and they never appear here (`gui_contract::sidebar`). */
export type GlyphKind =
  | "worktree_script"
  | "deleting"
  | "waiting"
  | "thinking"
  | "ready"
  | "active"
  | "idle"
  | "stopped";

const GLYPH_LABEL: Record<GlyphKind, string> = {
  worktree_script: "Running worktree script",
  deleting: "Deleting",
  waiting: "Waiting for input",
  thinking: "Agent working",
  ready: "Ready",
  active: "Running",
  idle: "Idle",
  stopped: "Stopped",
};

export function featureGlyph(
  feature: Feature,
  derived: SidebarFeature | undefined,
  deleting: boolean,
): GlyphKind {
  if (feature.pending_worktree_script) return "worktree_script";
  if (deleting) return "deleting";
  if (derived?.waiting_for_input) return "waiting";
  if (derived?.thinking && feature.status !== "stopped") return "thinking";
  if (feature.ready) return "ready";
  return feature.status;
}

function FeatureGlyph({ kind }: { kind: GlyphKind }) {
  const label = GLYPH_LABEL[kind];
  const body = (() => {
    switch (kind) {
      case "worktree_script": return "⚙";
      case "deleting":
      case "thinking": return <Spinner />;
      case "waiting": return "?";
      case "ready": return "✓";
      default: return <StatusDot status={kind} />;
    }
  })();
  return (
    <span className={`tree-glyph glyph-${kind}`} role="img" aria-label={label} title={label}>
      {body}
    </span>
  );
}

export function prLabel(pr: SidebarPr): string {
  if (pr.state !== "open") return `PR #${pr.number} ${pr.state}`;
  return pr.unresolved_threads === null
    ? `PR #${pr.number}`
    : `PR #${pr.number} · ${pr.unresolved_threads} open`;
}

function prTone(pr: SidebarPr): string {
  if (pr.state !== "open") return `pr-${pr.state}`;
  return pr.unresolved_threads === 0 ? "pr-clear" : "pr-open";
}

/** The TUI's session-kind icons in its Nerd Font mode; the window bundles
 *  the symbols font (styles.css "AMF Symbols"), so they always render. */
export function sessionKindIcon(session: FeatureSession, derived: SidebarSession | undefined): string {
  switch (session.kind) {
    case "claude":
    case "codex":
    case "opencode":
    case "pi": return "*";
    case "terminal": return ">";
    case "nvim": return "\ue6ae";
    case "vscode": return "\ue70c";
    case "todos": return "\uf0ae";
    default: return derived?.icon_nerd ?? derived?.icon ?? "$";
  }
}

const MODE_LABEL: Record<Feature["mode"], string> = {
  vibeless: "vibeless",
  vibe: "vibe",
  supervibe: "supervibe",
};

export interface PausedPlan {
  /** The existing feature an on-demand interview belongs to. */
  featureId: string | null;
  /** The project a creation-time interview's new feature will join. */
  projectName: string | null;
  featureName: string;
}

export interface SidebarTreeProps {
  projects: Project[];
  sidebar: SidebarSnapshot | undefined;
  stoppedSessionIds: string[];
  /** The selected row: a project, a feature, or one of a feature's tabs. */
  selection:
    | { kind: "project"; projectId: string }
    | { kind: "feature"; projectId: string; featureId: string; tab: string | undefined }
    | null;
  /** The feature whose deletion this window is running. */
  deletingFeatureId: string | null;
  pausedPlan: PausedPlan | null;
  onSelectProject: (projectId: string) => void;
  onSelectFeature: (projectId: string, featureId: string) => void;
  onSelectSession: (target: SessionTarget, kind: string) => void;
  /** Open a feature's VS Code tab (its tracked editor windows). */
  onSelectEditors?: (projectId: string, featureId: string) => void;
  onToggleCollapsed: (target: CollapseTarget, collapsed: boolean) => void;
  onResumePlan: () => void;
  onCreateFeature: (projectId: string) => void;
  /** Extra compact markers a workflow adds to a feature's main line. */
  renderFeatureExtra?: (feature: Feature) => ReactNode;
}

/** Running features first, then idle, then stopped; stable within each group
 *  (the GUI's established order, kept rather than the TUI's store order). */
const STATUS_RANK: Record<Feature["status"], number> = { active: 0, idle: 1, stopped: 2 };
export const byStatus = (features: Feature[]) =>
  [...features].sort((a, b) => STATUS_RANK[a.status] - STATUS_RANK[b.status]);

/** `snapshot` with one row's collapse flag set: shown at once, before the
 *  store write that makes it shared with the TUI comes back. */
export function withCollapsed(
  snapshot: WorkspaceSnapshot,
  target: CollapseTarget,
  collapsed: boolean,
): WorkspaceSnapshot {
  return {
    ...snapshot,
    projects: snapshot.projects.map((project) => {
      if (project.id !== target.project_id) return project;
      if (target.feature_id === null) return { ...project, collapsed };
      return {
        ...project,
        features: project.features.map((feature) =>
          feature.id === target.feature_id ? { ...feature, collapsed } : feature),
      };
    }),
  };
}

export default function SidebarTree(props: SidebarTreeProps) {
  return (
    <>
      {props.projects.map((project) => (
        <ProjectNode key={project.id} project={project} {...props} />
      ))}
    </>
  );
}

function ProjectNode({ project, ...props }: SidebarTreeProps & { project: Project }) {
  const collapsed = project.collapsed ?? false;
  const active = props.selection?.kind === "project" && props.selection.projectId === project.id;
  const repo = props.sidebar?.projects[project.id]?.repo_display ?? project.repo;
  const paused = props.pausedPlan?.projectName === project.name ? props.pausedPlan : null;
  const metaId = `tree-project-${project.id}`;
  return (
    <div className="nav-group">
      <div className={active ? "nav-item nav-item-active" : "nav-item"}>
        <button
          className="nav-chevron"
          aria-label={collapsed ? `Expand ${project.name}` : `Collapse ${project.name}`}
          aria-expanded={!collapsed}
          onClick={() => props.onToggleCollapsed({ project_id: project.id, feature_id: null }, !collapsed)}
        >
          <Icon name={collapsed ? "chevronRight" : "chevronDown"} size={14} />
        </button>
        <button
          className="nav-link"
          aria-describedby={metaId}
          title={`${project.name}\n${project.repo}`}
          onClick={() => props.onSelectProject(project.id)}
        >
          <span className="nav-label">{project.name}</span>
          <span className="nav-count">{project.features.length}</span>
        </button>
      </div>
      <div className="tree-project-meta" id={metaId} title={project.repo}>{repo}</div>
      {paused && (
        <button className="tree-chip tree-chip-warning tree-project-plan" onClick={props.onResumePlan}>
          plan paused: {paused.featureName} · Resume
        </button>
      )}
      {project.features.length === 0 && (
        <button className="tree-empty-hint" onClick={() => props.onCreateFeature(project.id)}>
          No features yet. Add one
        </button>
      )}
      {!collapsed && byStatus(project.features).map((feature) => (
        <FeatureNode key={feature.id} project={project} feature={feature} {...props} />
      ))}
    </div>
  );
}

function FeatureNode({
  project,
  feature,
  ...props
}: SidebarTreeProps & { project: Project; feature: Feature }) {
  const derived = props.sidebar?.features[feature.id];
  const deleting = props.deletingFeatureId === feature.id;
  const glyph = featureGlyph(feature, derived, deleting);
  const collapsed = feature.collapsed ?? true;
  const hasSessions = feature.sessions.length > 0;
  const selected = props.selection?.kind === "feature" && props.selection.featureId === feature.id
    ? props.selection : null;
  const displayName = feature.nickname || feature.name;
  const stopped = stoppedSessionCount(feature);
  const paused = props.pausedPlan?.featureId === feature.id ? props.pausedPlan : null;
  const metaId = `tree-feature-${feature.id}`;
  const select = () => props.onSelectFeature(project.id, feature.id);

  const chips: { key: string; text: ReactNode; className?: string; title?: string; onClick?: () => void }[] = [];
  // The TUI row's order: checkout, issue, PR, usage, badges, age, sessions.
  if (!feature.is_worktree) chips.push({ key: "repo", text: "repo", className: "tree-chip-warning", title: "Runs in the repository checkout, not a worktree" });
  if (derived?.issue) chips.push({ key: "issue", text: derived.issue, className: "tree-chip-info", title: `Created from issue ${derived.issue}` });
  if (derived?.pr) chips.push({ key: "pr", text: prLabel(derived.pr), className: `tree-pr ${prTone(derived.pr)}` });
  if (derived?.usage) chips.push({ key: "usage", text: derived.usage, className: "tree-chip-detail", title: "Agent token usage" });
  chips.push({
    key: "mode",
    text: MODE_LABEL[feature.mode] ?? feature.mode,
    className: `tree-chip-mode mode-${feature.mode}`,
    title: `Mode: ${feature.mode}`,
  });
  if (feature.review) chips.push({ key: "review", text: "review", className: "tree-chip-review" });
  if (feature.plan_mode) chips.push({ key: "plan", text: "plan", className: "tree-chip-info" });
  if (paused) chips.push({ key: "paused", text: "plan paused · Resume", className: "tree-chip-warning tree-chip-action", onClick: props.onResumePlan, title: "Resume the minimized plan interview" });
  if (feature.remote_control) chips.push({ key: "remote", text: "remote", className: "tree-chip-info" });
  if (deleting) chips.push({ key: "deleting", text: "deleting…", className: "tree-chip-danger" });
  if (feature.pending_worktree_script) chips.push({ key: "script", text: "running worktree script…", className: "tree-chip-info" });
  if (derived) chips.push({ key: "age", text: derived.created_age, className: "tree-chip-age", title: feature.created_at ? `Created ${feature.created_at}` : undefined });
  if (hasSessions) chips.push({ key: "sessions", text: `${feature.sessions.length} ${feature.sessions.length === 1 ? "session" : "sessions"}` });
  if (stopped > 0) chips.push({ key: "stopped", text: `${stopped} stopped`, className: "tree-chip-stopped", title: "Sessions stopped on their own" });
  const editors = derived?.editors ?? [];
  if (editors.length > 0) {
    const opening = editors.some((editor) => editor.state === "opening");
    chips.push({
      key: "vscode",
      text: <>{"\ue70c"} {opening ? "VS Code opening…" : editors.length === 1 ? "VS Code" : `VS Code ×${editors.length}`}</>,
      className: "tree-chip-vscode tree-chip-action",
      title: "VS Code windows AMF opened for this feature",
      onClick: props.onSelectEditors && (() => props.onSelectEditors?.(project.id, feature.id)),
    });
  }

  const tooltip = [
    feature.nickname ? `${feature.nickname} (${feature.branch})` : feature.name,
    GLYPH_LABEL[glyph],
    derived?.workdir_display ?? feature.workdir,
    feature.summary ? `${feature.summary}${derived?.summary_age ? ` (${derived.summary_age})` : ""}` : null,
  ].filter(Boolean).join("\n");

  return (
    <div className={selected ? "tree-feature tree-feature-active" : "tree-feature"}>
      <div className="tree-row">
        {hasSessions ? (
          <button
            className="nav-chevron"
            aria-label={collapsed ? `Show sessions of ${displayName}` : `Hide sessions of ${displayName}`}
            aria-expanded={!collapsed}
            onClick={() => props.onToggleCollapsed({ project_id: project.id, feature_id: feature.id }, !collapsed)}
          >
            <Icon name={collapsed ? "chevronRight" : "chevronDown"} size={12} />
          </button>
        ) : <span className="nav-chevron" aria-hidden="true" />}
        <FeatureGlyph kind={glyph} />
        <button
          className={deleting ? "tree-name tree-name-deleting" : "tree-name"}
          aria-describedby={metaId}
          aria-current={selected ? "page" : undefined}
          title={tooltip}
          onClick={select}
        >
          <span className="nav-label">{displayName}</span>
        </button>
        {feature.nickname && <span className="tree-branch" title={feature.branch}>({feature.branch})</span>}
        {derived?.pending_input && (
          <span className="tree-pending" role="img" aria-label="Input requested" title="Input requested">?</span>
        )}
        {props.renderFeatureExtra?.(feature)}
      </div>
      <div className="tree-meta" id={metaId} onClick={select}>
        {chips.map((chip) => chip.onClick ? (
          <button key={chip.key} className={`tree-chip ${chip.className ?? ""}`} title={chip.title}
            onClick={(event) => { event.stopPropagation(); chip.onClick?.(); }}>
            {chip.text}
          </button>
        ) : (
          <span key={chip.key} className={`tree-chip ${chip.className ?? ""}`} title={chip.title}>{chip.text}</span>
        ))}
      </div>
      {feature.summary && (
        <div className="tree-summary" title={tooltip} onClick={select}>— {feature.summary}</div>
      )}
      {!collapsed && hasSessions && (
        <div className="tree-sessions">
          <div className="tree-detail" title={feature.workdir}>{derived?.workdir_display ?? feature.workdir}</div>
          {feature.sessions.map((session) => (
            <SessionNode key={session.id} project={project} feature={feature} session={session}
              active={selected !== null && shownTab(feature, selected.tab) === session.id} {...props} />
          ))}
        </div>
      )}
    </div>
  );
}

/** The indicator's percentage, band and freshness (`Ctx ~74% WARNING
 *  STALE`); the token count after ` · ` stays in its tooltip, so a narrow
 *  sidebar keeps room for the session's name. */
export function contextSummary(text: string): string {
  return text.split(" · ")[0];
}

/** The session whose tab the feature page shows, in that page's own terms:
 *  a remembered tab, else the first terminal session, else the TODO list. */
function shownTab(feature: Feature, tab: string | undefined): string | undefined {
  const todos = feature.sessions.find((session) => session.kind === "todos")?.id;
  if (tab === "todos") return todos;
  if (tab !== undefined && feature.sessions.some((session) => session.id === tab)) return tab;
  return feature.sessions.find((session) => session.kind !== "todos")?.id ?? todos;
}

function SessionNode({
  project,
  feature,
  session,
  active,
  ...props
}: SidebarTreeProps & { project: Project; feature: Feature; session: FeatureSession; active: boolean }) {
  const derived = props.sidebar?.sessions[session.id];
  const native = session.kind === "todos";
  const running = !native && sessionRunning(feature, session, props.stoppedSessionIds);
  const context = derived?.context;
  const state = native ? "" : running ? "running" : "stopped";
  return (
    <div className={active ? "tree-session tree-session-active" : "tree-session"}>
      <button
        className={session.stopped ? "tree-session-row tree-session-stopped" : "tree-session-row"}
        title={[session.label, `${session.kind}${state ? ` — ${state}` : ""}`, context?.text, derived?.status_text]
          .filter(Boolean).join("\n")}
        aria-current={active ? "page" : undefined}
        onClick={() => props.onSelectSession(
          { project_id: project.id, feature_id: feature.id, session_id: session.id }, session.kind)}
      >
        <span className="tree-session-state" aria-hidden="true">
          {native ? null : <StatusDot status={running ? "active" : "stopped"} />}
        </span>
        <span className={`tree-kind kind-${session.kind}`} aria-hidden="true">
          {sessionKindIcon(session, derived)}
        </span>
        <span className="tree-session-label">{session.label}</span>
        {context && (context.pending_reset ? (
          <span className="tree-context ctx-pending" title="The context window was reset; waiting for a fresh sample">
            Ctx resetting
          </span>
        ) : (
          <span className={`tree-context ctx-${context.band}${context.stale ? " ctx-stale" : ""}`} title={context.text}>
            {contextSummary(context.text)}
          </span>
        ))}
      </button>
      {derived?.status_text && <div className="tree-session-status">{derived.status_text}</div>}
    </div>
  );
}
