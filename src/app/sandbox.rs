//! Keeping the sandbox registry in step with the project store.

use std::collections::HashMap;

use super::App;
use crate::container::args::SandboxSpec;

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
                let git_dir = project.repo.join(".git");
                // Only a directory `.git` has a config and hooks of its own to
                // protect (a linked worktree's `.git` is a file).
                let protect: Vec<_> = ["config", "hooks"]
                    .iter()
                    .map(|n| git_dir.join(n))
                    .filter(|p| p.exists())
                    .collect();
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
        crate::headless::set_untrusted_roots(
            self.store
                .projects
                .iter()
                .filter(|p| p.containerized)
                .map(|p| p.repo.clone())
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
