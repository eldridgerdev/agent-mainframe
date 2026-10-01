//! `amf doctor` — a read-only look at what AMF is putting on this machine.
//!
//! Strictly advisory: it never stops a session, kills a process, deletes a
//! worktree, or writes to the database. Every finding says what it saw and,
//! where there is one, what the user might do about it. The exit code is
//! always `0` — findings are advice, not failures.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::app::AppConfig;
use crate::db::editors::LaunchedEditor;
use crate::extension::{legacy_project_config_path, project_config_path};
use crate::project::{ProjectStatus, ProjectStore};
use crate::resources::limits::{ActiveHarness, LiveHarnesses, active_harness_sessions};
use crate::resources::mem::MemorySnapshot;
use crate::resources::procs;
use crate::tailscale::{self, TailscaleCli, TailscaleStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Nothing to do.
    Ok,
    /// Worth knowing; not a problem yet.
    Notice,
    /// Likely to bite: memory pressure, or resources nothing is using.
    Warn,
}

impl Severity {
    fn marker(self) -> &'static str {
        match self {
            Severity::Ok => "ok  ",
            Severity::Notice => "note",
            Severity::Warn => "warn",
        }
    }
}

/// One check's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    /// Stable identifier for scripts (`agents`, `memory`, `swap`, …).
    pub id: &'static str,
    pub severity: Severity,
    /// What was measured.
    pub summary: String,
    /// Supporting lines: the named sessions, worktrees, or processes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub detail: Vec<String>,
    /// What the user might do. Absent when there is nothing to suggest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advice: Option<String>,
}

impl Finding {
    fn new(id: &'static str, severity: Severity, summary: impl Into<String>) -> Self {
        Self {
            id,
            severity,
            summary: summary.into(),
            detail: Vec::new(),
            advice: None,
        }
    }

    fn with_detail(mut self, detail: Vec<String>) -> Self {
        self.detail = detail;
        self
    }

    fn with_advice(mut self, advice: impl Into<String>) -> Self {
        self.advice = Some(advice.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    /// Human-readable report.
    pub fn render(&self) -> String {
        let mut out = String::from("amf doctor\n\n");
        for finding in &self.findings {
            out.push_str(&format!(
                "[{}] {}\n",
                finding.severity.marker(),
                finding.summary
            ));
            for line in &finding.detail {
                out.push_str(&format!("       {line}\n"));
            }
            if let Some(advice) = &finding.advice {
                out.push_str(&format!("       → {advice}\n"));
            }
        }
        let warnings = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Warn)
            .count();
        out.push_str(&format!(
            "\n{} check{} run, {warnings} worth a look. Nothing was changed.\n",
            self.findings.len(),
            if self.findings.len() == 1 { "" } else { "s" }
        ));
        out
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Everything the checks read, gathered once so the report is a pure function
/// of it — which is what makes the checks testable without a machine.
pub struct Inputs<'a> {
    pub config: &'a AppConfig,
    pub store: &'a ProjectStore,
    /// Which tmux windows actually have a process running, for the harness
    /// census.
    pub live: &'a LiveHarnesses,
    /// Every `amf-*` tmux session on the server.
    pub tmux_sessions: &'a [String],
    pub memory: Option<MemorySnapshot>,
    /// True when running under WSL.
    pub is_wsl: bool,
    /// Worktree directories found on disk, per project repo.
    pub worktrees: &'a [PathBuf],
    /// Editor records AMF is still tracking.
    pub editors: &'a [LaunchedEditor],
    /// Liveness probe, injectable so tests need no real processes.
    pub pid_alive: &'a dyn Fn(i64) -> bool,
    /// What the local Tailscale reported, for the AMF Remote check.
    pub tailscale: &'a TailscaleStatus,
    /// Phones paired with AMF Remote and not revoked.
    pub paired_devices: usize,
}

/// Run every check.
pub fn diagnose(inputs: &Inputs<'_>) -> Report {
    let mut findings = vec![
        check_agents(inputs),
        check_open_editors(inputs),
        check_memory(inputs),
    ];
    findings.extend(check_swap(inputs));
    findings.push(check_orphan_sessions(inputs));
    findings.push(check_orphan_worktrees(inputs));
    findings.push(check_stale_editors(inputs));
    findings.push(check_legacy_project_config(inputs));
    findings.push(check_remote_access(inputs));
    Report { findings }
}

fn check_agents(inputs: &Inputs<'_>) -> Finding {
    let active: Vec<ActiveHarness> = active_harness_sessions(inputs.store, inputs.live);
    let detail: Vec<String> = active
        .iter()
        .map(|harness| {
            format!(
                "{}/{} — {}",
                harness.project_name, harness.feature_name, harness.session_label
            )
        })
        .collect();

    match inputs.config.agent_concurrency_limit() {
        None => Finding::new(
            "agents",
            Severity::Notice,
            format!(
                "{} agent session(s) running; no limit configured",
                active.len()
            ),
        )
        .with_detail(detail)
        .with_advice("set max_concurrent_agents to be warned before the machine fills up"),
        Some(limit) if active.len() >= limit => Finding::new(
            "agents",
            Severity::Warn,
            format!("{} agent session(s) running, limit {limit}", active.len()),
        )
        .with_detail(detail)
        .with_advice(
            "starting another will ask for confirmation; z on the dashboard lists idle ones",
        ),
        Some(limit) => Finding::new(
            "agents",
            Severity::Ok,
            format!("{} of {limit} agent session(s) running", active.len()),
        )
        .with_detail(detail),
    }
}

/// Editor windows open for features that are still running.
///
/// Reported next to the agent count because the count alone is misleading:
/// "2 of 4 agents" reads as headroom, while two editor windows and their
/// language servers can outweigh every harness on the machine put together.
/// Editors left behind by *stopped* features are a different problem and get
/// their own finding.
fn check_open_editors(inputs: &Inputs<'_>) -> Finding {
    let running: HashSet<&str> = inputs
        .store
        .projects
        .iter()
        .flat_map(|project| project.features.iter())
        .filter(|feature| feature.status != ProjectStatus::Stopped)
        .map(|feature| feature.id.as_str())
        .collect();

    let feature_name = |id: &str| -> String {
        inputs
            .store
            .projects
            .iter()
            .flat_map(|project| project.features.iter())
            .find(|feature| feature.id == id)
            .map(|feature| feature.name.clone())
            .unwrap_or_else(|| id.to_string())
    };

    let open: Vec<String> = inputs
        .editors
        .iter()
        .filter(|editor| running.contains(editor.feature_id.as_str()))
        .filter(|editor| (inputs.pid_alive)(editor.pid))
        .map(|editor| {
            format!(
                "{} on {}",
                editor.kind.display_name(),
                feature_name(&editor.feature_id)
            )
        })
        .collect();

    if open.is_empty() {
        return Finding::new(
            "editors-open",
            Severity::Ok,
            "no editor windows open for running features",
        );
    }
    Finding::new(
        "editors-open",
        Severity::Notice,
        format!("{} editor window(s) open alongside the agents", open.len()),
    )
    .with_detail(open)
    .with_advice(
        "language servers under these usually outweigh every agent on the machine, \
         and the agent count above does not include them",
    )
}

fn check_memory(inputs: &Inputs<'_>) -> Finding {
    let Some(memory) = inputs.memory else {
        return Finding::new(
            "memory",
            Severity::Notice,
            "no memory signal on this platform",
        )
        .with_advice("the agent limit is the only guard here");
    };

    let summary = format!(
        "{} MiB available of {} MiB ({})",
        memory.available_mb,
        memory.total_mb,
        memory.source.label()
    );
    match inputs.config.low_memory_threshold_mb() {
        Some(threshold) if memory.is_low(threshold) => {
            Finding::new("memory", Severity::Warn, summary)
                .with_detail(vec![format!("below the {threshold} MiB warn threshold")])
                .with_advice("stop a feature you are not using — z lists the idle ones")
        }
        _ => Finding::new("memory", Severity::Ok, summary),
    }
}

/// Swap gets its own finding, and on WSL it comes with the `.wslconfig` note.
fn check_swap(inputs: &Inputs<'_>) -> Option<Finding> {
    let memory = inputs.memory?;
    let (free, total) = (memory.swap_free_mb?, memory.swap_total_mb?);

    if total == 0 {
        let finding = Finding::new("swap", Severity::Notice, "no swap configured");
        return Some(if inputs.is_wsl {
            finding
                .with_detail(vec![
                    "under WSL, swap is set in %UserProfile%\\.wslconfig (swap=...)".to_string(),
                ])
                // Deliberately not a blanket recommendation: swap turns a hard
                // OOM kill into a long disk-thrashing freeze if concurrency
                // stays high, which is not obviously the better failure.
                .with_advice(
                    "adding swap trades an out-of-memory kill for heavy paging; \
                     lowering max_concurrent_agents addresses the cause instead",
                )
        } else {
            finding.with_advice(
                "without swap the kernel has no cushion; an over-committed machine is killed outright",
            )
        });
    }

    let used_fraction = 1.0 - (free as f64 / total as f64);
    Some(if used_fraction > 0.5 {
        Finding::new(
            "swap",
            Severity::Warn,
            format!(
                "swap {}% used ({free} MiB free of {total} MiB)",
                (used_fraction * 100.0).round()
            ),
        )
        .with_advice("heavy swapping is what makes the machine feel frozen; stop something")
    } else {
        Finding::new(
            "swap",
            Severity::Ok,
            format!("swap {free} MiB free of {total} MiB"),
        )
    })
}

fn check_orphan_sessions(inputs: &Inputs<'_>) -> Finding {
    let known: HashSet<&str> = inputs
        .store
        .projects
        .iter()
        .flat_map(|project| project.features.iter())
        .map(|feature| feature.tmux_session.as_str())
        .collect();

    let orphans: Vec<String> = inputs
        .tmux_sessions
        .iter()
        .filter(|session| !known.contains(session.as_str()))
        .cloned()
        .collect();

    if orphans.is_empty() {
        return Finding::new(
            "tmux-sessions",
            Severity::Ok,
            "no orphaned amf-* tmux sessions",
        );
    }
    Finding::new(
        "tmux-sessions",
        Severity::Warn,
        format!(
            "{} amf-* tmux session(s) with no matching feature",
            orphans.len()
        ),
    )
    .with_detail(orphans)
    .with_advice("these still hold agent processes: tmux kill-session -t <name>")
}

fn check_orphan_worktrees(inputs: &Inputs<'_>) -> Finding {
    let known: HashSet<&Path> = inputs
        .store
        .projects
        .iter()
        .flat_map(|project| project.features.iter())
        .map(|feature| feature.workdir.as_path())
        .collect();

    let orphans: Vec<String> = inputs
        .worktrees
        .iter()
        .filter(|path| !known.contains(path.as_path()))
        .map(|path| path.display().to_string())
        .collect();

    if orphans.is_empty() {
        return Finding::new("worktrees", Severity::Ok, "no orphaned worktrees on disk");
    }
    Finding::new(
        "worktrees",
        Severity::Notice,
        format!("{} worktree(s) with no matching feature", orphans.len()),
    )
    .with_detail(orphans)
    .with_advice("these only cost disk: git worktree remove <path> when you are done with them")
}

fn check_stale_editors(inputs: &Inputs<'_>) -> Finding {
    let stopped: HashSet<&str> = inputs
        .store
        .projects
        .iter()
        .flat_map(|project| project.features.iter())
        .filter(|feature| feature.status == ProjectStatus::Stopped)
        .map(|feature| feature.id.as_str())
        .collect();

    let stale: Vec<String> = inputs
        .editors
        .iter()
        .filter(|editor| stopped.contains(editor.feature_id.as_str()))
        .filter(|editor| (inputs.pid_alive)(editor.pid))
        .map(|editor| {
            format!(
                "{} (pid {}) on {}{}",
                editor.kind.display_name(),
                editor.pid,
                editor.worktree_path.display(),
                if editor.dedicated {
                    ""
                } else {
                    " — not opened by AMF"
                }
            )
        })
        .collect();

    if stale.is_empty() {
        return Finding::new(
            "editors",
            Severity::Ok,
            "no editors left running for stopped features",
        );
    }
    Finding::new(
        "editors",
        Severity::Warn,
        format!(
            "{} editor(s) still running for stopped features",
            stale.len()
        ),
    )
    .with_detail(stale)
    .with_advice(
        "language servers under these are usually the biggest consumers on the machine; \
         close the window, or set kill_editor_on_stop",
    )
}

/// Whether this is WSL. The kernel release carries the marker on both WSL1 and
/// WSL2, and `/proc/sys/kernel/osrelease` is readable without spawning anything.
/// Report project directories still keeping config at the pre-`amf.json`
/// path, so the migration is visible before someone trips over it.
///
/// Two distinct states, because the advice differs. A repo with only
/// `.amf/config.json` is still fully working — AMF reads it, and the next
/// config write migrates it. A repo with **both** is the one worth
/// knowing about: the legacy file is no longer read, so edits to it do
/// nothing while it sits there looking like config.
fn check_legacy_project_config(inputs: &Inputs<'_>) -> Finding {
    // Project repos only, not feature workdirs. A worktree's
    // `.amf/config.json` is the *tracked* file as its branch has it, so it
    // cannot be migrated on its own — it arrives when the branch merges.
    // Listing every worktree would bury the one actionable line under a
    // page of copies of it.
    let mut seen: HashSet<&Path> = HashSet::new();
    let dirs: Vec<&Path> = inputs
        .store
        .projects
        .iter()
        .map(|project| project.repo.as_path())
        .filter(|repo| seen.insert(repo))
        .collect();

    let mut unmigrated: Vec<String> = Vec::new();
    let mut shadowed: Vec<String> = Vec::new();
    for dir in dirs {
        let legacy = legacy_project_config_path(dir);
        if !legacy.exists() {
            continue;
        }
        if project_config_path(dir).exists() {
            shadowed.push(format!(
                "{} (shadowed by amf.json — no longer read)",
                legacy.display()
            ));
        } else {
            unmigrated.push(legacy.display().to_string());
        }
    }

    let total = unmigrated.len() + shadowed.len();
    if total == 0 {
        return Finding::new(
            "config-path",
            Severity::Ok,
            "no project config left at the legacy .amf/config.json path",
        );
    }

    let advice = if unmigrated.is_empty() {
        "the legacy file is dead weight: delete it, or keep it only as a backup"
    } else {
        "still read, and migrated on the next config write — or move it now: git mv .amf/config.json amf.json"
    };

    let mut detail = unmigrated;
    detail.extend(shadowed);
    Finding::new(
        "config-path",
        Severity::Notice,
        format!("{total} project config file(s) at the legacy .amf/config.json path"),
    )
    .with_detail(detail)
    .with_advice(advice)
}

/// Can a phone reach AMF Remote over HTTPS? Mirrors the pairing dialog's
/// setup steps (`app/remote_tailscale.rs`), one finding with the next step
/// as advice. AMF Remote is optional, so an unfinished setup is only a
/// notice, and someone who hasn't paired a phone and has no Tailscale is
/// told nothing is set up. The one warning is a setup that looks finished
/// but can't work: AMF is served, yet no other device can reach it.
fn check_remote_access(inputs: &Inputs<'_>) -> Finding {
    const ID: &str = "remote-access";
    let config = inputs.config;
    let port = tailscale::bind_port(&config.remote_bind);
    let cli = TailscaleCli::new(
        config.remote_tailscale_cli.as_deref(),
        config.remote_tailscale_socket.as_deref(),
    );
    let serve_cmd = cli.display_command(&format!("serve --bg {port}"));
    let serve_url = inputs.tailscale.serve_url();
    let untagged =
        matches!(inputs.tailscale, TailscaleStatus::Running(node) if !node.tagged_for_amf);
    let lock_down = format!(
        "limit it to your own devices: tag this computer {} and add the policy in docs/remote-control.md",
        tailscale::ACCESS_TAG
    );

    if let Some(url) = config
        .remote_public_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        if !url.starts_with("https://") {
            return Finding::new(
                ID,
                Severity::Notice,
                format!("AMF Remote pairs phones at {url}, which isn't HTTPS"),
            )
            .with_advice(
                "phones can pair but can't install AMF Remote or get notifications without HTTPS",
            );
        }
        if let Some(serve_url) =
            serve_url.filter(|serve| serve.trim_end_matches('/') != url.trim_end_matches('/'))
        {
            return Finding::new(
                ID,
                Severity::Notice,
                format!(
                    "remote_public_url ({url}) differs from what Tailscale serves ({serve_url})"
                ),
            )
            .with_advice(
                "remove remote_public_url to use Tailscale's address automatically, or correct it",
            );
        }
        if let TailscaleStatus::Running(node) = inputs.tailscale
            && node.isolated()
            && serve_url.is_some()
        {
            return isolated_finding(url);
        }
        let finding = Finding::new(
            ID,
            Severity::Ok,
            format!("AMF Remote pairs phones at {url}"),
        );
        return if untagged {
            finding.with_advice(lock_down)
        } else {
            finding
        };
    }

    match inputs.tailscale {
        TailscaleStatus::Running(node) if node.serve_url.is_some() && node.isolated() => {
            isolated_finding(node.serve_url.as_deref().unwrap_or_default())
        }
        TailscaleStatus::Running(node) => match (&node.serve_url, node.https_enabled) {
            (Some(url), _) => {
                let finding =
                    Finding::new(ID, Severity::Ok, format!("AMF Remote reachable over Tailscale at {url}"));
                if untagged { finding.with_advice(lock_down) } else { finding }
            }
            (None, false) => Finding::new(
                ID,
                Severity::Notice,
                "Tailscale is running, but HTTPS certificates are off for your tailnet",
            )
            .with_advice(format!(
                "enable MagicDNS and HTTPS Certificates at {}, then: {serve_cmd}",
                tailscale::ADMIN_DNS_URL
            )),
            (None, true) => Finding::new(
                ID,
                Severity::Notice,
                format!("Tailscale is running, but isn't serving AMF Remote's port {port}"),
            )
            .with_advice(format!(
                "{serve_cmd} — or press t in the pairing dialog (Ctrl+Space Q)"
            )),
        },
        TailscaleStatus::NeedsLogin => Finding::new(
            ID,
            Severity::Notice,
            "Tailscale is installed but not signed in",
        )
        .with_advice(cli.display_command("up")),
        TailscaleStatus::Stopped => Finding::new(ID, Severity::Notice, "Tailscale is switched off")
            .with_advice(cli.display_command("up")),
        TailscaleStatus::Unavailable(why) => Finding::new(
            ID,
            Severity::Notice,
            "Tailscale is installed but didn't answer",
        )
        .with_detail(vec![why.clone()])
        .with_advice(
            "start Tailscale (its app, or the tailscaled service); if tailscaled uses its \
             own --socket, set remote_tailscale_socket to it",
        ),
        TailscaleStatus::NotInstalled if inputs.paired_devices == 0 => Finding::new(
            ID,
            Severity::Ok,
            "AMF Remote (phone access) isn't set up — optional",
        ),
        TailscaleStatus::NotInstalled => Finding::new(
            ID,
            Severity::Notice,
            format!(
                "{} phone(s) paired, but no HTTPS address: the pairing QR points at this machine only",
                inputs.paired_devices
            ),
        )
        .with_advice(format!(
            "install Tailscale ({}) or set remote_public_url; see docs/remote-control.md",
            tailscale::DOWNLOAD_URL
        )),
    }
}

/// Served, but Tailscale shows no other device that may reach this computer:
/// no phone is signed in, or the access policy shuts them all out.
fn isolated_finding(url: &str) -> Finding {
    Finding::new(
        "remote-access",
        Severity::Warn,
        format!("AMF Remote is served at {url}, but no other device on your tailnet can reach this computer"),
    )
    .with_advice(format!(
        "sign in on your phone with the same Tailscale account (log out and back in if it already says connected); \
         if you limited access, the policy needs a grant from your devices to {}:443",
        tailscale::ACCESS_TAG
    ))
}

pub fn detect_wsl() -> bool {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|release| {
            let release = release.to_ascii_lowercase();
            release.contains("microsoft") || release.contains("wsl")
        })
        .unwrap_or(false)
}

/// Worktree directories on disk for every project in the store.
pub fn worktrees_on_disk(store: &ProjectStore) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for project in &store.projects {
        if !seen.insert(project.repo.clone()) {
            continue;
        }
        let Ok(worktrees) = crate::worktree::WorktreeManager::list(&project.repo) else {
            continue;
        };
        for worktree in worktrees {
            // The primary checkout is the repo itself, not a reclaimable
            // worktree.
            if worktree.path != project.repo {
                found.push(worktree.path);
            }
        }
    }
    found
}

/// Default liveness probe for the CLI.
pub fn pid_alive(pid: i64) -> bool {
    procs::pid_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::editors::EditorKind;
    use crate::project::{Feature, Project, SessionKind, VibeMode};
    use crate::resources::mem::MemorySource;
    use chrono::Utc;

    fn feature(name: &str, status: ProjectStatus) -> Feature {
        Feature {
            id: format!("feat-{name}"),
            name: name.to_string(),
            branch: name.to_string(),
            workdir: PathBuf::from("/tmp").join(name),
            is_worktree: true,
            tmux_session: format!("amf-{name}"),
            sessions: Vec::new(),
            collapsed: true,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: Default::default(),
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: true,
            status,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            summary: None,
            summary_updated_at: None,
            nickname: None,
            selected_plan_path: None,
            triage_source: None,
            review_source: None,
            issue_source: None,
        }
    }

    fn store(features: Vec<Feature>) -> ProjectStore {
        ProjectStore {
            version: 1,
            projects: vec![Project {
                id: "p".into(),
                name: "proj".into(),
                repo: PathBuf::from("/tmp/proj"),
                collapsed: false,
                features,
                created_at: Utc::now(),
                preferred_agent: Default::default(),
                is_git: true,
            }],
            session_bookmarks: Vec::new(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: Default::default(),
        }
    }

    fn snapshot(available_mb: u64, swap_free: u64, swap_total: u64) -> MemorySnapshot {
        MemorySnapshot {
            available_mb,
            total_mb: 16384,
            swap_free_mb: Some(swap_free),
            swap_total_mb: Some(swap_total),
            source: MemorySource::ProcMeminfo,
        }
    }

    struct Fixture {
        config: AppConfig,
        store: ProjectStore,
        live: LiveHarnesses,
        sessions: Vec<String>,
        worktrees: Vec<PathBuf>,
        editors: Vec<LaunchedEditor>,
        memory: Option<MemorySnapshot>,
        is_wsl: bool,
        tailscale: TailscaleStatus,
        paired_devices: usize,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                config: AppConfig::default(),
                store: store(vec![feature("alpha", ProjectStatus::Active)]),
                live: LiveHarnesses::default(),
                sessions: vec!["amf-alpha".to_string()],
                worktrees: vec![PathBuf::from("/tmp/alpha")],
                editors: Vec::new(),
                memory: Some(snapshot(8000, 2048, 2048)),
                is_wsl: false,
                tailscale: TailscaleStatus::NotInstalled,
                paired_devices: 0,
            }
        }

        fn run(&self, alive: &dyn Fn(i64) -> bool) -> Report {
            diagnose(&Inputs {
                config: &self.config,
                store: &self.store,
                live: &self.live,
                tmux_sessions: &self.sessions,
                memory: self.memory,
                is_wsl: self.is_wsl,
                worktrees: &self.worktrees,
                editors: &self.editors,
                pid_alive: alive,
                tailscale: &self.tailscale,
                paired_devices: self.paired_devices,
            })
        }
    }

    fn finding<'a>(report: &'a Report, id: &str) -> &'a Finding {
        report
            .findings
            .iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| panic!("no {id} finding"))
    }

    fn editor(feature_id: &str, pid: i64, dedicated: bool) -> LaunchedEditor {
        LaunchedEditor {
            id: format!("e{pid}"),
            feature_id: feature_id.to_string(),
            session_id: None,
            kind: EditorKind::Vscode,
            pid,
            worktree_path: PathBuf::from("/tmp/alpha"),
            dedicated,
            command: "code --new-window /tmp/alpha".into(),
            proc_started_at: String::new(),
            started_at: Utc::now(),
        }
    }

    /// Build a fixture whose project repo and single feature workdir are
    /// real directories, so the config-path check has something to stat.
    fn fixture_rooted_at(repo: &Path, workdir: &Path) -> Fixture {
        let mut fixture = Fixture::new();
        let mut feat = feature("alpha", ProjectStatus::Active);
        feat.workdir = workdir.to_path_buf();
        fixture.store = store(vec![feat]);
        fixture.store.projects[0].repo = repo.to_path_buf();
        fixture
    }

    fn write_legacy_config(dir: &Path) {
        let legacy = legacy_project_config_path(dir);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(legacy, "{}").unwrap();
    }

    #[test]
    fn a_migrated_repo_reports_nothing_to_do() {
        let repo = tempfile::TempDir::new().unwrap();
        std::fs::write(project_config_path(repo.path()), "{}").unwrap();
        let fixture = fixture_rooted_at(repo.path(), repo.path());

        let report = fixture.run(&|_| false);
        assert_eq!(finding(&report, "config-path").severity, Severity::Ok);
    }

    #[test]
    fn flags_a_repo_still_on_the_legacy_config_path() {
        let repo = tempfile::TempDir::new().unwrap();
        write_legacy_config(repo.path());
        let fixture = fixture_rooted_at(repo.path(), repo.path());

        let report = fixture.run(&|_| false);
        let found = finding(&report, "config-path");
        assert_eq!(found.severity, Severity::Notice);
        assert_eq!(found.detail.len(), 1, "repo dir reported once, not twice");
        assert!(found.detail[0].ends_with(".amf/config.json"));
        // Nothing is broken yet, so the advice says so rather than alarming.
        assert!(found.advice.as_ref().unwrap().contains("git mv"));
    }

    #[test]
    fn a_legacy_file_shadowed_by_amf_json_gets_different_advice() {
        let repo = tempfile::TempDir::new().unwrap();
        write_legacy_config(repo.path());
        std::fs::write(project_config_path(repo.path()), "{}").unwrap();
        let fixture = fixture_rooted_at(repo.path(), repo.path());

        let report = fixture.run(&|_| false);
        let found = finding(&report, "config-path");
        assert_eq!(found.severity, Severity::Notice);
        assert!(found.detail[0].contains("no longer read"));
        assert!(found.advice.as_ref().unwrap().contains("dead weight"));
    }

    #[test]
    fn a_worktrees_own_copy_is_not_reported_as_needing_migration() {
        // The worktree's file is whatever its branch has checked out; it
        // migrates when that branch does, so naming it here would be
        // advice the user cannot act on — once per worktree.
        let repo = tempfile::TempDir::new().unwrap();
        let worktree = tempfile::TempDir::new().unwrap();
        write_legacy_config(worktree.path());
        std::fs::write(project_config_path(repo.path()), "{}").unwrap();
        let fixture = fixture_rooted_at(repo.path(), worktree.path());

        let report = fixture.run(&|_| false);
        assert_eq!(finding(&report, "config-path").severity, Severity::Ok);
    }

    #[test]
    fn a_quiet_machine_reports_all_clear() {
        let report = Fixture::new().run(&|_| false);
        assert!(
            report.findings.iter().all(|f| f.severity == Severity::Ok),
            "unexpected findings: {:?}",
            report
                .findings
                .iter()
                .filter(|f| f.severity != Severity::Ok)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn warns_when_the_agent_limit_is_reached() {
        let mut fixture = Fixture::new();
        fixture.store.projects[0].features[0]
            .add_session_named(SessionKind::Claude, "Claude 1".into());
        fixture.live = LiveHarnesses::from_pairs([("amf-alpha", "claude")]);
        fixture.config.max_concurrent_agents = 1;

        let report = fixture.run(&|_| false);
        let agents = finding(&report, "agents");
        assert_eq!(agents.severity, Severity::Warn);
        assert!(agents.summary.contains("limit 1"));
        assert_eq!(agents.detail, vec!["proj/alpha — Claude 1"]);
    }

    #[test]
    fn reports_a_missing_memory_signal_without_guessing() {
        let mut fixture = Fixture::new();
        fixture.memory = None;

        let report = fixture.run(&|_| false);
        let memory = finding(&report, "memory");
        assert_eq!(memory.severity, Severity::Notice);
        assert!(memory.advice.as_deref().unwrap().contains("agent limit"));
        // No memory means no swap finding either, rather than a fabricated one.
        assert!(report.findings.iter().all(|f| f.id != "swap"));
    }

    #[test]
    fn warns_on_low_memory() {
        let mut fixture = Fixture::new();
        fixture.memory = Some(snapshot(400, 2048, 2048));

        let report = fixture.run(&|_| false);
        assert_eq!(finding(&report, "memory").severity, Severity::Warn);
    }

    #[test]
    fn wsl_swap_advice_does_not_recommend_swap_unconditionally() {
        let mut fixture = Fixture::new();
        fixture.is_wsl = true;
        fixture.memory = Some(snapshot(8000, 0, 0));

        let report = fixture.run(&|_| false);
        let swap = finding(&report, "swap");
        assert_eq!(swap.severity, Severity::Notice);
        assert!(swap.detail[0].contains(".wslconfig"));
        let advice = swap.advice.as_deref().unwrap();
        assert!(advice.contains("heavy paging"), "got {advice}");
        assert!(advice.contains("max_concurrent_agents"), "got {advice}");
    }

    #[test]
    fn flags_tmux_sessions_with_no_feature() {
        let mut fixture = Fixture::new();
        fixture.sessions.push("amf-ghost".to_string());

        let report = fixture.run(&|_| false);
        let sessions = finding(&report, "tmux-sessions");
        assert_eq!(sessions.severity, Severity::Warn);
        assert_eq!(sessions.detail, vec!["amf-ghost"]);
    }

    #[test]
    fn flags_worktrees_with_no_feature() {
        let mut fixture = Fixture::new();
        fixture.worktrees.push(PathBuf::from("/tmp/left-behind"));

        let report = fixture.run(&|_| false);
        let worktrees = finding(&report, "worktrees");
        assert_eq!(worktrees.severity, Severity::Notice);
        assert_eq!(worktrees.detail, vec!["/tmp/left-behind"]);
    }

    #[test]
    fn flags_editors_still_running_for_stopped_features() {
        let mut fixture = Fixture::new();
        fixture.store = store(vec![feature("alpha", ProjectStatus::Stopped)]);
        fixture.editors = vec![editor("feat-alpha", 4242, true)];

        let report = fixture.run(&|pid| pid == 4242);
        let editors = finding(&report, "editors");
        assert_eq!(editors.severity, Severity::Warn);
        assert!(editors.detail[0].contains("pid 4242"));

        // A dead pid is not a finding.
        let report = fixture.run(&|_| false);
        assert_eq!(finding(&report, "editors").severity, Severity::Ok);
    }

    #[test]
    fn a_running_feature_keeps_its_editor_out_of_the_stale_report() {
        let mut fixture = Fixture::new();
        fixture.editors = vec![editor("feat-alpha", 4242, true)];

        let report = fixture.run(&|_| true);
        // Not stale -- its feature is still running ...
        assert_eq!(finding(&report, "editors").severity, Severity::Ok);
        // ... but it is still weight on the machine, reported next to the
        // agent count that does not include it.
        let open = finding(&report, "editors-open");
        assert_eq!(open.severity, Severity::Notice);
        assert_eq!(open.detail, vec!["VS Code on alpha"]);
        assert!(open.advice.as_deref().unwrap().contains("outweigh"));
    }

    #[test]
    fn a_closed_editor_is_not_reported_as_open() {
        let mut fixture = Fixture::new();
        fixture.editors = vec![editor("feat-alpha", 4242, true)];

        let report = fixture.run(&|_| false);
        assert_eq!(finding(&report, "editors-open").severity, Severity::Ok);
    }

    #[test]
    fn a_stopped_features_editor_is_reported_once_as_stale_not_as_open() {
        let mut fixture = Fixture::new();
        fixture.store = store(vec![feature("alpha", ProjectStatus::Stopped)]);
        fixture.editors = vec![editor("feat-alpha", 4242, true)];

        let report = fixture.run(&|_| true);
        assert_eq!(finding(&report, "editors").severity, Severity::Warn);
        assert_eq!(finding(&report, "editors-open").severity, Severity::Ok);
    }

    fn tailnet(https: bool, serve: Option<&str>, tagged: bool) -> TailscaleStatus {
        TailscaleStatus::Running(crate::tailscale::TailnetNode {
            dns_name: "pc.tail1.ts.net".into(),
            https_enabled: https,
            tagged_for_amf: tagged,
            serve_url: serve.map(str::to_string),
            peers: 1,
        })
    }

    #[test]
    fn remote_access_is_quiet_when_nobody_uses_it() {
        let report = Fixture::new().run(&|_| false);
        assert_eq!(finding(&report, "remote-access").severity, Severity::Ok);
    }

    #[test]
    fn remote_access_names_the_next_tailscale_step() {
        let mut fixture = Fixture::new();
        fixture.tailscale = tailnet(false, None, false);
        let off = finding(&fixture.run(&|_| false), "remote-access").clone();
        assert_eq!(off.severity, Severity::Notice);
        assert!(off.advice.unwrap().contains("HTTPS Certificates"));

        fixture.tailscale = tailnet(true, None, false);
        let unserved = finding(&fixture.run(&|_| false), "remote-access").clone();
        assert!(
            unserved
                .advice
                .unwrap()
                .contains("tailscale serve --bg 47800")
        );

        fixture.tailscale = tailnet(true, Some("https://pc.tail1.ts.net"), false);
        let served = finding(&fixture.run(&|_| false), "remote-access").clone();
        assert_eq!(served.severity, Severity::Ok);
        assert!(served.summary.contains("https://pc.tail1.ts.net"));
        assert!(served.advice.unwrap().contains("tag:amf"));

        fixture.tailscale = tailnet(true, Some("https://pc.tail1.ts.net"), true);
        let locked = finding(&fixture.run(&|_| false), "remote-access").clone();
        assert_eq!(locked.advice, None);
    }

    #[test]
    fn remote_access_warns_when_no_device_can_reach_this_computer() {
        let mut fixture = Fixture::new();
        let mut isolated = tailnet(true, Some("https://pc.tail1.ts.net"), true);
        if let TailscaleStatus::Running(node) = &mut isolated {
            node.peers = 0;
        }
        fixture.tailscale = isolated;
        let report = fixture.run(&|_| false);
        let remote = finding(&report, "remote-access");
        assert_eq!(remote.severity, Severity::Warn);
        assert!(remote.summary.contains("no other device"));

        fixture.config.remote_public_url = Some("https://pc.tail1.ts.net".into());
        let report = fixture.run(&|_| false);
        assert_eq!(
            finding(&report, "remote-access").severity,
            Severity::Warn,
            "a configured URL pointing at the same tailnet address is just as unreachable"
        );
    }

    #[test]
    fn remote_access_flags_a_public_url_tailscale_disagrees_with() {
        let mut fixture = Fixture::new();
        fixture.config.remote_public_url = Some("https://old.tail1.ts.net".into());
        fixture.tailscale = tailnet(true, Some("https://pc.tail1.ts.net"), false);
        let report = fixture.run(&|_| false);
        let remote = finding(&report, "remote-access");
        assert_eq!(remote.severity, Severity::Notice);
        assert!(remote.summary.contains("differs"));

        fixture.config.remote_public_url = Some("https://pc.tail1.ts.net".into());
        let report = fixture.run(&|_| false);
        let agreed = finding(&report, "remote-access");
        assert_eq!(agreed.severity, Severity::Ok);
        assert!(
            agreed.advice.as_deref().unwrap().contains("tag:amf"),
            "a configured URL still gets the lock-down reminder"
        );

        fixture.config.remote_public_url = Some("http://192.168.0.2:47800".into());
        let report = fixture.run(&|_| false);
        assert!(
            finding(&report, "remote-access")
                .summary
                .contains("isn't HTTPS")
        );
    }

    #[test]
    fn remote_access_speaks_up_for_a_paired_phone_with_no_address() {
        let mut fixture = Fixture::new();
        fixture.paired_devices = 1;
        let report = fixture.run(&|_| false);
        let remote = finding(&report, "remote-access");
        assert_eq!(remote.severity, Severity::Notice);
        assert!(
            remote
                .advice
                .as_deref()
                .unwrap()
                .contains("tailscale.com/download")
        );
    }

    #[test]
    fn renders_text_and_json() {
        let mut fixture = Fixture::new();
        fixture.sessions.push("amf-ghost".to_string());
        let report = fixture.run(&|_| false);

        let text = report.render();
        assert!(text.contains("amf doctor"));
        assert!(text.contains("amf-ghost"));
        assert!(text.contains("Nothing was changed."));

        let json: serde_json::Value = serde_json::from_str(&report.to_json()).unwrap();
        let findings = json["findings"].as_array().unwrap();
        assert_eq!(findings.len(), report.findings.len());
        let sessions = findings
            .iter()
            .find(|f| f["id"] == "tmux-sessions")
            .unwrap();
        assert_eq!(sessions["severity"], "warn");
        assert_eq!(sessions["detail"][0], "amf-ghost");
    }
}
