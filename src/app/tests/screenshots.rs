use super::support::store_with_feature;
use crate::app::App;
use crate::db::AmfDb;
use crate::gui_contract::GuiHandle;
use crate::gui_screenshots::{self, EvidenceSelection};
use crate::project::{ProjectStatus, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};

fn app(dir: &std::path::Path) -> App {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].repo = dir.to_path_buf();
    store.projects[0].features[0].workdir = dir.to_path_buf();
    store.projects[0].features[0].add_session(SessionKind::Claude);
    store.projects[0].features[0].add_session(SessionKind::Codex);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(AmfDb::open(&dir.join("db")).unwrap());
    app
}

#[test]
fn concurrent_sessions_have_distinct_guidance_and_repeated_setup_is_stable() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let sessions = &app.store.projects[0].features[0].sessions;
    let first = app
        .screenshot_owner(0, 0, &sessions[0].id)
        .unwrap()
        .unwrap();
    let second = app
        .screenshot_owner(0, 0, &sessions[1].id)
        .unwrap()
        .unwrap();
    assert_ne!(first.directory(), second.directory());
    assert_eq!(
        first,
        app.screenshot_owner(0, 0, &sessions[0].id)
            .unwrap()
            .unwrap()
    );
    let claude = app
        .screenshot_launch_args(&sessions[0].id, true, vec!["existing-flag".into()])
        .unwrap();
    let codex = app
        .screenshot_launch_args(&sessions[1].id, false, vec![])
        .unwrap();
    assert!(claude[2].contains(&first.scope_id));
    assert!(codex[1].contains(&second.scope_id));
    assert!(claude[2].contains("only when the user explicitly requests visual validation"));
    assert!(!codex[1].contains(&first.scope_id));
}

#[test]
fn completed_cleanup_invalidates_inflight_results_and_allows_retry() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let id = app.store.projects[0].features[0].sessions[0].id.clone();
    let owner = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(3, 2))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let metadata = include_str!("fixtures/screenshot-claude.json")
        .replace("{{scope_id}}", &owner.scope_id)
        .replace("{{sha256}}", &crate::screenshot_evidence::hash(&bytes));
    std::fs::write(owner.directory().join("claude-review.png"), bytes).unwrap();
    std::fs::write(owner.directory().join("claude-review.json"), metadata).unwrap();
    let mut gui = GuiHandle::from_app(app);
    let read = gui_screenshots::plan(&mut gui, EvidenceSelection::default()).unwrap();
    let before_cleanup = read.run();
    let item = &before_cleanup.items[0];
    let image_read = gui_screenshots::plan_image(
        &gui,
        owner.scope_id.clone(),
        item.image_id.clone(),
        item.sha256.clone(),
        true,
    )
    .unwrap();
    let image = image_read.run().unwrap();
    assert_eq!(image.width, 3);
    gui_screenshots::cleanup(&mut gui, &owner.scope_id).unwrap();
    assert!(gui_screenshots::finish_image(&gui, &image_read).is_err());
    let listing = gui_screenshots::finish(&mut gui, before_cleanup).unwrap();
    assert!(listing.items.is_empty());
    assert!(listing.owners.is_empty());
    gui_screenshots::cleanup(&mut gui, &owner.scope_id).unwrap();
    assert!(!owner.directory().exists());
}

#[test]
fn root_evidence_is_retained_when_records_disappear_and_worktree_evidence_is_retired_after_success()
{
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    let id = app.store.projects[0].features[0].sessions[0].id.clone();
    let root = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
    app.store.projects[0].features[0].is_worktree = true;
    let second_id = app.store.projects[0].features[0].sessions[1].id.clone();
    let tree = app.screenshot_owner(0, 0, &second_id).unwrap().unwrap();
    app.store.projects.clear();
    app.screenshot_worktree_deleted(dir.path()).unwrap();
    let db = app.db.as_ref().unwrap();
    assert!(db.evidence_scope_active(&root.scope_id).unwrap());
    assert!(!db.evidence_scope_active(&tree.scope_id).unwrap());
    let mut gui = GuiHandle::from_app(app);
    let read = gui_screenshots::plan(&mut gui, EvidenceSelection::default()).unwrap();
    assert_eq!(read.run().owners.len(), 1);
    gui_screenshots::cleanup(&mut gui, &root.scope_id).unwrap();
}

#[test]
fn watcher_observes_writes_after_registration_with_bounded_synchronization() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app(dir.path());
    let id = app.store.projects[0].features[0].sessions[0].id.clone();
    let owner = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
    app.evidence_work.watch(std::slice::from_ref(&owner));
    app.evidence_work.changed();
    std::fs::write(owner.directory().join("writing.png"), b"incomplete").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !app.evidence_work.changed() {
        assert!(
            std::time::Instant::now() < deadline,
            "watcher did not report evidence write"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let listing = crate::screenshot_evidence::scan(vec![owner]);
    assert!(listing.items.is_empty());
    assert_eq!(listing.issues.len(), 1);
}

#[test]
fn cleanup_partial_failure_retires_scope_and_retry_never_follows_a_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let id = app.store.projects[0].features[0].sessions[0].id.clone();
    let owner = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
    let original = dir.path().join("moved-scope");
    std::fs::rename(owner.directory(), &original).unwrap();
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("keep.txt"), "keep").unwrap();
    std::os::unix::fs::symlink(&outside, owner.directory()).unwrap();
    let mut gui = GuiHandle::from_app(app);
    assert!(gui_screenshots::cleanup(&mut gui, &owner.scope_id).is_err());
    assert_eq!(
        std::fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(gui_screenshots::cleanup_scopes(&gui).unwrap()[0].1);
    std::fs::remove_file(owner.directory()).unwrap();
    std::fs::rename(original, owner.directory()).unwrap();
    gui_screenshots::cleanup(&mut gui, &owner.scope_id).unwrap();
    assert!(!owner.directory().exists());
    assert!(outside.join("keep.txt").exists());
}

#[test]
fn successful_and_failed_foreground_deletion_have_different_evidence_lifetimes() {
    use crate::app::{AppMode, DeleteStage, DeletingFeatureState};
    for failed in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.store.projects[0].features[0].is_worktree = true;
        let id = app.store.projects[0].features[0].sessions[0].id.clone();
        let owner = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
        app.save().unwrap();
        app.mode = AppMode::DeletingFeatureInProgress(DeletingFeatureState {
            project_name: "my-project".into(),
            feature_name: "my-feat".into(),
            tmux_session: "amf-my-feat".into(),
            is_worktree: true,
            repo: dir.path().to_path_buf(),
            workdir: dir.path().to_path_buf(),
            stage: DeleteStage::Completed,
            child: None,
            output: String::new(),
            output_rx: None,
            error: failed.then(|| "worktree deletion failed".into()),
        });
        app.complete_deleting_feature().unwrap();
        assert_eq!(
            app.db
                .as_ref()
                .unwrap()
                .evidence_scope_active(&owner.scope_id)
                .unwrap(),
            failed
        );
    }
}

#[test]
fn initial_read_discovers_a_completion_published_after_watch_registration_and_reopens_it() {
    use crate::screenshot_evidence::{Completion, hash};
    use std::io::Cursor;
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let id = app.store.projects[0].features[0].sessions[0].id.clone();
    let owner = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
    let mut gui = GuiHandle::from_app(app);
    let read = gui_screenshots::plan(&mut gui, EvidenceSelection::default()).unwrap();
    std::fs::write(owner.directory().join("ready.png"), b"unfinished").unwrap();
    assert!(read.run().items.is_empty());
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(3, 2))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    std::fs::write(owner.directory().join("ready.png"), &bytes).unwrap();
    let metadata = Completion {
        version: 1,
        scope_id: owner.scope_id.clone(),
        image_id: "ready".into(),
        file: "ready.png".into(),
        sha256: hash(&bytes),
        caption: "Ready".into(),
        captured_at: chrono::Utc::now(),
    };
    std::fs::write(
        owner.directory().join("ready.json"),
        serde_json::to_vec(&metadata).unwrap(),
    )
    .unwrap();
    for _ in 0..3 {
        let read = gui_screenshots::plan(&mut gui, EvidenceSelection::default()).unwrap();
        let listing = gui_screenshots::finish(&mut gui, read.run()).unwrap();
        assert_eq!(listing.items.len(), 1);
        assert_eq!(listing.items[0].owner.session_id, id);
    }
}

#[test]
fn representative_claude_and_codex_completion_outputs_keep_producing_attribution() {
    use std::io::Cursor;
    let dir = tempfile::tempdir().unwrap();
    let app = app(dir.path());
    let sessions = &app.store.projects[0].features[0].sessions;
    for (session, fixture, color) in [
        (
            &sessions[0],
            include_str!("fixtures/screenshot-claude.json"),
            30,
        ),
        (
            &sessions[1],
            include_str!("fixtures/screenshot-codex.json"),
            90,
        ),
    ] {
        let owner = app.screenshot_owner(0, 0, &session.id).unwrap().unwrap();
        let mut image = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            4,
            3,
            image::Rgb([color, 0, 0]),
        ))
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
        let image = image.into_inner();
        let metadata: crate::screenshot_evidence::Completion = serde_json::from_str(
            &fixture
                .replace("{{scope_id}}", &owner.scope_id)
                .replace("{{sha256}}", &crate::screenshot_evidence::hash(&image)),
        )
        .unwrap();
        std::fs::write(owner.directory().join(&metadata.file), image).unwrap();
        std::fs::write(
            owner
                .directory()
                .join(format!("{}.json", metadata.image_id)),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        let listing = crate::screenshot_evidence::scan(vec![owner.clone()]);
        assert_eq!(listing.items.len(), 1);
        assert_eq!(listing.items[0].owner.session_id, session.id);
        let thumbnail = crate::screenshot_evidence::load_image(
            &owner,
            &metadata.image_id,
            &metadata.sha256,
            true,
        )
        .unwrap();
        assert_eq!((thumbnail.width, thumbnail.height), (4, 3));
    }
}

#[test]
fn project_deletion_retires_successful_worktree_scopes_and_retains_failed_and_root_scopes() {
    for (is_worktree, failed) in [(true, false), (true, true), (false, false)] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app(dir.path());
        app.store.projects[0].features[0].is_worktree = is_worktree;
        let id = app.store.projects[0].features[0].sessions[0].id.clone();
        let owner = app.screenshot_owner(0, 0, &id).unwrap().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_kill_session().times(1).returning(|_| Ok(()));
        app.tmux = Box::new(tmux);
        let mut worktree = MockWorktreeOps::new();
        if is_worktree {
            worktree.expect_remove().times(1).returning(move |_, _| {
                if failed {
                    Err(anyhow::anyhow!("failed worktree removal"))
                } else {
                    Ok(())
                }
            });
        }
        app.worktree = Box::new(worktree);
        app.save().unwrap();
        app.mode = crate::app::AppMode::DeletingProject(app.store.projects[0].name.clone());
        assert_eq!(app.delete_project().is_err(), failed);
        assert_eq!(
            app.db
                .as_ref()
                .unwrap()
                .evidence_scope_active(&owner.scope_id)
                .unwrap(),
            failed || !is_worktree
        );
        assert_eq!(app.store.projects.is_empty(), !failed);
    }
}
