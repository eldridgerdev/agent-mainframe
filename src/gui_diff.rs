//! Read-only GUI diffs use the same Git snapshots and context expansion as
//! the TUI. Requests resolve stable feature IDs afresh; no workflow mode or
//! agent session is needed to browse changes.
use serde::{Deserialize, Serialize};

use crate::diff::{self, DiffFileStatus, DiffLineKind};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::gui_syntax::{DiffHighlights, HighlightBudget, SyntaxInfo, SyntaxLine};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffContext {
    #[default]
    Standard,
    Expanded,
    Full,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiffOptions {
    pub commit: Option<String>,
    pub base_ref: Option<String>,
    #[serde(default)]
    pub ignore_whitespace: bool,
    #[serde(default)]
    pub context: DiffContext,
}

#[derive(Debug, Serialize)]
pub struct DiffCommitView {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
}

#[derive(Debug, Serialize)]
pub struct DiffLineView {
    pub kind: &'static str,
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    /// Token spans covering `text`, coloured with whole-file context; `None`
    /// renders the text plain (see `DiffFileView::syntax` for why).
    pub syntax: SyntaxLine,
    /// Word-level changed runs of a paired removed/added line, as
    /// `[start, end)` offsets in **UTF-16 code units** of `text` (prefix
    /// included, like `syntax`), so the frontend can slice a JS string with
    /// them directly. Sorted, disjoint, never splitting a surrogate pair.
    /// Empty (and omitted) for unpaired lines, dissimilar pairs, and anything
    /// past [`WordBudget`]: the line-level colouring is then all there is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub emphasis: Vec<[u32; 2]>,
}

#[derive(Debug, Serialize)]
pub struct DiffHunkView {
    pub header: String,
    pub lines: Vec<DiffLineView>,
}

#[derive(Debug, Serialize)]
pub struct DiffFileView {
    pub path: String,
    pub old_path: Option<String>,
    pub status: &'static str,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    pub hunks: Vec<DiffHunkView>,
    /// Retains mode changes and rename metadata even when there are no hunks.
    pub patch: String,
    /// The detected language and whether its lines carry highlight spans.
    pub syntax: SyntaxInfo,
    /// AMF wrote this change itself (`amf_managed::is_amf_generated_change`,
    /// the rule behind the TUI's "AMF files" group); the GUI groups on it.
    pub amf_generated: bool,
}

#[derive(Debug, Serialize)]
pub struct DiffView {
    pub target: FeatureTarget,
    pub feature_name: String,
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
    pub commit: Option<String>,
    pub commits: Vec<DiffCommitView>,
    pub commits_error: Option<String>,
    pub files: Vec<DiffFileView>,
    pub total_additions: usize,
    pub total_deletions: usize,
}

pub fn load(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    options: DiffOptions,
) -> GuiResult<DiffView> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    let project = &app.store.projects[pi];
    if !project.is_git {
        return Err(GuiError::conflict("Diffs require a Git checkout"));
    }
    let feature = &project.features[fi];
    let (commits, commits_error) = match diff::list_diff_commits(&feature.workdir) {
        Ok(commits) => (commits, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let snapshot = if let Some(hash) = &options.commit {
        // Accept only the picker entry's full object ID. A force-push or branch
        // switch cannot turn an old selection into an unrelated revision.
        let commit = commits
            .iter()
            .find(|commit| &commit.hash == hash)
            .ok_or_else(|| {
                GuiError::conflict(
                    "That commit is no longer in the feature's history; choose current changes",
                )
            })?;
        diff::load_commit_snapshot(&feature.workdir, &commit.hash, options.ignore_whitespace)?
    } else {
        diff::load_snapshot(
            &feature.workdir,
            options.base_ref.as_deref(),
            options.ignore_whitespace,
        )?
    };
    let context = match options.context {
        DiffContext::Standard => 3,
        DiffContext::Expanded => 10,
        DiffContext::Full => usize::MAX,
    };
    let mut budget = HighlightBudget::view();
    let mut words = WordBudget::view();
    let files = snapshot
        .files
        .into_iter()
        .map(|file| build_file_view(file, context, &mut budget, Some(&mut words)))
        .collect();
    Ok(DiffView {
        target,
        feature_name: feature.name.clone(),
        branch: snapshot.branch,
        base_ref: snapshot.base_ref,
        base_commit: snapshot.base_commit,
        commit: options.commit,
        commits: commits
            .into_iter()
            .map(|c| DiffCommitView {
                hash: c.hash,
                short_hash: c.short_hash,
                subject: c.subject,
            })
            .collect(),
        commits_error,
        files,
        total_additions: snapshot.total_additions,
        total_deletions: snapshot.total_deletions,
    })
}

pub(crate) fn file_view(file: crate::diff::DiffFile, context: usize) -> DiffFileView {
    file_view_budgeted(file, context, &mut HighlightBudget::view())
}

/// [`file_view`] for one file of a larger projection that shares `budget`.
pub(crate) fn file_view_budgeted(
    file: crate::diff::DiffFile,
    context: usize,
    budget: &mut HighlightBudget,
) -> DiffFileView {
    build_file_view(file, context, budget, None)
}

/// Bounds on the word-level emphasis sent with one Changes view, so a huge or
/// minified diff cannot bloat the IPC payload or stall the load. Exceeding any
/// limit only drops emphasis for the affected line, hunk or later files; it is
/// never an error.
#[derive(Debug, Clone)]
pub(crate) struct WordBudget {
    /// A hunk with more lines than this gets no emphasis.
    pub max_hunk_lines: usize,
    /// A line longer than this (bytes) gets none.
    pub max_line_bytes: usize,
    /// A line with more changed runs than this gets none.
    pub max_ranges_per_line: usize,
    /// Changed runs left for the whole view.
    pub ranges_left: usize,
    /// Token diffs left for the whole view.
    pub pairs_left: usize,
}

impl WordBudget {
    pub(crate) fn view() -> Self {
        Self {
            max_hunk_lines: 2_000,
            max_line_bytes: 2_000,
            max_ranges_per_line: 64,
            ranges_left: 20_000,
            pairs_left: 5_000,
        }
    }
}

/// `[start, end)` byte ranges of the line's content to UTF-16 ranges of the
/// whole `text`. `prefix` is the byte length of the stripped diff marker.
fn utf16_ranges(text: &str, prefix: usize, ranges: &[std::ops::Range<usize>]) -> Vec<[u32; 2]> {
    ranges
        .iter()
        .filter_map(|r| {
            let (s, e) = (r.start + prefix, r.end + prefix);
            // Defensive: token ranges are char boundaries, but never panic.
            if !text.is_char_boundary(s) || !text.is_char_boundary(e) || s >= e {
                return None;
            }
            let start = text[..s].encode_utf16().count() as u32;
            let end = start + text[s..e].encode_utf16().count() as u32;
            Some([start, end])
        })
        .collect()
}

/// Per-line emphasis for a hunk under `budget`, indexed like `hunk.lines`.
fn hunk_emphasis(hunk: &diff::DiffHunk, budget: &mut WordBudget) -> Vec<Vec<[u32; 2]>> {
    let mut out = vec![Vec::new(); hunk.lines.len()];
    if hunk.lines.len() > budget.max_hunk_lines || budget.pairs_left == 0 {
        return out;
    }
    let (ranges, used) =
        crate::worddiff::hunk_intra_line_ranges_limited(&hunk.lines, budget.pairs_left);
    budget.pairs_left -= used;
    for (idx, found) in ranges.into_iter().enumerate() {
        let Some(found) = found else { continue };
        let line = &hunk.lines[idx];
        if line.text.len() > budget.max_line_bytes
            || found.len() > budget.max_ranges_per_line
            || found.len() > budget.ranges_left
        {
            continue;
        }
        let prefix = line.text.len() - crate::worddiff::line_content(&line.text).len();
        let converted = utf16_ranges(&line.text, prefix, &found);
        budget.ranges_left -= converted.len().min(budget.ranges_left);
        out[idx] = converted;
    }
    out
}

fn build_file_view(
    file: crate::diff::DiffFile,
    context: usize,
    budget: &mut HighlightBudget,
    mut words: Option<&mut WordBudget>,
) -> DiffFileView {
    let amf_generated = crate::amf_managed::is_amf_generated_change(&file);
    let highlights = DiffHighlights::for_file(&file, budget);
    let hunks = file.hunks_with_context(context).unwrap_or(file.hunks);
    let hunks = hunks
        .into_iter()
        .map(|hunk| {
            let locations = diff::line_locations_in_hunk(&hunk);
            let mut emphasis = match words.as_deref_mut() {
                Some(words) => hunk_emphasis(&hunk, words),
                None => vec![Vec::new(); hunk.lines.len()],
            }
            .into_iter();
            DiffHunkView {
                header: hunk.header,
                lines: hunk
                    .lines
                    .into_iter()
                    .zip(locations)
                    .map(|(line, location)| {
                        let old_line = location.and_then(|l| l.old_line);
                        let new_line = location.and_then(|l| l.new_line);
                        DiffLineView {
                            kind: match line.kind {
                                DiffLineKind::Context => "context",
                                DiffLineKind::Added => "added",
                                DiffLineKind::Removed => "removed",
                                DiffLineKind::NoNewlineMarker => "marker",
                            },
                            syntax: highlights.line(&line.kind, &line.text, old_line, new_line),
                            text: line.text,
                            old_line,
                            new_line,
                            emphasis: emphasis.next().unwrap_or_default(),
                        }
                    })
                    .collect(),
            }
        })
        .collect();
    DiffFileView {
        path: file.path,
        old_path: file.old_path,
        status: match file.status {
            DiffFileStatus::Added => "added",
            DiffFileStatus::Modified => "modified",
            DiffFileStatus::Deleted => "deleted",
            DiffFileStatus::Renamed => "renamed",
            DiffFileStatus::Copied => "copied",
            DiffFileStatus::TypeChanged => "type_changed",
            DiffFileStatus::Untracked => "untracked",
        },
        additions: file.additions,
        deletions: file.deletions,
        is_binary: file.is_binary,
        hunks,
        patch: file.patch,
        syntax: highlights.info,
        amf_generated,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::{App, AppMode};
    use crate::db::AmfDb;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::Path;
    use std::process::Command;

    pub(crate) fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    pub(crate) fn fixture() -> (tempfile::TempDir, GuiHandle, FeatureTarget) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        let content = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        std::fs::write(repo.join("code.txt"), &content).unwrap();
        std::fs::write(repo.join("old.txt"), "rename me\n").unwrap();
        std::fs::write(repo.join("delete.txt"), "delete me\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        git(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(
            repo.join("code.txt"),
            content.replace("line 8\n", "committed 🦀\n"),
        )
        .unwrap();
        git(&repo, &["add", "code.txt"]);
        git(&repo, &["commit", "-m", "feature commit"]);
        let mut project = Project::new("Demo".into(), repo.clone(), true, AgentKind::Claude);
        let mut feature = Feature::new_for_project(
            "Demo",
            "Feature".into(),
            "feature".into(),
            repo,
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        feature.status = ProjectStatus::Stopped;
        let target = FeatureTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let db = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_check_harness_available().returning(|_| Ok(()));
        tmux.expect_session_exists().return_const(false);
        tmux.expect_window_exists().return_const(false);
        tmux.expect_list_panes().returning(Vec::new);
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.db = Some(db);
        app.store_version = Some(version);
        (dir, GuiHandle::from_app(app), target)
    }

    fn pair_hunk(pairs: &[(&str, &str)]) -> diff::DiffHunk {
        let mut lines = Vec::new();
        for (old, _) in pairs {
            lines.push(diff::DiffLine {
                kind: DiffLineKind::Removed,
                text: format!("-{old}"),
            });
        }
        for (_, new) in pairs {
            lines.push(diff::DiffLine {
                kind: DiffLineKind::Added,
                text: format!("+{new}"),
            });
        }
        diff::DiffHunk {
            header: "@@ -1 +1 @@".into(),
            old_start: 1,
            old_lines: pairs.len(),
            new_start: 1,
            new_lines: pairs.len(),
            lines,
        }
    }

    /// Slice `text` the way the frontend does: by UTF-16 code units.
    fn slice16(text: &str, [s, e]: [u32; 2]) -> String {
        let units: Vec<u16> = text.encode_utf16().collect();
        String::from_utf16(&units[s as usize..e as usize]).unwrap()
    }

    #[test]
    fn emphasis_pairs_lines_and_offsets_include_the_prefix_in_utf16_units() {
        let hunk = pair_hunk(&[("let x = foo(1);", "let x = foo(2);")]);
        let out = hunk_emphasis(&hunk, &mut WordBudget::view());
        assert_eq!(out[0].len(), 1);
        assert_eq!(slice16(&hunk.lines[0].text, out[0][0]), "1");
        assert_eq!(slice16(&hunk.lines[1].text, out[1][0]), "2");
    }

    #[test]
    fn emphasis_ranges_are_utf16_not_bytes_for_non_ascii_emoji_and_combining_text() {
        for (old, new, want_old, want_new) in [
            ("名前 = \"🦀🦀\" + a", "名前 = \"🦀🦀\" + b", "a", "b"),
            ("café 🦀 value x", "café 🦀 value y", "x", "y"),
            ("let e\u{301}t = 1;", "let e\u{301}t = 2;", "1", "2"),
            (
                "héllo wörld ünï 🦀 old",
                "héllo wörld ünï 🦀 new",
                "old",
                "new",
            ),
        ] {
            let hunk = pair_hunk(&[(old, new)]);
            let out = hunk_emphasis(&hunk, &mut WordBudget::view());
            assert_eq!(out[0].len(), 1, "{old}");
            assert_eq!(slice16(&hunk.lines[0].text, out[0][0]), want_old);
            assert_eq!(slice16(&hunk.lines[1].text, out[1][0]), want_new);
        }
        // A changed emoji is two UTF-16 units wide and never split.
        let hunk = pair_hunk(&[("say hello 🦀 now", "say hello 🎉 now")]);
        let out = hunk_emphasis(&hunk, &mut WordBudget::view());
        let [s, e] = out[0][0];
        assert_eq!(e - s, 2);
        assert_eq!(slice16(&hunk.lines[0].text, out[0][0]), "🦀");
        assert_eq!(slice16(&hunk.lines[1].text, out[1][0]), "🎉");
    }

    #[test]
    fn emphasis_skips_unpaired_context_and_dissimilar_lines() {
        let mut hunk = pair_hunk(&[("value = 1;", "value = 2;")]);
        hunk.lines.push(diff::DiffLine {
            kind: DiffLineKind::Context,
            text: " ctx".into(),
        });
        hunk.lines.insert(
            0,
            diff::DiffLine {
                kind: DiffLineKind::Context,
                text: " head".into(),
            },
        );
        let out = hunk_emphasis(&hunk, &mut WordBudget::view());
        assert!(out[0].is_empty() && out[3].is_empty());
        assert!(!out[1].is_empty() && !out[2].is_empty());
        let rewrite = pair_hunk(&[("alpha beta gamma", "zzz yyy xxx www")]);
        assert!(hunk_emphasis(&rewrite, &mut WordBudget::view())[0].is_empty());
    }

    #[test]
    fn emphasis_budget_degrades_silently_to_no_ranges() {
        let pairs = [
            ("a = 1;", "a = 2;"),
            ("b = 1;", "b = 2;"),
            ("c = 1;", "c = 2;"),
        ];
        let hunk = pair_hunk(&pairs);
        // Hunk too big.
        let mut b = WordBudget::view();
        b.max_hunk_lines = 5;
        assert!(hunk_emphasis(&hunk, &mut b).iter().all(Vec::is_empty));
        // Line too long.
        let mut b = WordBudget::view();
        b.max_line_bytes = 3;
        assert!(hunk_emphasis(&hunk, &mut b).iter().all(Vec::is_empty));
        // Too many pairs: only the first is diffed.
        let mut b = WordBudget::view();
        b.pairs_left = 1;
        let out = hunk_emphasis(&hunk, &mut b);
        assert!(!out[0].is_empty() && !out[3].is_empty());
        assert!(out[1].is_empty() && out[2].is_empty() && out[4].is_empty());
        // Total ranges exhausted.
        let mut b = WordBudget::view();
        b.ranges_left = 1;
        let out = hunk_emphasis(&hunk, &mut b);
        assert_eq!(out.iter().map(Vec::len).sum::<usize>(), 1);
        assert_eq!(b.ranges_left, 0);
        // Too many runs on one line.
        let many_old = (0..10)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let many_new = (0..10)
            .map(|i| format!("x{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let hunk = pair_hunk(&[(many_old.as_str(), many_new.as_str())]);
        let mut b = WordBudget::view();
        b.max_ranges_per_line = 2;
        assert!(hunk_emphasis(&hunk, &mut b).iter().all(Vec::is_empty));
    }

    #[test]
    fn emphasis_is_omitted_from_json_when_empty_and_present_when_set() {
        let line = |emphasis: Vec<[u32; 2]>| DiffLineView {
            kind: "added",
            text: "+x".into(),
            old_line: None,
            new_line: Some(1),
            syntax: None,
            emphasis,
        };
        let plain = serde_json::to_value(line(Vec::new())).unwrap();
        assert!(plain.get("emphasis").is_none());
        let marked = serde_json::to_value(line(vec![[1, 2]])).unwrap();
        assert_eq!(marked["emphasis"], serde_json::json!([[1, 2]]));
    }

    #[test]
    fn load_emits_emphasis_for_changes_view_but_review_projection_does_not() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let path = repo.join("code.txt");
        let current = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, current.replace("line 9\n", "line 9 🦀 edited\n")).unwrap();
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        let code = view.files.iter().find(|f| f.path == "code.txt").unwrap();
        let added = code
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .find(|l| l.text == "+line 9 🦀 edited")
            .unwrap();
        assert_eq!(slice16(&added.text, added.emphasis[0]), " 🦀 edited");
        let plain = file_view(
            diff::load_snapshot(&repo, None, false)
                .unwrap()
                .files
                .into_iter()
                .find(|f| f.path == "code.txt")
                .unwrap(),
            3,
        );
        assert!(
            plain
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .all(|l| l.emphasis.is_empty())
        );
    }

    #[test]
    fn current_changes_include_commits_staged_unstaged_and_untracked_without_starting() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let path = repo.join("code.txt");
        let current = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, current.replace("line 9\n", "unstaged\n")).unwrap();
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("untracked.txt"), "new 🦀\n").unwrap();
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        assert_eq!(view.commits.len(), 1);
        let code = view.files.iter().find(|f| f.path == "code.txt").unwrap();
        assert!(
            code.hunks[0]
                .lines
                .iter()
                .any(|l| l.text == "+committed 🦀" && l.new_line == Some(8))
        );
        assert!(
            code.hunks[0]
                .lines
                .iter()
                .any(|l| l.text == "+unstaged" && l.new_line == Some(9))
        );
        assert!(
            view.files
                .iter()
                .any(|f| f.path == "staged.txt" && f.status == "added")
        );
        assert!(
            view.files
                .iter()
                .any(|f| f.path == "untracked.txt" && f.status == "untracked")
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        assert_eq!(
            gui.snapshot().projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }

    #[test]
    fn commit_scope_excludes_working_changes_and_refuses_stale_or_arbitrary_revisions() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let hash = git(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("untracked.txt"), "not committed\n").unwrap();
        let options = DiffOptions {
            commit: Some(hash.clone()),
            ..Default::default()
        };
        let view = load(&mut gui, target.clone(), options.clone()).unwrap();
        assert_eq!(view.files.len(), 1);
        assert_eq!(view.files[0].path, "code.txt");
        assert_eq!(view.commit.as_deref(), Some(hash.as_str()));
        assert_eq!(
            load(
                &mut gui,
                target.clone(),
                DiffOptions {
                    commit: Some("HEAD".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        git(&repo, &["reset", "--hard", "main"]);
        assert_eq!(
            load(&mut gui, target, options).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
    }

    #[test]
    fn context_expansion_keeps_source_coordinates_and_totals() {
        let (_dir, mut gui, target) = fixture();
        let standard = load(&mut gui, target.clone(), DiffOptions::default()).unwrap();
        let full = load(
            &mut gui,
            target,
            DiffOptions {
                context: DiffContext::Full,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(full.files[0].hunks[0].lines.len() > standard.files[0].hunks[0].lines.len());
        assert_eq!(full.total_additions, standard.total_additions);
        assert_eq!(full.total_deletions, standard.total_deletions);
        let last = full.files[0].hunks[0].lines.last().unwrap();
        assert_eq!((last.old_line, last.new_line), (Some(20), Some(20)));
    }

    #[test]
    fn refresh_and_whitespace_filter_use_current_git_contents() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        git(&repo, &["reset", "--hard", "main"]);
        assert!(
            load(&mut gui, target.clone(), DiffOptions::default())
                .unwrap()
                .files
                .is_empty()
        );
        let path = repo.join("code.txt");
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, content.replace("line 8\n", "line    8\n")).unwrap();
        assert_eq!(
            load(&mut gui, target.clone(), DiffOptions::default())
                .unwrap()
                .files
                .len(),
            1
        );
        assert!(
            load(
                &mut gui,
                target,
                DiffOptions {
                    ignore_whitespace: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .files
            .is_empty()
        );
    }

    #[test]
    fn rename_binary_deletion_and_no_newline_metadata_survive_the_contract() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        git(&repo, &["mv", "old.txt", "new.txt"]);
        git(&repo, &["rm", "delete.txt"]);
        std::fs::write(repo.join("binary.bin"), b"\0binary\0").unwrap();
        git(&repo, &["add", "binary.bin"]);
        std::fs::write(repo.join("nonewline.txt"), "no newline").unwrap();
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        let renamed = view.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
        assert_eq!(renamed.status, "renamed");
        assert!(renamed.patch.contains("rename from"));
        assert!(
            view.files
                .iter()
                .any(|f| f.path == "binary.bin" && f.is_binary)
        );
        let deleted = view.files.iter().find(|f| f.path == "delete.txt").unwrap();
        assert_eq!(deleted.status, "deleted");
        assert!(
            deleted.hunks[0]
                .lines
                .iter()
                .any(|l| l.old_line == Some(1) && l.new_line.is_none())
        );
        let no_newline = view
            .files
            .iter()
            .find(|f| f.path == "nonewline.txt")
            .unwrap();
        assert!(
            no_newline.hunks[0]
                .lines
                .iter()
                .any(|l| l.kind == "marker" && l.old_line.is_none() && l.new_line.is_none())
        );
    }

    #[test]
    fn every_file_reports_its_syntax_state_and_uncoloured_lines_stay_plain() {
        use crate::gui_syntax::SyntaxStatus;
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        std::fs::write(repo.join("lib.rs"), "fn main() {}\n").unwrap();
        std::fs::write(repo.join("deploy"), "#!/usr/bin/env python3\nprint(1)\n").unwrap();
        std::fs::write(repo.join("image.bin"), b"\0binary\0").unwrap();
        git(&repo, &["add", "image.bin"]);
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        let file = |path: &str| view.files.iter().find(|f| f.path == path).unwrap();
        // The test sandbox installs no parsers: a supported language says so
        // and names the parser an install would fetch.
        assert_eq!(file("lib.rs").syntax.status, SyntaxStatus::NotInstalled);
        assert_eq!(file("lib.rs").syntax.language.as_deref(), Some("Rust"));
        assert_eq!(file("lib.rs").syntax.language_key.as_deref(), Some("rust"));
        assert_eq!(file("deploy").syntax.language.as_deref(), Some("Python"));
        assert_eq!(file("code.txt").syntax.status, SyntaxStatus::Unsupported);
        assert_eq!(file("code.txt").syntax.language, None);
        assert_eq!(file("image.bin").syntax.status, SyntaxStatus::Binary);
        assert!(
            view.files
                .iter()
                .flat_map(|f| &f.hunks)
                .flat_map(|h| &h.lines)
                .all(|l| l.syntax.is_none())
        );
        let json = serde_json::to_value(&view.files[0]).unwrap();
        assert!(json["syntax"]["status"].is_string());
        assert!(json["hunks"][0]["lines"][0]["syntax"].is_null());
    }

    #[test]
    fn amf_generated_flag_follows_the_shared_classifier_and_serializes() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".amf")).unwrap();
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::create_dir_all(repo.join(".opencode/plugins")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::write(repo.join(".amf/scratch.txt"), "generated\n").unwrap();
        std::fs::write(repo.join(".opencode/plugins/change-tracker.js"), "//\n").unwrap();
        std::fs::write(repo.join("src/mine.rs"), "fn main() {}\n").unwrap();
        // A staged managed path is someone's chosen work, not AMF's.
        std::fs::write(repo.join(".claude/amf-hook-state.json"), "{}\n").unwrap();
        git(&repo, &["add", ".claude/amf-hook-state.json"]);
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        let flag = |path: &str| {
            view.files
                .iter()
                .find(|f| f.path == path)
                .unwrap_or_else(|| {
                    panic!(
                        "{path} missing from {:?}",
                        view.files.iter().map(|f| &f.path).collect::<Vec<_>>()
                    )
                })
                .amf_generated
        };
        assert!(flag(".amf/scratch.txt"));
        assert!(flag(".opencode/plugins/change-tracker.js"));
        assert!(!flag(".claude/amf-hook-state.json"));
        assert!(!flag("src/mine.rs"));
        assert!(!flag("code.txt"));
        let json = serde_json::to_value(&view).unwrap();
        let amf = json["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == ".amf/scratch.txt")
            .unwrap();
        assert_eq!(amf["amf_generated"], serde_json::Value::Bool(true));
    }

    #[test]
    fn external_deletion_and_wrong_project_cannot_load_a_stale_feature() {
        let (dir, mut gui, target) = fixture();
        let wrong = FeatureTarget {
            project_id: "wrong".into(),
            ..target.clone()
        };
        assert_eq!(
            load(&mut gui, wrong, DiffOptions::default())
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
        let other = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = other.load_store().unwrap();
        store.projects[0].features.clear();
        other.save_store(&store).unwrap();
        assert_eq!(
            load(&mut gui, target, DiffOptions::default())
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
    }

    #[test]
    fn ordinary_directories_and_invalid_base_refs_report_actionable_errors() {
        let (_dir, mut gui, target) = fixture();
        let error = load(
            &mut gui,
            target.clone(),
            DiffOptions {
                base_ref: Some("missing-base".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.message.contains("missing-base"));
        let app = gui.app_for_workflow();
        app.store.projects[0].is_git = false;
        assert_eq!(
            load(&mut gui, target, DiffOptions::default())
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
    }
}
