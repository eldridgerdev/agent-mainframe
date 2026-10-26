use super::*;
use crate::app::App;
use crate::db::AmfDb;
use crate::extension::{ExtensionConfig, set_test_global_extension_config};
use crate::gui_contract::GuiErrorKind;
use crate::project::{Feature, Project, ProjectStore, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Fixture {
    root: tempfile::TempDir,
    gui: Mutex<GuiHandle>,
    id: String,
    available: Arc<AtomicUsize>,
    /// The shared setter's own check of the chosen harness, through tmux.
    calls: Arc<AtomicUsize>,
    /// Version probes for the offered choices, outside the GUI lock.
    probes: AtomicUsize,
    probed_while_locked: AtomicBool,
    /// Probes run on concurrent threads; each one's `try_lock` would hold the
    /// GUI lock just long enough to make a sibling's check look "locked".
    lock_check: Mutex<()>,
    conflict_at: Arc<AtomicUsize>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        set_test_global_extension_config(None);
    }
}
impl Fixture {
    fn global(&self) -> std::path::PathBuf {
        self.root.path().join("config.json")
    }
    fn repo(&self) -> std::path::PathBuf {
        self.root.path().join("repo")
    }
    fn db(&self) -> AmfDb {
        AmfDb::open(&self.root.path().join("amf.db")).unwrap()
    }
    fn probe(&self, agent: &AgentKind) -> bool {
        self.probes.fetch_add(1, Ordering::SeqCst);
        {
            let _one_at_a_time = self.lock_check.lock().unwrap();
            if self.gui.try_lock().is_err() {
                self.probed_while_locked.store(true, Ordering::SeqCst);
            }
        }
        let index = AgentKind::ALL
            .iter()
            .position(|candidate| candidate == agent)
            .unwrap();
        self.available.load(Ordering::SeqCst) & (1 << index) != 0
    }
    fn try_load(&self) -> GuiResult<ProjectHarnessChoices> {
        read(&self.gui, &self.id, &self.global(), &|agent| {
            self.probe(agent)
        })
    }
    fn load(&self) -> ProjectHarnessChoices {
        self.try_load().unwrap()
    }
    fn save(&self, revision: &str, next: AgentKind) -> GuiResult<ProjectHarnessChoices> {
        let path = self.global();
        save_at(&self.gui, &self.id, revision, next, &path, &|agent| {
            self.probe(agent)
        })
    }
    fn app(&self) -> std::sync::MutexGuard<'_, GuiHandle> {
        self.gui.lock().unwrap()
    }
    fn edit(&self, edit: impl FnOnce(&mut ProjectStore)) {
        let db = self.db();
        let mut store = db.load_store().unwrap();
        edit(&mut store);
        db.save_store(&store).unwrap();
    }
}
fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut project = Project::new("Project".into(), repo.clone(), true, AgentKind::Codex);
    project.features.push(Feature::new_for_project(
        "Project",
        "Existing".into(),
        "existing".into(),
        repo,
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    ));
    let id = project.id.clone();
    let mut store = ProjectStore::empty();
    store.projects.push(project);
    store.available_harnesses = AgentKind::ALL.to_vec();
    let db_path = root.path().join("amf.db");
    let db = AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    let (_, version) = db.load_store_versioned().unwrap();
    let available = Arc::new(AtomicUsize::new(15));
    let calls = Arc::new(AtomicUsize::new(0));
    let conflict_at = Arc::new(AtomicUsize::new(usize::MAX));
    let (mask, count, conflict) = (available.clone(), calls.clone(), conflict_at.clone());
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available()
        .returning(move |agent| {
            let call = count.fetch_add(1, Ordering::SeqCst) + 1;
            if call == conflict.load(Ordering::SeqCst) {
                let db = AmfDb::open(&db_path).unwrap();
                let mut external = db.load_store().unwrap();
                external.projects[0].features[0].nickname = Some("Sibling change".into());
                db.save_store(&external).unwrap();
            }
            let index = AgentKind::ALL
                .iter()
                .position(|candidate| candidate == agent)
                .unwrap();
            if mask.load(Ordering::SeqCst) & (1 << index) != 0 {
                Ok(())
            } else {
                anyhow::bail!("Harness removed")
            }
        });
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(db);
    app.store_version = Some(version);
    set_test_global_extension_config(Some(ExtensionConfig::default()));
    Fixture {
        root,
        gui: Mutex::new(GuiHandle::from_app(app)),
        id,
        available,
        calls,
        probes: AtomicUsize::new(0),
        probed_while_locked: AtomicBool::new(false),
        lock_check: Mutex::new(()),
        conflict_at,
    }
}

#[test]
fn preference_changes_only_default_and_shared_normalization() {
    let f = fixture();
    let view = f.load();
    assert_eq!(view.default_agent, Some(AgentKind::Codex));
    let before = serde_json::to_value(&f.db().load_store().unwrap().projects[0].features).unwrap();
    let saved = f.save(&view.revision, AgentKind::Opencode).unwrap();
    assert_eq!(saved.settings.preferred_agent, AgentKind::Opencode);
    assert_eq!(saved.default_agent, Some(AgentKind::Opencode));
    assert_eq!(
        serde_json::to_value(&f.db().load_store().unwrap().projects[0].features).unwrap(),
        before
    );
    assert!(!f.repo().join("amf.json").exists());
}

#[test]
fn unavailable_or_outside_saved_catalog_falls_back_without_rewriting_preference() {
    let f = fixture();
    f.available.store(3, Ordering::SeqCst);
    let view = f.load();
    assert_eq!(view.default_agent, Some(AgentKind::Claude));
    assert_eq!(view.settings.preferred_agent, AgentKind::Codex);
    f.edit(|store| store.available_harnesses = vec![AgentKind::Opencode]);
    let view = f.load();
    assert_eq!(view.agents, vec![AgentKind::Opencode]);
    assert_eq!(view.default_agent, Some(AgentKind::Opencode));
    f.available.store(0, Ordering::SeqCst);
    let view = f.load();
    assert!(view.agents.is_empty());
    assert!(view.default_agent.is_none());
}

#[test]
fn real_external_preference_project_config_and_installation_edits_invalidate_confirmation() {
    for change in 0..5 {
        let f = fixture();
        let view = f.load();
        match change {
            0 => f.edit(|store| store.projects[0].preferred_agent = AgentKind::Pi),
            1 => f.edit(|store| store.projects[0].name = "Renamed".into()),
            2 => std::fs::write(f.repo().join("amf.json"), r#"{"future":true}"#).unwrap(),
            3 => std::fs::write(f.global(), r#"{"future":true}"#).unwrap(),
            _ => f.available.store(3, Ordering::SeqCst),
        }
        assert_eq!(
            f.save(&view.revision, AgentKind::Opencode)
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
        assert_ne!(
            f.db().load_store().unwrap().projects[0].preferred_agent,
            AgentKind::Opencode
        );
    }
}

#[test]
fn checkout_replacement_and_project_deletion_refuse_save() {
    let f = fixture();
    let view = f.load();
    std::fs::rename(f.repo(), f.root.path().join("old-repo")).unwrap();
    std::fs::create_dir(f.repo()).unwrap();
    assert_eq!(
        f.save(&view.revision, AgentKind::Pi).unwrap_err().kind,
        GuiErrorKind::Conflict
    );
    f.edit(|store| store.projects.clear());
    assert_eq!(
        f.save(&view.revision, AgentKind::Pi).unwrap_err().kind,
        GuiErrorKind::NotFound
    );
}

#[test]
fn unrelated_sibling_changes_survive_and_post_validation_cas_never_overwrites() {
    let f = fixture();
    let view = f.load();
    f.edit(|store| store.projects[0].features[0].nickname = Some("Earlier change".into()));
    f.save(&view.revision, AgentKind::Opencode).unwrap();
    assert_eq!(
        f.db().load_store().unwrap().projects[0].features[0]
            .nickname
            .as_deref(),
        Some("Earlier change")
    );
    let view = f.load();
    // The setter's selected-harness check runs after validation. Inject a
    // real external save there, immediately before persistence.
    f.conflict_at
        .store(f.calls.load(Ordering::SeqCst) + 1, Ordering::SeqCst);
    assert_eq!(
        f.save(&view.revision, AgentKind::Pi).unwrap_err().kind,
        GuiErrorKind::Conflict
    );
    let external = f.db().load_store().unwrap();
    assert_eq!(external.projects[0].preferred_agent, AgentKind::Opencode);
    assert_eq!(
        external.projects[0].features[0].nickname.as_deref(),
        Some("Sibling change")
    );
    assert_eq!(
        f.app().app_for_workflow().store.projects[0].preferred_agent,
        AgentKind::Opencode
    );
}

#[test]
fn malformed_config_and_failed_save_preserve_saved_preference() {
    let f = fixture();
    let view = f.load();
    std::fs::write(f.repo().join("amf.json"), "{invalid").unwrap();
    assert!(f.save(&view.revision, AgentKind::Pi).is_err());
    assert_eq!(
        std::fs::read_to_string(f.repo().join("amf.json")).unwrap(),
        "{invalid"
    );
    std::fs::remove_file(f.repo().join("amf.json")).unwrap();
    let view = f.load();
    let db = rusqlite::Connection::open(f.root.path().join("amf.db")).unwrap();
    db.execute_batch("CREATE TRIGGER reject_preference BEFORE INSERT ON projects BEGIN SELECT RAISE(FAIL, 'fixture save failure'); END;").unwrap();
    assert!(f.save(&view.revision, AgentKind::Pi).is_err());
    assert_eq!(
        f.app().app_for_workflow().store.projects[0].preferred_agent,
        AgentKind::Codex
    );
    assert_eq!(
        f.db().load_store().unwrap().projects[0].preferred_agent,
        AgentKind::Codex
    );
}

#[test]
fn probes_run_outside_the_lock_and_save_probes_each_harness_once() {
    let f = fixture();
    let view = f.load();
    assert_eq!(f.probes.load(Ordering::SeqCst), AgentKind::ALL.len());
    f.probes.store(0, Ordering::SeqCst);
    f.save(&view.revision, AgentKind::Opencode).unwrap();
    // One round for the offered choices, reused for the refreshed view, plus
    // the shared setter's check of the chosen harness.
    assert_eq!(f.probes.load(Ordering::SeqCst), AgentKind::ALL.len());
    assert_eq!(f.calls.load(Ordering::SeqCst), 1);
    assert!(!f.probed_while_locked.load(Ordering::SeqCst));
}

#[test]
fn an_active_workflow_blocks_saving_but_not_reading_choices() {
    let f = fixture();
    let view = f.load();
    f.app().app_for_workflow().mode = AppMode::DeletingProject("Project".into());
    // The creation forms read the same choices while a workflow is open.
    assert_eq!(f.try_load().unwrap().revision, view.revision);
    assert_eq!(
        f.save(&view.revision, AgentKind::Opencode)
            .unwrap_err()
            .kind,
        GuiErrorKind::Conflict
    );
    assert_eq!(
        f.db().load_store().unwrap().projects[0].preferred_agent,
        AgentKind::Codex
    );
}
