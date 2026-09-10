use super::*;
use std::fs;

fn origin(root: &Path) -> ConsultationOrigin {
    ConsultationOrigin {
        project_id: "p".into(),
        feature_id: "f".into(),
        session_id: "s".into(),
        launch_generation: "g".into(),
        provider_session_id: None,
        workdir: root.to_path_buf(),
        repository_identity: "repo".into(),
        tmux_server_identity: "server".into(),
        tmux_session_id: "tmux".into(),
        tmux_window_id: "window".into(),
        tmux_pane_id: "pane".into(),
    }
}

#[test]
fn captures_deterministic_hashes_and_omits_only_bounded_ranges() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source.rs"), b"0123456789abcdef").unwrap();
    let origin = origin(dir.path());
    let collector = EvidenceCollector::new(&origin)
        .unwrap()
        .with_limits(8, 8)
        .unwrap();
    let packet = collector
        .capture(
            &origin,
            &[EvidenceSelection {
                id: "source".into(),
                kind: EvidenceKind::Source,
                relative_path: "source.rs".into(),
                ranges: vec![],
                required: false,
            }],
        )
        .unwrap();
    let item = &packet.items[0];
    assert_eq!(item.included_bytes, 8);
    assert_eq!(
        item.omitted_ranges,
        vec![EvidenceRange { start: 8, end: 16 }]
    );
    assert!(item.retrievable);
    let retrieved = collector
        .retrieve_omission(&origin, &packet, "source", item.omitted_ranges[0].clone())
        .unwrap();
    assert_eq!(retrieved.text, "89abcdef");
    assert_eq!(retrieved.source_hash, item.source_hash);
}

#[test]
fn origin_and_source_changes_block_retrieval() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source.txt"), b"stable").unwrap();
    let origin = origin(dir.path());
    let collector = EvidenceCollector::new(&origin)
        .unwrap()
        .with_limits(2, 2)
        .unwrap();
    let packet = collector
        .capture(
            &origin,
            &[EvidenceSelection {
                id: "x".into(),
                kind: EvidenceKind::Source,
                relative_path: "source.txt".into(),
                ranges: vec![],
                required: false,
            }],
        )
        .unwrap();
    let mut wrong = origin.clone();
    wrong.launch_generation = "new".into();
    assert!(
        collector
            .retrieve_omission(
                &wrong,
                &packet,
                "x",
                packet.items[0].omitted_ranges[0].clone()
            )
            .is_err()
    );
    fs::write(dir.path().join("source.txt"), b"changed").unwrap();
    assert!(
        collector
            .retrieve_omission(
                &origin,
                &packet,
                "x",
                packet.items[0].omitted_ranges[0].clone()
            )
            .is_err()
    );
}

#[test]
fn traversal_and_escaping_symlink_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret"), b"secret").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret"), dir.path().join("link")).unwrap();
    let origin = origin(dir.path());
    let collector = EvidenceCollector::new(&origin).unwrap();
    for path in [PathBuf::from("../secret"), PathBuf::from("link")] {
        assert!(
            collector
                .capture(
                    &origin,
                    &[EvidenceSelection {
                        id: "bad".into(),
                        kind: EvidenceKind::Source,
                        relative_path: path,
                        ranges: vec![],
                        required: false,
                    }]
                )
                .is_err()
        );
    }
}

#[test]
fn required_evidence_cannot_be_silently_truncated() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("source.txt"), b"0123456789").unwrap();
    let origin = origin(dir.path());
    let collector = EvidenceCollector::new(&origin)
        .unwrap()
        .with_limits(2, 2)
        .unwrap();
    assert!(
        collector
            .capture(
                &origin,
                &[EvidenceSelection {
                    id: "required".into(),
                    kind: EvidenceKind::Source,
                    relative_path: "source.txt".into(),
                    ranges: vec![],
                    required: true,
                }]
            )
            .is_err()
    );
}

#[test]
fn required_items_are_allocated_before_optional_items() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("optional.txt"), b"optional").unwrap();
    fs::write(dir.path().join("required.txt"), b"required").unwrap();
    let origin = origin(dir.path());
    let collector = EvidenceCollector::new(&origin)
        .unwrap()
        .with_limits(8, 8)
        .unwrap();
    let packet = collector
        .capture(
            &origin,
            &[
                EvidenceSelection {
                    id: "optional".into(),
                    kind: EvidenceKind::Source,
                    relative_path: "optional.txt".into(),
                    ranges: vec![],
                    required: false,
                },
                EvidenceSelection {
                    id: "required".into(),
                    kind: EvidenceKind::Source,
                    relative_path: "required.txt".into(),
                    ranges: vec![],
                    required: true,
                },
            ],
        )
        .unwrap();
    assert_eq!(packet.items[0].id, "required");
    assert_eq!(packet.items[0].included_bytes, 8);
    assert_eq!(packet.items[1].included_bytes, 0);
}
