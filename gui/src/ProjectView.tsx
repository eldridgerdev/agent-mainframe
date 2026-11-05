import type { ReactNode } from "react";
import type { AgentSlug, Feature, ModeSlug, Project } from "./api";
import { byStatus } from "./SidebarTree";
import { stoppedSessionCount } from "./SessionControls";
import { EmptyState, Icon, Menu, StatusDot } from "./ui";
import { PageHeader, StartStopButton, type Lifecycle } from "./WorkspaceControls";

export default function ProjectView({
  project,
  harnessName,
  modeName,
  lifecycle,
  onOpenFeature,
  onNewFeature,
  onPrTriage,
  onIssues,
  onPrReview,
  onAgentAvailability,
  onProjectPreference,
  todoPanel,
}: {
  project: Project;
  harnessName: (slug: AgentSlug) => string;
  modeName: (slug: ModeSlug) => string;
  lifecycle: (feature: Feature) => Lifecycle;
  onOpenFeature: (feature: Feature) => void;
  onNewFeature: () => void;
  onPrTriage: () => void;
  onIssues: () => void;
  onPrReview: () => void;
  onAgentAvailability: () => void;
  onProjectPreference: () => void;
  todoPanel: ReactNode;
}) {
  const running = project.features.filter((feature) => feature.status !== "stopped").length;
  return (
    <div className="page">
      <PageHeader
        icon="folder"
        title={project.name}
        subtitle={
          <>
            <span className="mono">{project.repo}</span>
            {project.is_git && <span className="tag">git</span>}
          </>
        }
        actions={
          <>
            {project.is_git && <button className="btn btn-ghost btn-sm" aria-label="Issues" title="Issues" onClick={onIssues}>
              <Icon name="issue" size={14} /><span className="btn-label-optional">Issues</span>
            </button>}
            {project.is_git && <Menu
              label="Pull request tools"
              text="Pull request"
              icon={null}
              className="btn btn-ghost btn-sm"
              title="Read feedback on a pull request, or review one"
              items={[
                { label: "Review comments", icon: "gitPullRequest", title: "Read and reply to review feedback on this pull request (PR Triage in the terminal app)", onSelect: onPrTriage },
                { label: "Write a review", icon: "gitCompare", title: "Comment on someone else's pull request and post your review (PR Review in the terminal app)", onSelect: onPrReview },
              ]}
            />}
            <Menu
              label="Project settings"
              className="btn btn-ghost btn-icon btn-sm"
              title="Project settings"
              items={[
                { label: "Agent availability", icon: "plug", onSelect: onAgentAvailability },
                { label: "Preferred agent", icon: "star", onSelect: onProjectPreference },
              ]}
            />
            <span className="page-actions-sep" aria-hidden="true" />
            <button className="btn btn-primary btn-sm" onClick={onNewFeature}>
              <Icon name="plus" size={14} /> New feature
            </button>
          </>
        }
      />
      <div className="page-body project-grid">
        <section className="card" aria-label="Features">
          <header className="panel-header">
            <div>
              <h3>Features</h3>
            </div>
            {project.features.length > 0 && (
              <span className="count-pill">{running} of {project.features.length} running</span>
            )}
          </header>
          {project.features.length === 0 ? (
            <EmptyState
              icon="branch"
              title="No features yet"
              action={
                <button className="btn btn-secondary" onClick={onNewFeature}>
                  <Icon name="plus" /> New feature
                </button>
              }
            >
              A feature is a branch with its own agent sessions.
            </EmptyState>
          ) : (
            <ul className="feature-list">
              {byStatus(project.features).map((feature) => (
                <li key={feature.id} className="feature-row">
                  <button className="feature-open" onClick={() => onOpenFeature(feature)}>
                    <StatusDot status={feature.status} />
                    <span className="feature-text">
                      <span className="feature-name">{feature.name}</span>
                      <span className="feature-meta">
                        <span className="mono">{feature.branch}</span>
                        <span className="dot-sep" />
                        {harnessName(feature.agent)}
                        <span className="dot-sep" />
                        {modeName(feature.mode)}
                        {feature.sessions.length > 0 && (
                          <>
                            <span className="dot-sep" />
                            {feature.sessions.length} session{feature.sessions.length === 1 ? "" : "s"}
                          </>
                        )}
                        {stoppedSessionCount(feature) > 0 && (
                          <>
                            <span className="dot-sep" />
                            {stoppedSessionCount(feature)} stopped
                          </>
                        )}
                      </span>
                    </span>
                  </button>
                  <StartStopButton status={feature.status} size="sm" {...lifecycle(feature)} />
                </li>
              ))}
            </ul>
          )}
        </section>
        <div className="card">{todoPanel}</div>
      </div>
    </div>
  );
}
