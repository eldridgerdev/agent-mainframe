//! Rename a feature or session, mark a feature ready, and generate its AI
//! summary: the GUI's counterparts of the dashboard's `r`, `y` and `Z`.
//!
//! Each action drives the same `App` method the TUI runs
//! (`apply_rename_feature`, `apply_rename_session`, `toggle_feature_ready`,
//! `trigger_summary_for_indices`), addressed by stable ids instead of the
//! dashboard cursor. Every write echoes the value the caller saw
//! (`expected`), so a change made elsewhere is refused rather than
//! overwritten, and nothing retargets silently.
//!
//! Like the TUI, a rename changes the stored display value only: no tmux
//! window, branch or worktree is renamed. A feature "rename" is its nickname
//! (empty clears it); a session rename is its label.
//!
//! The summary is a headless AI call. The TUI announces it with a toast; the
//! GUI is stricter and requires an acknowledgement of the exact prompt and
//! harness shown by [`summary_notice`] before anything is spawned.
use crate::app::{App, Selection};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::project::AgentKind;
use crate::prompts::PromptId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
pub struct SessionLabel {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Serialize)]
pub struct FeatureActionsView {
    pub name: String,
    pub nickname: Option<String>,
    pub ready: bool,
    pub summary: Option<String>,
    pub summary_generating: bool,
    pub sessions: Vec<SessionLabel>,
    /// The outcome of a summary that finished since the last load (success
    /// or the failure text). Taken once.
    pub summary_outcome: Option<String>,
}

fn locate(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<(usize, usize)> {
    gui.refresh_store()?;
    gui.locate(target)
}

pub fn load(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<FeatureActionsView> {
    let (pi, fi) = locate(gui, &target)?;
    let session = gui.app_for_workflow().store.projects[pi].features[fi]
        .tmux_session
        .clone();
    let summary_outcome = match gui.summary_outcome.take() {
        Some((owner, text)) if owner == session => Some(text),
        other => {
            gui.summary_outcome = other;
            None
        }
    };
    let app = gui.app_for_workflow();
    let feature = &app.store.projects[pi].features[fi];
    Ok(FeatureActionsView {
        name: feature.name.clone(),
        nickname: feature.nickname.clone(),
        ready: feature.ready,
        summary: feature.summary.clone(),
        summary_generating: app.summary_state.generating.contains(&feature.tmux_session),
        sessions: feature
            .sessions
            .iter()
            .map(|s| SessionLabel {
                id: s.id.clone(),
                label: s.label.clone(),
            })
            .collect(),
        summary_outcome,
    })
}

/// Run a TUI `apply_*`/toggle on a selection, restoring the selection and
/// rolling the in-memory edit back when the save fails without adopting a
/// newer store (the shared save adopts the winner on a conflict).
fn run_selected(
    app: &mut App,
    selection: Selection,
    action: impl FnOnce(&mut App) -> anyhow::Result<()>,
) -> GuiResult<()> {
    let previous_selection = std::mem::replace(&mut app.selection, selection);
    let previous_store = app.store.clone();
    let version = app.store_version;
    let result = action(app);
    app.selection = previous_selection;
    if let Err(error) = result {
        if app.store_version == version {
            app.store = previous_store;
        }
        return Err(GuiError::from(error));
    }
    Ok(())
}

fn reject_control_characters(value: &str, what: &str) -> GuiResult<()> {
    if value.chars().any(char::is_control) {
        return Err(GuiError::conflict(format!(
            "{what} cannot contain line breaks or control characters"
        )));
    }
    Ok(())
}

/// Set (or, when empty, clear) the feature's nickname. `expected` is the
/// nickname the caller saw; a different current value is a conflict.
pub fn rename_feature(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    expected: Option<String>,
    nickname: String,
) -> GuiResult<FeatureActionsView> {
    reject_control_characters(&nickname, "A nickname")?;
    let (pi, fi) = locate(gui, &target)?;
    let app = gui.app_for_workflow();
    if app.store.projects[pi].features[fi].nickname != expected.filter(|v| !v.is_empty()) {
        return Err(GuiError::conflict(
            "The nickname changed since you opened this. Reload and try again.",
        ));
    }
    run_selected(app, Selection::Feature(pi, fi), |app| {
        app.start_rename_feature();
        if let crate::app::AppMode::RenamingFeature(state) = &mut app.mode {
            state.input = nickname;
        }
        let result = app.apply_rename_feature();
        app.mode = crate::app::AppMode::Normal;
        result
    })?;
    load(gui, target)
}

/// Rename one session's label. `expected` is the label the caller saw.
pub fn rename_session(
    gui: &mut GuiHandle,
    target: SessionTarget,
    expected: String,
    label: String,
) -> GuiResult<FeatureActionsView> {
    reject_control_characters(&label, "A session name")?;
    if label.trim().is_empty() {
        // The TUI's own refusal, stated before touching anything.
        return Err(GuiError::conflict("Name cannot be empty"));
    }
    let feature_target = FeatureTarget {
        project_id: target.project_id.clone(),
        feature_id: target.feature_id.clone(),
    };
    let (pi, fi) = locate(gui, &feature_target)?;
    let app = gui.app_for_workflow();
    let sessions = &app.store.projects[pi].features[fi].sessions;
    let si = sessions
        .iter()
        .position(|s| s.id == target.session_id)
        .ok_or_else(|| GuiError::not_found("The selected session no longer exists"))?;
    if sessions[si].label != expected {
        return Err(GuiError::conflict(
            "The session name changed since you opened this. Reload and try again.",
        ));
    }
    run_selected(app, Selection::Session(pi, fi, si), |app| {
        app.start_rename_session();
        if let crate::app::AppMode::RenamingSession(state) = &mut app.mode {
            state.input = label;
        }
        let result = app.apply_rename_session();
        app.mode = crate::app::AppMode::Normal;
        result
    })?;
    load(gui, feature_target)
}

/// Set the ready flag. The TUI's `y` is a toggle, so `expected` (the value
/// the caller saw) guards against toggling the wrong way after a change made
/// elsewhere; the requested end state is `!expected`.
pub fn set_ready(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    expected: bool,
) -> GuiResult<FeatureActionsView> {
    let (pi, fi) = locate(gui, &target)?;
    let app = gui.app_for_workflow();
    if app.store.projects[pi].features[fi].ready != expected {
        return Err(GuiError::conflict(
            "The ready state changed since you opened this. Reload and try again.",
        ));
    }
    run_selected(app, Selection::Feature(pi, fi), App::toggle_feature_ready)?;
    load(gui, target)
}

/// What the person must see before the GUI makes the headless call.
#[derive(Debug, Serialize)]
pub struct SummaryNotice {
    pub prompt_id: String,
    pub title: String,
    pub harness: String,
    pub harness_name: String,
    pub template: String,
    /// Binds the confirmation to exactly this prompt text and harness.
    pub revision: String,
}

#[derive(Debug, Deserialize)]
pub struct SummaryAck {
    pub prompt_id: String,
    pub harness: String,
    pub revision: String,
}

fn revision(harness: &AgentKind, template: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(harness.slug().as_bytes());
    hasher.update([0]);
    hasher.update(template.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn notice_for(app: &App, pi: usize, fi: usize) -> GuiResult<SummaryNotice> {
    let project = &app.store.projects[pi];
    let feature = &project.features[fi];
    if app.summary_state.generating.contains(&feature.tmux_session) {
        return Err(GuiError::conflict("Summary already generating..."));
    }
    if app.summary_rx.is_some() {
        return Err(GuiError::conflict(
            "Another summary is still generating. Wait for it to finish.",
        ));
    }
    if app.summary_window_for(pi, fi).is_none() {
        return Err(GuiError::conflict(
            "No agent window found: this feature has no agent session to summarize.",
        ));
    }
    if !app.tmux.session_exists(&feature.tmux_session) {
        return Err(GuiError::conflict(
            "The feature is stopped; start it before generating a summary.",
        ));
    }
    let (template, _) = app.resolve_headless_template(
        PromptId::SessionSummary,
        &feature.agent,
        &project.repo,
        &feature.workdir,
    );
    Ok(SummaryNotice {
        prompt_id: PromptId::SessionSummary.as_str().into(),
        title: PromptId::SessionSummary.spec().title.into(),
        harness: feature.agent.slug().into(),
        harness_name: feature.agent.display_name().into(),
        revision: revision(&feature.agent, &template),
        template,
    })
}

/// The pre-call notice. Spawns nothing.
pub fn summary_notice(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<SummaryNotice> {
    let (pi, fi) = locate(gui, &target)?;
    gui.reject_ambiguous_live_session(pi, fi)?;
    notice_for(gui.app_for_workflow(), pi, fi)
}

pub fn summary_generate(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    ack: SummaryAck,
) -> GuiResult<FeatureActionsView> {
    summary_generate_with(
        gui,
        target,
        ack,
        crate::summary::SummaryManager::generate_summary,
    )
}

pub(crate) fn summary_generate_with(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    ack: SummaryAck,
    generate: crate::app::SummaryGenerator,
) -> GuiResult<FeatureActionsView> {
    let (pi, fi) = locate(gui, &target)?;
    gui.reject_ambiguous_live_session(pi, fi)?;
    let app = gui.app_for_workflow();
    // Re-resolve now: an override saved after the notice, or a changed
    // harness, must be shown again rather than silently used.
    let notice = notice_for(app, pi, fi)?;
    if ack.prompt_id != notice.prompt_id
        || ack.harness != notice.harness
        || ack.revision != notice.revision
    {
        return Err(GuiError::conflict(
            "The prompt or harness changed after the notice was shown. Review it again.",
        ));
    }
    gui.summary_outcome = None;
    let app = gui.app_for_workflow();
    app.message = None;
    app.trigger_summary_for_indices(pi, fi, generate)
        .map_err(GuiError::from)?;
    // The trigger reports refusals only through `message`; nothing running
    // means it refused.
    if app.summary_rx.is_none() {
        let reason = app
            .message
            .take()
            .unwrap_or_else(|| "The summary could not be started.".into());
        return Err(GuiError::conflict(reason));
    }
    load(gui, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::AmfDb;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, SessionKind, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::Path;

    struct Fixture {
        dir: tempfile::TempDir,
        gui: GuiHandle,
        feature: FeatureTarget,
        session: SessionTarget,
    }

    fn fixture(tmux_running: bool) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let mut project =
            Project::new("Project".into(), dir.path().into(), true, AgentKind::Claude);
        let mut feature = Feature::new_for_project(
            "Project",
            "Feature".into(),
            "feature".into(),
            dir.path().into(),
            true,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        let session_id = feature.add_session(SessionKind::Claude).id.clone();
        let feature_target = FeatureTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let db = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        // No other expectation: a rename or toggle that touched tmux would panic.
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(tmux_running);
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.db = Some(db);
        app.store_version = Some(version);
        Fixture {
            dir,
            gui: GuiHandle::from_app(app),
            session: SessionTarget {
                project_id: feature_target.project_id.clone(),
                feature_id: feature_target.feature_id.clone(),
                session_id,
            },
            feature: feature_target,
        }
    }

    fn stored(f: &Fixture) -> crate::project::Feature {
        AmfDb::open(&f.dir.path().join("amf.db"))
            .unwrap()
            .load_store()
            .unwrap()
            .projects[0]
            .features[0]
            .clone()
    }

    fn external(f: &Fixture, edit: impl FnOnce(&mut crate::project::Feature)) {
        let db = AmfDb::open(&f.dir.path().join("amf.db")).unwrap();
        let mut store = db.load_store().unwrap();
        edit(&mut store.projects[0].features[0]);
        db.save_store(&store).unwrap();
    }

    #[test]
    fn nickname_sets_clears_and_refuses_a_stale_or_multiline_value() {
        let mut f = fixture(false);
        let view = rename_feature(&mut f.gui, f.feature.clone(), None, "Shiny".into()).unwrap();
        assert_eq!(view.nickname.as_deref(), Some("Shiny"));
        assert_eq!(stored(&f).nickname.as_deref(), Some("Shiny"));
        // The branch and name are untouched: only the display nickname moves.
        assert_eq!(
            (stored(&f).name.as_str(), stored(&f).branch.as_str()),
            ("Feature", "feature")
        );

        external(&f, |feature| feature.nickname = Some("Elsewhere".into()));
        let stale = rename_feature(
            &mut f.gui,
            f.feature.clone(),
            Some("Shiny".into()),
            "X".into(),
        )
        .unwrap_err();
        assert!(stale.message.contains("changed"), "{}", stale.message);
        assert_eq!(stored(&f).nickname.as_deref(), Some("Elsewhere"));

        assert!(
            rename_feature(
                &mut f.gui,
                f.feature.clone(),
                Some("Elsewhere".into()),
                "a\nb".into()
            )
            .is_err()
        );
        let cleared = rename_feature(
            &mut f.gui,
            f.feature.clone(),
            Some("Elsewhere".into()),
            String::new(),
        )
        .unwrap();
        assert_eq!(cleared.nickname, None);
        assert_eq!(stored(&f).nickname, None);
    }

    #[test]
    fn session_rename_validates_and_refuses_stale_or_missing_targets() {
        let mut f = fixture(false);
        let old = stored(&f).sessions[0].label.clone();
        assert!(
            rename_session(&mut f.gui, f.session.clone(), old.clone(), "  ".into())
                .unwrap_err()
                .message
                .contains("empty")
        );
        let view =
            rename_session(&mut f.gui, f.session.clone(), old.clone(), "Main".into()).unwrap();
        assert_eq!(view.sessions[0].label, "Main");
        assert_eq!(stored(&f).sessions[0].label, "Main");
        assert!(
            rename_session(&mut f.gui, f.session.clone(), old, "Again".into())
                .unwrap_err()
                .message
                .contains("changed")
        );
        let mut gone = f.session.clone();
        gone.session_id = "nope".into();
        assert!(rename_session(&mut f.gui, gone, "Main".into(), "X".into()).is_err());
        let mut other_feature = f.session.clone();
        other_feature.feature_id = "nope".into();
        assert!(rename_session(&mut f.gui, other_feature, "Main".into(), "X".into()).is_err());
        assert_eq!(stored(&f).sessions[0].label, "Main");
    }

    #[test]
    fn ready_flag_is_the_persisted_field_and_a_stale_toggle_is_refused() {
        let mut f = fixture(false);
        assert!(
            set_ready(&mut f.gui, f.feature.clone(), false)
                .unwrap()
                .ready
        );
        assert!(stored(&f).ready);
        external(&f, |feature| feature.ready = false);
        let err = set_ready(&mut f.gui, f.feature.clone(), true).unwrap_err();
        assert!(err.message.contains("changed"));
        assert!(!stored(&f).ready);
    }

    #[test]
    fn a_failed_write_does_not_leave_a_phantom_rename() {
        let mut f = fixture(false);
        f.gui.app_for_workflow().db =
            Some(AmfDb::open_read_only(&f.dir.path().join("amf.db")).unwrap());
        assert!(rename_feature(&mut f.gui, f.feature.clone(), None, "Ghost".into()).is_err());
        assert_eq!(
            f.gui.app_for_workflow().store.projects[0].features[0].nickname,
            None
        );
        assert!(set_ready(&mut f.gui, f.feature.clone(), false).is_err());
        assert!(!f.gui.app_for_workflow().store.projects[0].features[0].ready);
    }

    fn ack(notice: &SummaryNotice) -> SummaryAck {
        SummaryAck {
            prompt_id: notice.prompt_id.clone(),
            harness: notice.harness.clone(),
            revision: notice.revision.clone(),
        }
    }

    fn fake_ok(_: &str, _: &str, _: &Path, _: AgentKind, template: &str) -> anyhow::Result<String> {
        assert!(template.contains("{{recent_lines}}") || !template.is_empty());
        Ok("Fake one-line summary".into())
    }

    fn fake_err(_: &str, _: &str, _: &Path, _: AgentKind, _: &str) -> anyhow::Result<String> {
        anyhow::bail!("model unavailable")
    }

    fn fake_unreachable(
        _: &str,
        _: &str,
        _: &Path,
        _: AgentKind,
        _: &str,
    ) -> anyhow::Result<String> {
        panic!("no call may be made without a matching acknowledgement")
    }

    #[test]
    fn the_notice_names_the_prompt_and_harness_and_spawns_nothing() {
        let mut f = fixture(true);
        let notice = summary_notice(&mut f.gui, f.feature.clone()).unwrap();
        assert_eq!(notice.prompt_id, "session.summary");
        assert_eq!(notice.harness, "claude");
        assert!(!notice.template.is_empty());
        assert!(f.gui.app_for_workflow().summary_rx.is_none());
        assert!(f.gui.app_for_workflow().summary_state.generating.is_empty());
    }

    #[test]
    fn a_changed_prompt_or_wrong_acknowledgement_never_reaches_the_harness() {
        let mut f = fixture(true);
        let notice = summary_notice(&mut f.gui, f.feature.clone()).unwrap();
        for bad in [
            SummaryAck {
                revision: "stale".into(),
                ..ack(&notice)
            },
            SummaryAck {
                harness: "codex".into(),
                ..ack(&notice)
            },
            SummaryAck {
                prompt_id: "learning.answer".into(),
                ..ack(&notice)
            },
        ] {
            let err = summary_generate_with(&mut f.gui, f.feature.clone(), bad, fake_unreachable)
                .unwrap_err();
            assert!(
                err.message.contains("changed after the notice"),
                "{}",
                err.message
            );
        }
        assert!(f.gui.app_for_workflow().summary_rx.is_none());
    }

    #[test]
    fn a_stopped_feature_or_missing_agent_window_is_refused_with_a_reason() {
        let mut stopped = fixture(false);
        let err = summary_notice(&mut stopped.gui, stopped.feature.clone()).unwrap_err();
        assert!(err.message.contains("stopped"), "{}", err.message);
        let mut f = fixture(true);
        external(&f, |feature| feature.sessions.clear());
        let err = summary_notice(&mut f.gui, f.feature.clone()).unwrap_err();
        assert!(err.message.contains("No agent window"), "{}", err.message);
    }

    fn wait_for_outcome(f: &mut Fixture) -> String {
        for _ in 0..1500 {
            f.gui.refresh_store().unwrap();
            if let Some((_, outcome)) = f.gui.summary_outcome.take() {
                return outcome;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("summary did not finish");
    }

    #[test]
    fn a_confirmed_summary_is_persisted_once_and_blocks_a_second_run_meanwhile() {
        let mut f = fixture(true);
        let notice = summary_notice(&mut f.gui, f.feature.clone()).unwrap();
        let view =
            summary_generate_with(&mut f.gui, f.feature.clone(), ack(&notice), fake_ok).unwrap();
        // Either still running or already finished; a second request is refused while running.
        if view.summary_generating {
            let again = summary_notice(&mut f.gui, f.feature.clone());
            assert!(again.is_err() || f.gui.app_for_workflow().summary_rx.is_none());
        }
        let outcome = wait_for_outcome(&mut f);
        assert!(outcome.contains("Fake one-line summary"), "{outcome}");
        assert_eq!(stored(&f).summary.as_deref(), Some("Fake one-line summary"));
        let view = load(&mut f.gui, f.feature.clone()).unwrap();
        assert_eq!(view.summary.as_deref(), Some("Fake one-line summary"));
        assert!(!view.summary_generating);
        assert_eq!(
            view.summary_outcome, None,
            "the outcome is handed over once"
        );
    }

    #[test]
    fn a_failed_summary_reports_the_failure_and_keeps_the_old_summary() {
        let mut f = fixture(true);
        external(&f, |feature| feature.summary = Some("Previous".into()));
        let notice = summary_notice(&mut f.gui, f.feature.clone()).unwrap();
        summary_generate_with(&mut f.gui, f.feature.clone(), ack(&notice), fake_err).unwrap();
        let outcome = wait_for_outcome(&mut f);
        assert!(outcome.contains("Failed to generate summary"), "{outcome}");
        assert!(outcome.contains("model unavailable"), "{outcome}");
        assert_eq!(stored(&f).summary.as_deref(), Some("Previous"));
    }
}
