use super::*;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn row(language: HighlightLanguage, state: ParserState, revision: &str) -> SyntaxLanguageRow {
    SyntaxLanguageRow {
        key: language.package_key().into(),
        title: language.picker_title().into(),
        state,
        revision: revision.into(),
    }
}
fn request(action: SyntaxOperationAction, revision: &str) -> SyntaxLanguageRequest {
    SyntaxLanguageRequest {
        language: "json".into(),
        action,
        revision: revision.into(),
    }
}
fn wait_done(state: &InstallState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.lock().unwrap().running {
        assert!(Instant::now() < deadline, "worker did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn catalog_uses_shared_languages_and_never_reads_during_a_worker() {
    let state = InstallState::default();
    let refreshed = std::cell::Cell::new(false);
    let view = read_languages(
        &state,
        || refreshed.set(true),
        |language| Ok(row(language, ParserState::Available, "initial")),
    )
    .unwrap();
    assert!(refreshed.get());
    assert_eq!(
        view.languages
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>(),
        HighlightLanguage::ALL
            .iter()
            .map(|language| language.package_key())
            .collect::<Vec<_>>()
    );
    state.lock().unwrap().running = true;
    let busy = read_languages(
        &state,
        || panic!("must not reload changing libraries"),
        |_| panic!("must not load changing libraries"),
    )
    .unwrap();
    assert!(busy.languages.is_empty());
    assert!(busy.operation.running);
}

#[test]
fn actions_and_per_file_installer_share_the_busy_guard() {
    for disk_state in [
        ParserState::Available,
        ParserState::Installed,
        ParserState::Broken,
    ] {
        for action in [
            SyntaxOperationAction::Install,
            SyntaxOperationAction::Repair,
            SyntaxOperationAction::Uninstall,
        ] {
            let state = InstallState::default();
            let (release, gate) = mpsc::channel();
            let started = apply_with(
                &state,
                request(action, "current"),
                |language| Ok(row(language, disk_state, "current")),
                move |_, _| {
                    gate.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok("done".into())
                },
            );
            let valid = if action == SyntaxOperationAction::Install {
                disk_state == ParserState::Available
            } else {
                disk_state != ParserState::Available
            };
            assert_eq!(started.is_ok(), valid, "{disk_state:?}: {action:?}");
            if valid {
                let started = started.unwrap();
                assert_eq!(started.action, Some(action));
                assert_eq!(started.operation_id, 1);
                assert!(
                    super::super::start_install(
                        &state,
                        "rust",
                        |_| HighlightInstallState::Available,
                        |_, _| panic!("per-file installer must not run concurrently")
                    )
                    .is_err()
                );
                release.send(()).unwrap();
                wait_done(&state);
                assert_eq!(state.lock().unwrap().completed, 1);
            } else {
                assert_eq!(state.lock().unwrap().operation_id, 0);
            }
        }
    }
}

#[test]
fn stale_or_unknown_requests_never_mutate() {
    let state = InstallState::default();
    let error = apply_with(
        &state,
        request(SyntaxOperationAction::Uninstall, "old"),
        |language| Ok(row(language, ParserState::Installed, "new")),
        |_, _| panic!("stale request must not mutate"),
    )
    .unwrap_err();
    assert!(error.message.contains("Parser files changed"));
    let mut unknown = request(SyntaxOperationAction::Install, "current");
    unknown.language = "../outside".into();
    assert!(
        apply_with(
            &state,
            unknown,
            |_| panic!("unknown path must not be read"),
            |_, _| panic!("unknown path must not mutate")
        )
        .is_err()
    );
    assert_eq!(state.lock().unwrap().operation_id, 0);
}

fn fixture(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let source = root.join("tree-sitter/sources/json");
    let library = root.join("tree-sitter/parsers/json.so");
    std::fs::create_dir_all(source.join("queries")).unwrap();
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(source.join("queries/highlights.scm"), "query-a").unwrap();
    std::fs::write(&library, "library-a").unwrap();
    (source, library)
}

#[test]
fn fingerprint_binds_bytes_with_preserved_size_and_time_and_root_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (source, library) = fixture(temp.path());
    let revision = || {
        disk_revision(
            HighlightLanguage::Json,
            ParserState::Installed,
            &source,
            &library,
        )
        .unwrap()
    };
    let initial = revision();
    let modified = std::fs::metadata(&library).unwrap().modified().unwrap();
    std::fs::write(&library, "library-b").unwrap();
    std::fs::File::open(&library)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let replaced_library = revision();
    assert_ne!(initial, replaced_library);
    let query = source.join("queries/highlights.scm");
    let modified = std::fs::metadata(&query).unwrap().modified().unwrap();
    std::fs::write(&query, "query-b").unwrap();
    std::fs::File::open(query)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_ne!(replaced_library, revision());
    std::fs::rename(
        temp.path().join("tree-sitter"),
        temp.path().join("old-tree-sitter"),
    )
    .unwrap();
    fixture(temp.path());
    assert_ne!(initial, revision());
}

#[test]
fn fingerprint_binds_all_source_file_bytes_with_preserved_size_and_time() {
    let temp = tempfile::tempdir().unwrap();
    let (source, library) = fixture(temp.path());
    let other = source.join("src/parser.c");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "source-a").unwrap();
    let revision = || {
        disk_revision(
            HighlightLanguage::Json,
            ParserState::Installed,
            &source,
            &library,
        )
        .unwrap()
    };
    let initial = revision();
    let modified = std::fs::metadata(&other).unwrap().modified().unwrap();
    std::fs::write(&other, "source-b").unwrap();
    std::fs::File::open(&other)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert_ne!(initial, revision());
}

#[test]
fn real_external_artifact_edit_is_rechecked_before_destructive_action() {
    let temp = tempfile::tempdir().unwrap();
    let (source, library) = fixture(temp.path());
    let before = disk_revision(
        HighlightLanguage::Json,
        ParserState::Installed,
        &source,
        &library,
    )
    .unwrap();
    std::fs::write(&library, "external replacement").unwrap();
    let state = InstallState::default();
    let error = apply_with(
        &state,
        request(SyntaxOperationAction::Uninstall, &before),
        |language| {
            Ok(row(
                language,
                ParserState::Installed,
                &disk_revision(language, ParserState::Installed, &source, &library)?,
            ))
        },
        |_, _| panic!("external replacement must survive stale uninstall"),
    )
    .unwrap_err();
    assert!(error.message.contains("Parser files changed"));
    assert_eq!(
        std::fs::read_to_string(library).unwrap(),
        "external replacement"
    );
    assert!(source.exists());
}
