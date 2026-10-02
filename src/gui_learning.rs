//! GUI Learning adapter. Browsing and questions reuse the TUI engines;
//! stable workflow identities keep delayed GUI actions off a reopened reader.
use serde::{Deserialize, Serialize};

use crate::app::{
    AppMode, BrowseScope, LearningAnchor, LearningLevel, LearningListEntry, LearningListGroup,
    LearningQaIntent,
};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::project::{AgentKind, SessionKind};

pub(crate) struct LearningContext {
    id: String,
    target: FeatureTarget,
    revision: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearningEntry {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearningAnswerView {
    pub id: String,
    pub parent_id: Option<String>,
    pub question: String,
    pub answer: Option<String>,
    pub anchor: String,
    pub status: String,
    pub intent: String,
    pub run_mode: String,
    pub harness: AgentKind,
    pub error: Option<String>,
    pub drift: Option<String>,
    pub spawned_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearningView {
    pub workflow_id: String,
    pub revision: u64,
    pub target: FeatureTarget,
    pub feature_name: String,
    pub scope: String,
    pub is_git: bool,
    pub entries: Vec<LearningEntry>,
    pub content_path: Option<String>,
    pub content: Vec<String>,
    pub content_line_labels: Vec<String>,
    pub content_error: Option<String>,
    pub anchor: String,
    pub harness: AgentKind,
    pub harnesses: Vec<AgentKind>,
    pub level: String,
    pub history_saved: bool,
    pub qa: Vec<LearningAnswerView>,
    pub error: Option<String>,
    pub notice: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LearningAction {
    SelectEntry {
        key: String,
    },
    ToggleScope,
    Refresh,
    ProjectAnchor,
    FileAnchor,
    LinesAnchor {
        start: usize,
        end: usize,
    },
    Settings {
        harness: AgentKind,
        level: String,
    },
    Ask {
        question: String,
        intent: String,
        parent_id: Option<String>,
    },
    DeepDive {
        qa_id: String,
    },
    Close,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearningHandoff {
    pub target: SessionTarget,
    pub draft_prompt: String,
    pub notice: Option<String>,
}

fn entry_key(entry: &LearningListEntry) -> String {
    match entry {
        LearningListEntry::StartHereHeader => "start_here".into(),
        LearningListEntry::ProjectTour => "project".into(),
        LearningListEntry::Dir { path, .. } => format!("dir:{path}"),
        LearningListEntry::File {
            path,
            group: LearningListGroup::StartHere,
            ..
        } => format!("start_file:{path}"),
        LearningListEntry::File { path, .. } => format!("file:{path}"),
    }
}

/// Refresh stable target indices before using any selection-dependent engine.
fn validate(gui: &mut GuiHandle, id: &str, revision: Option<u64>) -> GuiResult<()> {
    gui.refresh_snapshot()?;
    let context = gui
        .learning_context
        .as_ref()
        .filter(|c| c.id == id && revision.is_none_or(|r| c.revision == r))
        .ok_or_else(|| GuiError::conflict("Learning changed; refresh and retry"))?;
    let target = context.target.clone();
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The Learning feature was deleted"))?;
    let AppMode::Learning(state) = &mut app.mode else {
        return Err(GuiError::conflict("Learning is no longer open"));
    };
    if state.workdir != app.store.projects[pi].features[fi].workdir {
        return Err(GuiError::conflict(
            "The feature's checkout changed; reopen Learning",
        ));
    }
    state.pi = pi;
    state.fi = fi;
    Ok(())
}

pub fn begin(gui: &mut GuiHandle, target: FeatureTarget) -> GuiResult<LearningView> {
    gui.refresh_snapshot()?;
    if gui.learning_context.is_some() && matches!(gui.app_for_workflow().mode, AppMode::Learning(_))
    {
        // Repeated opens reuse the reader and its unsent context.
        let c = gui.learning_context.as_ref().unwrap();
        if c.target.project_id == target.project_id && c.target.feature_id == target.feature_id {
            return snapshot(gui)?.ok_or_else(|| GuiError::conflict("Learning is no longer open"));
        }
        return Err(GuiError::conflict(
            "Close Learning before opening another feature",
        ));
    }
    let app = gui.app_for_workflow();
    if !matches!(app.mode, AppMode::Normal) || app.paused_plan_interview.is_some() {
        return Err(GuiError::conflict(
            "Finish the current workflow before opening Learning",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    app.open_learning_mode(pi, fi).map_err(GuiError::from)?;
    gui.learning_context = Some(LearningContext {
        id: uuid::Uuid::new_v4().to_string(),
        target,
        revision: 0,
    });
    snapshot(gui)?.ok_or_else(|| GuiError::conflict("Learning could not open"))
}

pub fn snapshot(gui: &mut GuiHandle) -> GuiResult<Option<LearningView>> {
    let Some(c) = &gui.learning_context else {
        return Ok(None);
    };
    let id = c.id.clone();
    if !matches!(gui.app_for_workflow().mode, AppMode::Learning(_)) {
        gui.learning_context = None;
        return Ok(None);
    }
    validate(gui, &id, None)?;
    let c = gui.learning_context.as_ref().unwrap();
    let (workflow_id, revision, target) = (c.id.clone(), c.revision, c.target.clone());
    let app = gui.app_for_workflow();
    let AppMode::Learning(s) = &app.mode else {
        unreachable!()
    };
    Ok(Some(LearningView {
        workflow_id,
        revision,
        target,
        feature_name: s.feature_name.clone(),
        scope: if s.scope == BrowseScope::RepoTree {
            "repo_tree"
        } else {
            "branch_changes"
        }
        .into(),
        is_git: s.is_git,
        entries: s
            .entries
            .iter()
            .map(|e| LearningEntry {
                key: entry_key(e),
                label: match e {
                    LearningListEntry::StartHereHeader => "Start here".into(),
                    LearningListEntry::ProjectTour => "Tour this project".into(),
                    LearningListEntry::Dir {
                        path, truncated, ..
                    } => {
                        if *truncated > 0 {
                            format!("{path} ({truncated} more items)")
                        } else {
                            path.clone()
                        }
                    }
                    LearningListEntry::File { path, .. } => path.clone(),
                },
                kind: match e {
                    LearningListEntry::Dir { .. } => "dir",
                    LearningListEntry::File { .. } => "file",
                    LearningListEntry::ProjectTour => "project",
                    _ => "header",
                }
                .into(),
                depth: e.depth(),
                expanded: matches!(e, LearningListEntry::Dir { expanded: true, .. }),
            })
            .collect(),
        content_path: s.content_path.clone(),
        content: if s.scope == BrowseScope::BranchChanges {
            s.selected_diff_file()
                .map(|f| f.addressable_line_diff_texts())
                .unwrap_or_default()
        } else {
            s.content.clone()
        },
        content_line_labels: if s.scope == BrowseScope::BranchChanges {
            s.selected_diff_file()
                .map(|f| {
                    f.addressable_lines()
                        .iter()
                        .map(|l| match (l.old_line, l.new_line) {
                            (Some(old), Some(new)) => format!("{old}→{new}"),
                            (Some(old), None) => format!("{old}→−"),
                            (None, Some(new)) => format!("+{new}"),
                            _ => String::new(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        } else {
            (1..=s.content.len()).map(|n| n.to_string()).collect()
        },
        content_error: s.content_error.clone(),
        anchor: s.anchor.describe(s.content_path.as_deref()),
        harness: s.harness.clone(),
        harnesses: AgentKind::ALL.to_vec(),
        level: s.level.as_str().into(),
        history_saved: !s.session_id.is_empty(),
        qa: s
            .qa
            .iter()
            .map(|q| LearningAnswerView {
                id: q.id.clone(),
                parent_id: q.parent_qa_id.clone(),
                question: q.question.clone(),
                answer: q.answer.clone(),
                anchor: q.anchor.describe(q.file_path.as_deref()),
                status: q.status.as_str().into(),
                intent: q.intent.as_str().into(),
                run_mode: q.run_mode.description().into(),
                harness: q.harness.clone(),
                error: q.error.clone(),
                drift: s
                    .drift_for(&q.id)
                    .map(|d| d.describe(q.anchor.line_range_for_display())),
                spawned_session_id: q.spawned_session_id.clone(),
            })
            .collect(),
        error: s.error.as_ref().map(|error| {
            error
                .replace("Press the scope key", "Switch the file scope")
                .replace(" (F)", "")
                .replace(" (D on the dashboard)", "")
        }),
        notice: s.notice.clone(),
    }))
}

pub fn act(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    action: LearningAction,
) -> GuiResult<Option<LearningView>> {
    if matches!(action, LearningAction::Close) {
        let valid = gui
            .learning_context
            .as_ref()
            .is_some_and(|c| c.id == workflow_id && c.revision == revision);
        if !valid || !matches!(gui.app_for_workflow().mode, AppMode::Learning(_)) {
            return Err(GuiError::conflict("Learning changed; refresh and retry"));
        }
        // A deleted feature must not trap the reader open.
        gui.app_for_workflow().close_learning_mode();
        gui.learning_context = None;
        return Ok(None);
    }
    validate(gui, workflow_id, Some(revision))?;
    let app = gui.app_for_workflow();
    match action {
        LearningAction::Close => unreachable!("handled before target validation"),
        LearningAction::SelectEntry { key } => {
            let AppMode::Learning(s) = &mut app.mode else {
                unreachable!()
            };
            s.selected_entry = s
                .entries
                .iter()
                .position(|e| entry_key(e) == key)
                .ok_or_else(|| GuiError::not_found("File entry changed; refresh Learning"))?;
            match s.selected_entry() {
                Some(LearningListEntry::Dir { .. }) => {
                    app.learning_toggle_dir();
                }
                Some(LearningListEntry::StartHereHeader) => app.learning_toggle_start_here(),
                _ => app.learning_load_selected_content(),
            }
        }
        LearningAction::ToggleScope => app.learning_toggle_scope(),
        LearningAction::Refresh => {
            app.learning_reload_entries();
            app.learning_load_selected_content();
            app.learning_check_anchor_drift();
        }
        LearningAction::ProjectAnchor => app.learning_select_project(),
        LearningAction::FileAnchor => app.learning_select_whole_file(),
        LearningAction::LinesAnchor { start, end } => {
            let AppMode::Learning(s) = &mut app.mode else {
                unreachable!()
            };
            let count = s.selectable_line_count();
            if start == 0 || end < start || end > count || s.content_error.is_some() {
                return Err(GuiError::conflict(
                    "Select a valid range in the loaded file",
                ));
            }
            s.cursor_line = end - 1;
            s.selection_anchor = Some(start - 1);
            s.anchor = LearningAnchor::File;
            // Repo/diff line mapping remains owned by the shared engine.
            app.learning_cursor_move(0);
        }
        LearningAction::Settings { harness, level } => {
            let level = match level.as_str() {
                "newcomer" => LearningLevel::Newcomer,
                "familiar" => LearningLevel::Familiar,
                _ => return Err(GuiError::conflict("Unknown Learning level")),
            };
            let AppMode::Learning(s) = &mut app.mode else {
                unreachable!()
            };
            if let Some(db) = &app.db
                && !s.session_id.is_empty()
            {
                db.set_learning_session_settings(&s.session_id, &harness, level)
                    .map_err(GuiError::from)?;
            }
            s.harness = harness;
            s.level = level;
        }
        LearningAction::Ask {
            question,
            intent,
            parent_id,
        } => {
            if question.trim().is_empty() {
                return Err(GuiError::conflict("Enter a question first"));
            }
            let intent = match intent.as_str() {
                "explain" => LearningQaIntent::Explain,
                "action" => LearningQaIntent::Action,
                _ => return Err(GuiError::conflict("Unknown question intent")),
            };
            let AppMode::Learning(s) = &mut app.mode else {
                unreachable!()
            };
            if s.content_error.is_some()
                && parent_id.is_none()
                && s.anchor != LearningAnchor::Project
            {
                return Err(GuiError::conflict(
                    "This file could not be read; choose another file",
                ));
            }
            s.error = None;
            if let Some(parent) = &parent_id {
                s.selected_qa =
                    s.qa.iter().position(|q| &q.id == parent).ok_or_else(|| {
                        GuiError::not_found("That question is no longer in Learning")
                    })?;
                app.learning_open_follow_up();
                let AppMode::Learning(s) = &mut app.mode else {
                    unreachable!()
                };
                let editor = s.question.as_mut().ok_or_else(|| {
                    GuiError::conflict(
                        s.error
                            .clone()
                            .unwrap_or_else(|| "The answer is not ready".into()),
                    )
                })?;
                editor.editor = crate::editor::TextEditor::new(question);
                editor.intent = intent;
                app.learning_submit_question();
            } else {
                app.learning_ask_at(&question, intent, None, None);
            }
        }
        LearningAction::DeepDive { qa_id } => {
            let AppMode::Learning(s) = &mut app.mode else {
                unreachable!()
            };
            s.selected_qa =
                s.qa.iter()
                    .position(|q| q.id == qa_id)
                    .ok_or_else(|| GuiError::not_found("Question was removed"))?;
            app.learning_deep_dive();
        }
    }
    gui.learning_context.as_mut().unwrap().revision += 1;
    snapshot(gui)
}

/// Crossing into editing uses the existing GUI resource gate and session
/// engine. The shared Learning seed stays editable and unsent.
pub fn launch_agent(
    gui: &mut GuiHandle,
    workflow_id: &str,
    revision: u64,
    qa_id: &str,
    approved: bool,
) -> GuiResult<LearningHandoff> {
    validate(gui, workflow_id, Some(revision))?;
    let target = gui.learning_context.as_ref().unwrap().target.clone();
    let app = gui.app_for_workflow();
    let AppMode::Learning(s) = &app.mode else {
        unreachable!()
    };
    let qa =
        s.qa.iter()
            .find(|q| q.id == qa_id)
            .cloned()
            .ok_or_else(|| GuiError::not_found("Question was removed"))?;
    if qa.answer.is_none() {
        return Err(GuiError::conflict(
            "Wait for an answer before opening an editing agent",
        ));
    }
    let draft_prompt = crate::app::learning::escalation_seed(&qa, s.drift_for(qa_id));
    if let Some(session_id) = &qa.spawned_session_id {
        let f = &app.store.projects[s.pi].features[s.fi];
        if let Some(session) = f.sessions.iter().find(|v| &v.id == session_id)
            && app.tmux.session_exists(&f.tmux_session)
            && app
                .tmux
                .window_exists(&f.tmux_session, &session.tmux_window)
        {
            let handoff = LearningHandoff {
                target: SessionTarget {
                    project_id: target.project_id,
                    feature_id: target.feature_id,
                    session_id: session_id.clone(),
                },
                draft_prompt,
                notice: None,
            };
            gui.resolve_session_target(&handoff.target)?;
            gui.app_for_workflow().close_learning_mode();
            gui.learning_context = None;
            return Ok(handoff);
        }
    }
    let kind = match app.store.projects[s.pi].features[s.fi].agent {
        AgentKind::Claude => SessionKind::Claude,
        AgentKind::Codex => SessionKind::Codex,
        AgentKind::Opencode => SessionKind::Opencode,
        AgentKind::Pi => SessionKind::Pi,
    };
    let response = gui.add_session(
        target,
        kind,
        Some(crate::app::learning::learning_session_label(&qa)),
        approved,
    )?;
    let app = gui.app_for_workflow();
    let AppMode::Learning(s) = &mut app.mode else {
        unreachable!()
    };
    let row = s.qa.iter_mut().find(|q| q.id == qa_id).unwrap();
    row.spawned_session_id = Some(response.target.session_id.clone());
    row.updated_at = crate::db::learning::now_timestamp();
    let row = row.clone();
    // Report partial success without hiding the session that already started.
    let notice = app.persist_learning_qa(&row).err().map(|error| {
        format!("Agent started, but the link back to this answer was not saved: {error}")
    });
    if let Some(error) = &notice {
        app.log_warn(
            "learning",
            format!("Agent started; answer link was not saved: {error}"),
        );
    }
    app.close_learning_mode();
    gui.learning_context = None;
    Ok(LearningHandoff {
        target: response.target,
        draft_prompt,
        notice,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::db::AmfDb;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::{Feature, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    fn fixture(tmux: MockTmuxOps) -> (tempfile::TempDir, GuiHandle, FeatureTarget) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "first\nsecond\nthird\n").unwrap();
        std::fs::write(dir.path().join("other.rs"), "other file\n").unwrap();
        let mut project = Project::new("Demo".into(), dir.path().into(), false, AgentKind::Claude);
        let feature = Feature::new_for_project(
            "Demo",
            "Feature".into(),
            "feature".into(),
            dir.path().into(),
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        let target = FeatureTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let db = AmfDb::open(&dir.path().join("learning.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.db = Some(db);
        app.store_version = Some(version);
        (dir, GuiHandle::from_app(app), target)
    }

    fn apply(gui: &mut GuiHandle, view: &LearningView, action: LearningAction) -> LearningView {
        act(gui, &view.workflow_id, view.revision, action)
            .unwrap()
            .unwrap()
    }

    fn ask(gui: &mut GuiHandle, view: &LearningView) -> LearningView {
        apply(
            gui,
            view,
            LearningAction::Ask {
                question: "What does this do?".into(),
                intent: "explain".into(),
                parent_id: None,
            },
        )
    }

    fn deliver(gui: &mut GuiHandle, qa_id: &str) {
        gui.app_for_workflow()
            .learning_runs
            .sender()
            .send(crate::app::learning::LearningAnswer {
                qa_id: qa_id.into(),
                result: Ok("An explanation.".into()),
            })
            .unwrap();
    }

    #[test]
    fn range_questions_capture_code_and_followups_keep_the_original_anchor() {
        let (_dir, mut gui, target) = fixture(MockTmuxOps::new());
        let view = begin(&mut gui, target).unwrap();
        let view = apply(
            &mut gui,
            &view,
            LearningAction::SelectEntry {
                key: "file:README.md".into(),
            },
        );
        let view = apply(
            &mut gui,
            &view,
            LearningAction::LinesAnchor { start: 2, end: 3 },
        );
        assert_eq!(view.anchor, "lines 2-3 of README.md");
        let view = ask(&mut gui, &view);
        let qa_id = view.qa[0].id.clone();
        let AppMode::Learning(s) = &gui.app_for_workflow().mode else {
            panic!()
        };
        assert_eq!(s.qa[0].selection_text, "second\nthird");
        deliver(&mut gui, &qa_id);
        let view = snapshot(&mut gui).unwrap().unwrap();
        let view = apply(
            &mut gui,
            &view,
            LearningAction::SelectEntry {
                key: "file:other.rs".into(),
            },
        );
        let view = apply(
            &mut gui,
            &view,
            LearningAction::Ask {
                question: "Why?".into(),
                intent: "explain".into(),
                parent_id: Some(qa_id.clone()),
            },
        );
        assert_eq!(view.qa[1].parent_id.as_deref(), Some(qa_id.as_str()));
        assert_eq!(view.qa[1].anchor, "lines 2-3 of README.md");
        let AppMode::Learning(s) = &gui.app_for_workflow().mode else {
            panic!()
        };
        assert_eq!(s.qa[1].selection_text, "second\nthird");
    }

    #[test]
    fn stale_and_duplicate_submissions_cannot_enqueue_another_paid_question() {
        let (_dir, mut gui, target) = fixture(MockTmuxOps::new());
        let first = begin(&mut gui, target.clone()).unwrap();
        let keys: std::collections::HashSet<_> =
            first.entries.iter().map(|entry| &entry.key).collect();
        assert_eq!(
            keys.len(),
            first.entries.len(),
            "orientation shortcuts need distinct keys from their tree entries"
        );
        assert_eq!(
            begin(&mut gui, target.clone()).unwrap().workflow_id,
            first.workflow_id
        );
        let current = ask(&mut gui, &first);
        let error = act(
            &mut gui,
            &first.workflow_id,
            first.revision,
            LearningAction::Ask {
                question: "Duplicate".into(),
                intent: "explain".into(),
                parent_id: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert_eq!(snapshot(&mut gui).unwrap().unwrap().qa.len(), 1);
        act(
            &mut gui,
            &current.workflow_id,
            current.revision,
            LearningAction::Close,
        )
        .unwrap();
        let reopened = begin(&mut gui, target).unwrap();
        assert_ne!(reopened.workflow_id, current.workflow_id);
        let error = act(
            &mut gui,
            &current.workflow_id,
            current.revision,
            LearningAction::Close,
        )
        .unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::Conflict);
        assert!(snapshot(&mut gui).unwrap().is_some());
    }

    #[test]
    fn answers_delivered_after_close_are_saved_and_visible_on_reopen() {
        let (_dir, mut gui, target) = fixture(MockTmuxOps::new());
        let view = begin(&mut gui, target.clone()).unwrap();
        let view = ask(&mut gui, &view);
        let qa_id = view.qa[0].id.clone();
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            LearningAction::Close,
        )
        .unwrap();
        deliver(&mut gui, &qa_id);
        gui.refresh_snapshot().unwrap();
        let reopened = begin(&mut gui, target).unwrap();
        assert_eq!(reopened.qa[0].status, "answered");
        assert_eq!(reopened.qa[0].answer.as_deref(), Some("An explanation."));
    }

    #[test]
    fn stable_target_survives_reordering_and_deleted_feature_can_still_close() {
        let (dir, mut gui, target) = fixture(MockTmuxOps::new());
        begin(&mut gui, target.clone()).unwrap();
        let writer = AmfDb::open(&dir.path().join("learning.db")).unwrap();
        let mut store = writer.load_store().unwrap();
        store.projects.insert(
            0,
            Project::new("Other".into(), dir.path().into(), false, AgentKind::Claude),
        );
        writer.save_store(&store).unwrap();
        let view = snapshot(&mut gui).unwrap().unwrap();
        let AppMode::Learning(s) = &gui.app_for_workflow().mode else {
            panic!()
        };
        assert_eq!(s.pi, 1);
        writer.save_store(&ProjectStore::empty()).unwrap();
        let error = act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            LearningAction::Ask {
                question: "Stale".into(),
                intent: "explain".into(),
                parent_id: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::NotFound);
        assert!(
            act(
                &mut gui,
                &view.workflow_id,
                view.revision,
                LearningAction::Close
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn codex_questions_truthfully_report_repository_reading_and_settings_persist() {
        let (_dir, mut gui, target) = fixture(MockTmuxOps::new());
        let view = begin(&mut gui, target.clone()).unwrap();
        let view = apply(
            &mut gui,
            &view,
            LearningAction::Settings {
                harness: AgentKind::Codex,
                level: "familiar".into(),
            },
        );
        let view = ask(&mut gui, &view);
        assert_eq!(view.qa[0].run_mode, "read the repo");
        act(
            &mut gui,
            &view.workflow_id,
            view.revision,
            LearningAction::Close,
        )
        .unwrap();
        let view = begin(&mut gui, target).unwrap();
        assert_eq!(view.harness, AgentKind::Codex);
        assert_eq!(view.level, "familiar");
    }

    #[test]
    fn handoff_waits_for_resource_approval_then_creates_an_unsent_shared_seed() {
        let _lease_lock = crate::resources::limits::lock_lease_tests();
        assert_eq!(crate::resources::limits::wait_for_in_flight(0), 0);
        let _lease = crate::resources::limits::HeadlessLease::acquire();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_list_panes().returning(Vec::new);
        tmux.expect_create_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .times(1)
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_send_keys().never();
        let (_dir, mut gui, target) = fixture(tmux);
        gui.app_for_workflow().config.max_concurrent_agents = 1;
        gui.app_for_workflow().config.low_memory_warn_mb = 0;
        let view = begin(&mut gui, target).unwrap();
        let view = ask(&mut gui, &view);
        let qa_id = view.qa[0].id.clone();
        deliver(&mut gui, &qa_id);
        let view = snapshot(&mut gui).unwrap().unwrap();
        let error =
            launch_agent(&mut gui, &view.workflow_id, view.revision, &qa_id, false).unwrap_err();
        assert_eq!(error.kind, GuiErrorKind::NeedsApproval);
        assert!(view.qa[0].spawned_session_id.is_none());
        let handoff =
            launch_agent(&mut gui, &view.workflow_id, view.revision, &qa_id, true).unwrap();
        assert!(handoff.draft_prompt.contains("An explanation."));
        assert!(snapshot(&mut gui).unwrap().is_none());
        let saved = gui
            .db()
            .unwrap()
            .learning_qa(
                &gui.db()
                    .unwrap()
                    .load_or_create_learning_session(
                        &handoff.target.project_id,
                        &handoff.target.feature_id,
                        "Demo",
                        &AgentKind::Claude,
                        LearningLevel::Newcomer,
                    )
                    .unwrap()
                    .id,
            )
            .unwrap();
        assert_eq!(
            saved[0].spawned_session_id.as_deref(),
            Some(handoff.target.session_id.as_str())
        );
    }

    #[test]
    fn branch_reader_rows_capture_the_displayed_diff_and_deep_dives_are_deduplicated() {
        let (dir, mut gui, target) = fixture(MockTmuxOps::new());
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        std::fs::write(dir.path().join(".gitignore"), "learning.db*\n").unwrap();
        git(&["init", "--initial-branch=main"]);
        git(&["config", "user.name", "AMF Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["add", "README.md", "other.rs"]);
        git(&["commit", "-m", "initial"]);
        git(&["checkout", "-b", "feature"]);
        std::fs::write(dir.path().join("README.md"), "first\nchanged\nthird\n").unwrap();
        let writer = AmfDb::open(&dir.path().join("learning.db")).unwrap();
        let mut store = writer.load_store().unwrap();
        store.projects[0].is_git = true;
        writer.save_store(&store).unwrap();
        let view = begin(&mut gui, target).unwrap();
        let view = apply(&mut gui, &view, LearningAction::ToggleScope);
        assert!(view.error.is_none(), "{:?}", view.error);
        let view = apply(
            &mut gui,
            &view,
            LearningAction::SelectEntry {
                key: "file:README.md".into(),
            },
        );
        let added = view
            .content
            .iter()
            .position(|line| line == "+changed")
            .unwrap();
        assert_eq!(view.content_line_labels[added], "+2");
        let view = apply(
            &mut gui,
            &view,
            LearningAction::LinesAnchor {
                start: added + 1,
                end: added + 1,
            },
        );
        let view = ask(&mut gui, &view);
        let qa_id = view.qa[0].id.clone();
        let AppMode::Learning(s) = &gui.app_for_workflow().mode else {
            panic!()
        };
        assert_eq!(s.qa[0].selection_text, "+changed");
        assert!(s.qa[0].selection_is_diff);
        assert_eq!(s.qa[0].anchor, LearningAnchor::Lines { start: 2, end: 2 });
        deliver(&mut gui, &qa_id);
        let view = snapshot(&mut gui).unwrap().unwrap();
        let view = apply(
            &mut gui,
            &view,
            LearningAction::DeepDive {
                qa_id: qa_id.clone(),
            },
        );
        assert_eq!(view.qa[1].run_mode, "read the repo");
        let view = apply(&mut gui, &view, LearningAction::DeepDive { qa_id });
        assert_eq!(
            view.qa.len(),
            2,
            "repeated deep dives reuse the existing run"
        );
    }
}
