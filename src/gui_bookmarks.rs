//! Shared bookmark slots with stable identities and no session launch on navigation.
use crate::gui_contract::{GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::project::SessionBookmark;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct BookmarkRow {
    pub slot: usize,
    pub target: SessionBookmark,
    pub label: String,
    pub stale: bool,
}

pub fn load(gui: &mut GuiHandle) -> GuiResult<Vec<BookmarkRow>> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    Ok(app
        .store
        .session_bookmarks
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let resolved = app.resolve_bookmark_indices(target);
            let label = resolved
                .map(|(pi, fi, si)| {
                    let project = &app.store.projects[pi];
                    let feature = &project.features[fi];
                    format!(
                        "{} / {} / {}",
                        project.name, feature.name, feature.sessions[si].label
                    )
                })
                .unwrap_or_else(|| "[stale]".into());
            BookmarkRow {
                slot: index + 1,
                target: target.clone(),
                label,
                stale: resolved.is_none(),
            }
        })
        .collect())
}

fn bookmark(target: &SessionTarget) -> SessionBookmark {
    SessionBookmark {
        project_id: target.project_id.clone(),
        feature_id: target.feature_id.clone(),
        session_id: target.session_id.clone(),
    }
}

// The shared save adopts the winning store on optimistic conflicts. On other
// I/O failures its baseline stays unchanged, so roll back the unsaved edit.
fn mutate(
    app: &mut crate::app::App,
    action: impl FnOnce(&mut crate::app::App) -> anyhow::Result<()>,
) -> GuiResult<()> {
    let previous = app.store.session_bookmarks.clone();
    let version = app.store_version;
    if let Err(error) = action(app) {
        if app.store_version == version {
            app.store.session_bookmarks = previous;
        }
        return Err(GuiError::from(error));
    }
    Ok(())
}

pub fn add(gui: &mut GuiHandle, target: &SessionTarget) -> GuiResult<Vec<BookmarkRow>> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let (pi, fi, si) = app
        .resolve_bookmark_indices(&bookmark(target))
        .ok_or_else(|| GuiError::conflict("Session no longer exists. Refresh bookmarks."))?;
    mutate(app, |app| app.bookmark_session_indices(pi, fi, si))?;
    load(gui)
}

pub fn remove(gui: &mut GuiHandle, target: &SessionTarget) -> GuiResult<Vec<BookmarkRow>> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    if let Some(index) = app
        .store
        .session_bookmarks
        .iter()
        .position(|row| *row == bookmark(target))
    {
        // Stable identity, never the slot number from an older picker load.
        mutate(app, |app| app.remove_bookmark_slot(index + 1))?;
    }
    load(gui)
}

pub fn resolve(gui: &mut GuiHandle, target: SessionTarget) -> GuiResult<SessionTarget> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let row = bookmark(&target);
    if !app.store.session_bookmarks.contains(&row) {
        return Err(GuiError::conflict(
            "Bookmark was removed. Refresh bookmarks.",
        ));
    }
    if app.resolve_bookmark_indices(&row).is_none() {
        remove(gui, &target)?;
        return Err(GuiError::conflict(
            "Stale bookmark removed: session no longer exists.",
        ));
    }
    // Unlike the TUI's enter_view, GUI navigation never starts stopped agents.
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppMode};
    use crate::db::AmfDb;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, SessionKind, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    fn fixture() -> (tempfile::TempDir, GuiHandle, Vec<SessionTarget>) {
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
        let targets = (0..10)
            .map(|index| {
                let session = feature.add_session(SessionKind::Terminal);
                // Opposite lexical order proves loading preserves slot insertion order.
                session.id = format!("session-{}", 10 - index);
                let session_id = session.id.clone();
                SessionTarget {
                    project_id: project.id.clone(),
                    feature_id: feature.id.clone(),
                    session_id,
                }
            })
            .collect();
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let db = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(db);
        app.store_version = Some(version);
        (dir, GuiHandle::from_app(app), targets)
    }

    #[test]
    fn shared_slots_deduplicate_evict_oldest_and_roundtrip_in_order() {
        let (dir, mut gui, targets) = fixture();
        for target in &targets[..9] {
            add(&mut gui, target).unwrap();
        }
        assert_eq!(add(&mut gui, &targets[0]).unwrap().len(), 9);
        let rows = add(&mut gui, &targets[9]).unwrap();
        assert_eq!(rows[0].target, bookmark(&targets[1]));
        assert_eq!(rows[8].slot, 9);
        let loaded = AmfDb::open(&dir.path().join("amf.db"))
            .unwrap()
            .load_store()
            .unwrap();
        assert_eq!(
            loaded.session_bookmarks,
            targets[1..].iter().map(bookmark).collect::<Vec<_>>()
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
    }

    #[test]
    fn external_slot_shift_cannot_remove_wrong_bookmark_or_launch_an_agent() {
        let (dir, mut gui, targets) = fixture();
        add(&mut gui, &targets[0]).unwrap();
        add(&mut gui, &targets[1]).unwrap();
        let writer = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = writer.load_store().unwrap();
        store.session_bookmarks.remove(0);
        writer.save_store(&store).unwrap();
        let rows = remove(&mut gui, &targets[0]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, bookmark(&targets[1]));
        assert_eq!(
            resolve(&mut gui, targets[1].clone()).unwrap().session_id,
            targets[1].session_id
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
    }

    #[test]
    fn stale_target_is_pruned_and_deleted_session_cannot_be_added() {
        let (dir, mut gui, targets) = fixture();
        add(&mut gui, &targets[0]).unwrap();
        let writer = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = writer.load_store().unwrap();
        store.projects[0].features[0].sessions.remove(0);
        writer.save_store(&store).unwrap();
        assert!(load(&mut gui).unwrap()[0].stale);
        assert!(
            resolve(&mut gui, targets[0].clone())
                .unwrap_err()
                .message
                .contains("Stale bookmark removed")
        );
        assert!(writer.load_store().unwrap().session_bookmarks.is_empty());
        assert!(add(&mut gui, &targets[0]).is_err());
    }
    #[test]
    fn failed_database_write_cannot_leave_phantom_bookmarks() {
        let (dir, mut gui, targets) = fixture();
        add(&mut gui, &targets[0]).unwrap();
        gui.app_for_workflow().db =
            Some(AmfDb::open_read_only(&dir.path().join("amf.db")).unwrap());
        assert!(add(&mut gui, &targets[1]).is_err());
        assert_eq!(load(&mut gui).unwrap().len(), 1);
        assert!(remove(&mut gui, &targets[0]).is_err());
        assert_eq!(load(&mut gui).unwrap()[0].target, bookmark(&targets[0]));
    }
    #[test]
    fn concurrent_save_conflict_preserves_winning_shared_bookmark() {
        let (dir, mut gui, targets) = fixture();
        let writer = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let result = mutate(gui.app_for_workflow(), |app| {
            let mut store = writer.load_store().unwrap();
            store.session_bookmarks.push(bookmark(&targets[1]));
            writer.save_store(&store).unwrap();
            app.bookmark_session_indices(0, 0, 0)
        });
        assert!(result.is_err());
        assert_eq!(load(&mut gui).unwrap()[0].target, bookmark(&targets[1]));
        assert_eq!(
            writer.load_store().unwrap().session_bookmarks,
            vec![bookmark(&targets[1])]
        );
    }
}
