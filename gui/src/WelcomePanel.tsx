import { useQuery } from "@tanstack/react-query";
import { allowedHarnesses } from "./api";
import AgentSetupNotice from "./AgentSetupNotice";
import { Icon } from "./ui";

/** First run: what AMF is, the three steps to a working agent, and (when no
 * agent can start) where to get one. Shown until the first project exists. */
export default function WelcomePanel({ onAddProject, onOpenAvailability }: {
  onAddProject: () => void;
  onOpenAvailability: () => void;
}) {
  // The same answer the New project form reads: agents allowed and installed.
  const agents = useQuery({ queryKey: ["allowed-harnesses", null], queryFn: () => allowedHarnesses(null), retry: false });
  return <section className="welcome" aria-labelledby="welcome-title">
    <h2 id="welcome-title">Welcome to AMF</h2>
    <p className="welcome-lead">
      AMF runs AI coding agents (Claude Code, Codex, OpenCode or Pi) on your repositories, each piece of work on its own branch.
    </p>
    {agents.isSuccess && agents.data.length === 0 && <AgentSetupNotice checking={agents.isFetching}
      onCheckAgain={() => void agents.refetch()} onOpenAvailability={onOpenAvailability} />}
    <ol className="welcome-steps" aria-label="Getting started">
      <li><strong>Add a project</strong><span>Any git repository on this computer.</span></li>
      <li><strong>Create a feature</strong><span>A branch with its own agent sessions, so work stays separate.</span></li>
      <li><strong>Send a prompt</strong><span>The agent works in a terminal; review its changes in Changes and Final Review.</span></li>
    </ol>
    <button className="btn btn-primary" onClick={onAddProject}>
      <Icon name="plus" /> Add a project
    </button>
  </section>;
}
