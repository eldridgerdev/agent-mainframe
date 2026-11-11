//! Keeping the sandbox registry in step with the project store.

use std::collections::HashMap;

use super::App;
use crate::container::args::SandboxSpec;

/// Git metadata the sandbox must not be able to rewrite, because host git (AMF's
/// own diff/status calls, the user's shell prompt) reads it and honours
/// `core.fsmonitor`, hooks and `gitdir:` redirects by running commands.
///
/// Mounted read-only on top of the writable checkout: the repository's config
/// and hooks, every linked worktree's `.git` file (a rewritten `gitdir:` line
/// would point host git at a config the sandbox chose) and each
/// `.git/worktrees/<n>/{gitdir,commondir,config.worktree}`. Only paths that
/// exist can be mounted, so a `.git/commondir` that does not exist yet cannot
/// be pre-empted; `docs/container-projects.md` states that limit.
pub(crate) fn git_protected_paths(
    repo: &std::path::Path,
    workdirs: &[&std::path::PathBuf],
) -> Vec<std::path::PathBuf> {
    let git_dir = repo.join(".git");
    let mut out = Vec::new();
    if git_dir.is_dir() {
        for n in ["config", "hooks", "commondir", "config.worktree"] {
            let p = git_dir.join(n);
            if p.exists() {
                out.push(p);
            }
        }
        if let Ok(entries) = std::fs::read_dir(git_dir.join("worktrees")) {
            let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            dirs.sort();
            for d in dirs {
                for n in ["gitdir", "commondir", "config.worktree"] {
                    let p = d.join(n);
                    if p.is_file() {
                        out.push(p);
                    }
                }
            }
        }
    }
    for wd in workdirs {
        // A linked worktree's `.git` is a file; the repo root's is the
        // directory handled above.
        let dot_git = wd.join(".git");
        if dot_git.is_file() && !out.contains(&dot_git) {
            out.push(dot_git);
        }
    }
    out
}

impl App {
    /// Rebuild the registry from the store: every feature of a containerized
    /// project gets a [`SandboxSpec`]. Cheap, and called wherever sessions can
    /// start, so a project is never launched on the host because the registry
    /// was a save behind.
    pub fn sync_sandbox_registry(&self) {
        let settings = &self.config.projects.container;
        let mut specs = HashMap::new();
        for project in self.store.projects.iter().filter(|p| p.containerized) {
            let short: String = project
                .id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(12)
                .collect();
            for feature in &project.features {
                let mut mounts = vec![project.repo.clone()];
                if !feature.workdir.starts_with(&project.repo) {
                    mounts.push(feature.workdir.clone());
                }
                let workdirs: Vec<_> = project.features.iter().map(|f| &f.workdir).collect();
                let protect = git_protected_paths(&project.repo, &workdirs);
                specs.insert(
                    feature.tmux_session.clone(),
                    SandboxSpec {
                        tmux_session: feature.tmux_session.clone(),
                        mounts,
                        protect,
                        workdir: feature.workdir.clone(),
                        home_volume: format!("amf-sbx-home-{short}"),
                        settings: settings.clone(),
                    },
                );
            }
        }
        self.sandbox.replace_all(specs);
        // The repo and every feature workdir: a worktree outside the repo is
        // mounted into the sandbox and is just as untrusted.
        crate::headless::set_untrusted_roots(
            self.store
                .projects
                .iter()
                .filter(|p| p.containerized)
                .flat_map(|p| {
                    std::iter::once(p.repo.clone())
                        .chain(p.features.iter().map(|f| f.workdir.clone()))
                })
                .collect(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    fn project(name: &str, repo: &str, containerized: bool, workdirs: &[&str]) -> Project {
        let mut p = Project::new(name.into(), repo.into(), true, AgentKind::Claude);
        p.containerized = containerized;
        for (i, wd) in workdirs.iter().enumerate() {
            p.features.push(Feature::new_for_project(
                name,
                format!("feat{i}"),
                format!("b{i}"),
                (*wd).into(),
                true,
                VibeMode::Vibeless,
                false,
                false,
                AgentKind::Claude,
                false,
                false,
            ));
        }
        p
    }

    #[test]
    fn only_containerized_projects_are_registered_with_their_mounts() {
        let store = ProjectStore {
            projects: vec![
                project(
                    "sbx",
                    "/code/sbx",
                    true,
                    &["/code/sbx/.worktrees/f0", "/elsewhere/f1"],
                ),
                project(
                    "plain",
                    "/code/plain",
                    false,
                    &["/code/plain/.worktrees/f0"],
                ),
            ],
            ..ProjectStore::empty()
        };
        let app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.sync_sandbox_registry();
        assert_eq!(app.sandbox.len(), 2);

        let inside = app.sandbox.get("amf-sbx-feat0").expect("registered");
        assert!(
            inside.protect.is_empty(),
            "no such .git here, nothing to protect"
        );
        assert_eq!(inside.mounts, vec![std::path::PathBuf::from("/code/sbx")]);

        // A worktree outside the repo is mounted too, or git would break.
        let outside = app.sandbox.get("amf-sbx-feat1").expect("registered");
        assert_eq!(outside.mounts.len(), 2);
        assert_eq!(outside.mounts[1], std::path::PathBuf::from("/elsewhere/f1"));

        assert!(app.sandbox.get("amf-plain-feat0").is_none());
        assert_eq!(
            inside.home_volume,
            app.sandbox.get("amf-sbx-feat1").unwrap().home_volume
        );
    }

    #[test]
    fn clearing_the_flag_unregisters_on_the_next_sync() {
        let mut store = ProjectStore {
            projects: vec![project(
                "sbx",
                "/code/sbx",
                true,
                &["/code/sbx/.worktrees/f0"],
            )],
            ..ProjectStore::empty()
        };
        let mut app = App::new_for_test(
            store.clone(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.sync_sandbox_registry();
        assert!(!app.sandbox.is_empty());
        store.projects[0].containerized = false;
        app.store = store;
        app.sync_sandbox_registry();
        assert!(app.sandbox.is_empty());
    }
}

#[cfg(test)]
mod git_protection_tests {
    use super::*;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    #[test]
    fn git_config_and_hooks_are_protected_when_they_exist() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join(".git/hooks")).unwrap();
        std::fs::write(repo.path().join(".git/config"), "[core]\n").unwrap();
        let mut p = Project::new("sbx".into(), repo.path().into(), true, AgentKind::Claude);
        p.containerized = true;
        p.features.push(Feature::new_for_project(
            "sbx",
            "f".into(),
            "b".into(),
            repo.path().join(".worktrees/f"),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        ));
        let app = App::new_for_test(
            ProjectStore {
                projects: vec![p],
                ..ProjectStore::empty()
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.sync_sandbox_registry();
        let spec = app.sandbox.get("amf-sbx-f").unwrap();
        assert_eq!(
            spec.protect,
            vec![
                repo.path().join(".git/config"),
                repo.path().join(".git/hooks")
            ]
        );
    }
}

#[cfg(test)]
mod worktree_protection_tests {
    use super::*;

    #[test]
    fn worktree_git_files_and_metadata_are_protected() {
        let repo = tempfile::tempdir().unwrap();
        let g = repo.path().join(".git");
        std::fs::create_dir_all(g.join("hooks")).unwrap();
        std::fs::create_dir_all(g.join("worktrees/f")).unwrap();
        std::fs::write(g.join("config"), "").unwrap();
        std::fs::write(g.join("worktrees/f/gitdir"), "x").unwrap();
        std::fs::write(g.join("worktrees/f/commondir"), "../..").unwrap();
        let wd = repo.path().join(".worktrees/f");
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join(".git"), "gitdir: x").unwrap();

        let got = git_protected_paths(repo.path(), &[&wd]);
        assert!(got.contains(&g.join("config")));
        assert!(got.contains(&g.join("hooks")));
        assert!(got.contains(&g.join("worktrees/f/gitdir")));
        assert!(got.contains(&g.join("worktrees/f/commondir")));
        assert!(got.contains(&wd.join(".git")));
    }
}
