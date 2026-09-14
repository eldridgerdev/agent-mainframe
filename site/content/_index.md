+++
title = "Agent Mainframe"
template = "index.html"

[extra]
headline = "Run multiple AI coding agents in parallel, without losing track of them."
subhead = "AMF is a terminal dashboard for managing Claude Code, Codex, OpenCode, and Pi sessions. It organizes work by project and feature, creates isolated git worktrees when needed, and shows which agents are asking a question and which have finished work waiting to be reviewed."
install_cmd = "cargo install agent-mainframe --locked\namf"
install_note = "The crate installs the amf binary. Keep ~/.cargo/bin in your PATH. This source installation requires Rust, a C compiler, and tmux."
hero_image = "images/dashboard.png"
hero_image_alt = "AMF dashboard showing a project with four concurrent Claude agent sessions, each on its own git worktree"

agents = ["Claude Code", "Codex", "OpenCode", "Pi"]

[[extra.features]]
title = "One dashboard, many agents"
body = "Run several coding-agent sessions side by side and see at a glance which ones need attention."

[[extra.features]]
title = "Isolated by design"
body = "Concurrent features stay isolated with git branches and worktrees, so parallel agents never step on each other's changes."

[[extra.features]]
title = "Everything embedded"
body = "Agent terminals, shells, Neovim, VS Code, and custom sessions all run inside the same dashboard."

[[extra.features]]
title = "Guided workflows"
body = "Guided planning, supervised edits, final diff review, and GitHub PR review workflows, without leaving the terminal."

[[extra.features]]
title = "Understand code you didn't write"
body = "Browse a repository read-only and ask an agent about any file, hunk, or line range, with answers kept per project."

[[extra.features]]
title = "Reusable building blocks"
body = "Reusable prompts, scoped TODO lists, themes, lifecycle hooks, and workspace presets carry across features and projects."
+++

## About the project

🤖 AMF is a terminal-based control center for running AI coding agents (Claude Code, Codex, OpenCode, Pi) against your own repos — built for everyday developers, not teams burning enterprise budgets on the priciest frontier models. It's designed around subscription plans people already have through work or pay for out of pocket, so you get real leverage from AI-assisted coding without needing a premium API tier. Full transparency is the core value: every prompt is visible and editable, every diff is reviewable before it lands, and nothing happens in a black box — whether you want to read every line an agent writes or just skim the summary and trust the process. The goal is to make AI-assisted development practical and honest for the average developer, not just the well-funded ones.
