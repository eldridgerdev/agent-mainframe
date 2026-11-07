use super::{
    PATCH_JUMP_STEP, handle_diff_viewer_key, handle_diff_viewer_wheel,
    handle_review_harness_pick_key,
};
use crate::app::App;
use crate::app::{
    AppMode, DiffViewerFocus, DiffViewerLayout, DiffViewerState, FileFilter, ViewState,
};
use crate::diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind};
use crate::project::{
    AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

mod support;
use support::*;

mod viewport_navigation;

mod search;

mod editor_launch;

mod finish_flow;

mod comment_suggestions;

mod overview;

mod file_navigation;

mod editor_behavior;

mod completion_handoff;

mod base_ref;

mod comment_verdicts;

mod file_tree;

mod comment_navigation;

mod undo;

mod history;
