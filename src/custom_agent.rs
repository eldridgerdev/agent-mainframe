//! Run a configured harness command without losing the harness launch context.
//! A PATH shim supplies AMF's arguments even when a Python/shell script execs
//! the harness. It restores PATH before exec so child agents are independent.
use std::path::Path;

use anyhow::Result;
use sha2::{Digest, Sha256};

use crate::project::{FeatureSession, SessionKind, VibeMode};
use crate::tmux::TmuxManager;
use crate::traits::TmuxOps;

/// Upgrade only generic sessions whose saved command unambiguously matches
/// an explicitly declared harness. IDs, windows and live processes stay intact.
pub(crate) fn reconcile(
    store: &mut crate::project::ProjectStore,
    global: &crate::extension::ExtensionConfig,
) -> bool {
    let mut changed = false;
    for project in &mut store.projects {
        let config = crate::extension::merge_project_extension_config(global, &project.repo);
        for session in project.features.iter_mut().flat_map(|f| &mut f.sessions) {
            if session.kind != SessionKind::Custom || session.command.is_none() {
                continue;
            }
            let matching: Vec<_> = config
                .custom_sessions
                .iter()
                .filter(|c| c.command == session.command)
                .collect();
            let Some(agent) = matching.first().and_then(|c| c.agent.as_ref()) else {
                continue;
            };
            if matching.iter().all(|c| c.agent.as_ref() == Some(agent)) {
                session.kind = match agent {
                    crate::project::AgentKind::Claude => SessionKind::Claude,
                    crate::project::AgentKind::Opencode => SessionKind::Opencode,
                    crate::project::AgentKind::Codex => SessionKind::Codex,
                    crate::project::AgentKind::Pi => SessionKind::Pi,
                };
                session.status_text = None;
                changed = true;
            }
        }
    }
    changed
}

pub(crate) fn launch_args(
    session: &FeatureSession,
    workdir: &Path,
    mode: &VibeMode,
    claude_args: Vec<String>,
) -> Vec<String> {
    match session.kind {
        SessionKind::Codex => crate::codex_config::launch_override_args(workdir, mode),
        SessionKind::Claude => claude_args,
        _ => Vec::new(),
    }
}

pub(crate) fn launch(
    tmux: &dyn TmuxOps,
    tmux_session: &str,
    workdir: &Path,
    session: &FeatureSession,
    args: Vec<String>,
) -> Result<()> {
    let config = crate::extension::CustomSessionConfig {
        pre_check: session.pre_check.clone(),
        ..Default::default()
    };
    // Like generic custom sessions, a failed restart check leaves a shell.
    if config.run_pre_check(workdir).is_err() {
        return Ok(());
    }
    let command = shell_command(tmux_session, workdir, session, args)?;
    tmux.run_shell_command(tmux_session, &session.tmux_window, &command)
}

pub(crate) fn shell_command(
    tmux_session: &str,
    workdir: &Path,
    session: &FeatureSession,
    args: Vec<String>,
) -> Result<String> {
    use std::os::unix::fs::PermissionsExt;
    let executable = match session.kind {
        SessionKind::Claude => "claude",
        SessionKind::Codex => "codex",
        SessionKind::Opencode => "opencode",
        SessionKind::Pi => "pi",
        _ => anyhow::bail!("custom agent command requires an agent harness"),
    };
    let shim_dir = crate::extension::generated_amf_subdir(workdir, "agent-launch")
        .join(format!("{:x}", Sha256::digest(session.id.as_bytes())));
    std::fs::create_dir_all(&shim_dir)?;
    let shim = shim_dir.join(executable);
    let quoted_args = args
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "#!/bin/sh\namf_custom_binary=$AMF_CUSTOM_AGENT_BIN\nPATH=$AMF_CUSTOM_AGENT_PATH\nexport PATH\nunset AMF_CUSTOM_AGENT_BIN AMF_CUSTOM_AGENT_PATH\nexec \"$amf_custom_binary\" {quoted_args} \"$@\"\n"
    );
    let pending = shim_dir.join(format!(".pending-{}", uuid::Uuid::new_v4()));
    std::fs::write(&pending, script)?;
    std::fs::set_permissions(&pending, std::fs::Permissions::from_mode(0o700))?;
    std::fs::rename(pending, &shim)?;
    let command = session.command.as_deref().unwrap_or(executable);
    let body = format!(
        "AMF_CUSTOM_AGENT_BIN=$(command -v {executable}) || exit; \
         AMF_CUSTOM_AGENT_PATH=$PATH; export AMF_CUSTOM_AGENT_BIN AMF_CUSTOM_AGENT_PATH; \
         PATH={}:$PATH; export PATH; {command}",
        quote(&shim_dir.to_string_lossy())
    );
    let status_dir = crate::extension::generated_amf_subdir(workdir, "session-status");
    Ok(format!(
        "{} {} bash -c {}",
        TmuxManager::agent_launch_env(tmux_session, &session.tmux_window, &session.id),
        TmuxManager::shell_env_prefix(&[
            ("AMF_SESSION_ID", &session.id),
            ("AMF_STATUS_DIR", &status_dir.to_string_lossy()),
        ]),
        quote(&body),
    ))
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::{CustomSessionConfig, ExtensionConfig};
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, ProjectStore};
    use std::os::unix::fs::PermissionsExt;

    fn fixture(root: &Path) -> ProjectStore {
        let mut feature = Feature::new_for_project(
            "test",
            "main".into(),
            "main".into(),
            root.to_owned(),
            false,
            VibeMode::SuperVibe,
            false,
            false,
            AgentKind::Codex,
            false,
            false,
        );
        feature.status = ProjectStatus::Idle;
        feature.add_custom_session_named(
            "orchestrator".into(),
            "orchestrator".into(),
            Some("python3 launcher.py".into()),
            Some("cleanup".into()),
            Some("test -d .".into()),
        );
        let mut project = Project::new("test".into(), root.to_owned(), false, AgentKind::Codex);
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        store
    }

    #[test]
    fn explicit_agent_recognition_preserves_existing_session_identity_and_roundtrips() {
        let root = tempfile::tempdir().unwrap();
        let mut store = fixture(root.path());
        let original = store.projects[0].features[0].sessions[0].clone();
        let config = ExtensionConfig {
            custom_sessions: vec![CustomSessionConfig {
                name: "orchestrator".into(),
                agent: Some(AgentKind::Codex),
                command: original.command.clone(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(reconcile(&mut store, &config));
        assert!(!reconcile(&mut store, &config));
        let session = &store.projects[0].features[0].sessions[0];
        assert_eq!(session.kind, SessionKind::Codex);
        assert_eq!(session.id, original.id);
        assert_eq!(session.tmux_window, original.tmux_window);
        assert_eq!(session.command, original.command);
        assert_eq!(session.on_stop, original.on_stop);
        assert_eq!(session.pre_check, original.pre_check);
        let db = crate::db::AmfDb::open(&root.path().join("store.db")).unwrap();
        db.save_store(&store).unwrap();
        let loaded = db.load_store().unwrap();
        let loaded = &loaded.projects[0].features[0].sessions[0];
        assert_eq!(loaded.kind, SessionKind::Codex);
        assert_eq!(loaded.command, original.command);
    }

    #[test]
    fn commands_with_no_declaration_or_conflicting_declarations_remain_generic() {
        let root = tempfile::tempdir().unwrap();
        let mut store = fixture(root.path());
        assert!(!reconcile(&mut store, &ExtensionConfig::default()));
        let config = ExtensionConfig {
            custom_sessions: vec![
                CustomSessionConfig {
                    name: "agent".into(),
                    agent: Some(AgentKind::Codex),
                    command: Some("python3 launcher.py".into()),
                    ..Default::default()
                },
                CustomSessionConfig {
                    name: "generic".into(),
                    command: Some("python3 launcher.py".into()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(!reconcile(&mut store, &config));
        assert_eq!(
            store.projects[0].features[0].sessions[0].kind,
            SessionKind::Custom
        );
    }

    #[test]
    fn python_exec_receives_launch_arguments_identity_and_restores_path() {
        let root = tempfile::Builder::new()
            .prefix("custom agent ' ")
            .tempdir()
            .unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let fake = bin.join("codex");
        std::fs::write(
            &fake,
            r#"#!/usr/bin/env python3
import json, os, shutil, sys
print(json.dumps(dict(argv=sys.argv[1:], active=os.environ['AMF_ACTIVE'],
    session=os.environ['AMF_FEATURE_SESSION_ID'], window=os.environ['AMF_TMUX_WINDOW'],
    tmux=os.environ['AMF_TMUX_SESSION'], resolved=shutil.which('codex'),
    shim_env='AMF_CUSTOM_AGENT_BIN' in os.environ)))
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            root.path().join("launcher.py"),
            "import os\nos.execvp('codex', ['codex', '--model', \"custom ' model\"])\n",
        )
        .unwrap();
        let mut store = fixture(root.path());
        let session = &mut store.projects[0].features[0].sessions[0];
        session.kind = SessionKind::Codex;
        let expected = vec![
            "-c".into(),
            "notify=[\"a path with ' quote\"]".into(),
            "-c".into(),
            "developer_instructions=\"first\\nsecond\"".into(),
        ];
        let command =
            shell_command("amf-test-main", root.path(), session, expected.clone()).unwrap();
        // Production intentionally prepends installed harness directories.
        // Override that PATH inside the launch environment, so this fixture
        // can never resolve the host's real Codex executable.
        let fixture_path = format!("{}:/usr/bin:/bin", bin.display());
        let command = command.replacen(
            " bash -c ",
            &format!(" env PATH={} bash -c ", quote(&fixture_path)),
            1,
        );
        let output = std::process::Command::new("bash")
            .args(["-c", &command])
            .current_dir(root.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut expected = expected;
        expected.extend(["--model".into(), "custom ' model".into()]);
        assert_eq!(value["argv"], serde_json::json!(expected));
        assert_eq!(value["session"], session.id);
        assert_eq!(value["active"], "1");
        assert_eq!(value["window"], "orchestrator");
        assert_eq!(value["tmux"], "amf-test-main");
        assert_eq!(value["resolved"], fake.to_string_lossy().as_ref());
        assert_eq!(value["shim_env"], false);
    }
}
