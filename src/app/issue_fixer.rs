//! GitHub issue browsing and request ownership.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::thread;

use anyhow::Result;

use crate::app::{App, FeatureSetupState};
use crate::editor::TextEditor;
use crate::github::{GhCli, GithubIssue, GithubIssuePage, GithubRepository, GithubTransport};
use crate::project::{AgentKind, VibeMode};

pub(crate) const ISSUE_PAGE_SIZE: u32 = 25;
pub(crate) const ISSUE_FIX_PROMPT_MAX_CHARS: usize = 12_000;

/// Build the editable prompt shown before an issue-fixing feature is created.
/// All GitHub text is data inside labeled sections; it is never passed to a
/// shell or interpolated into a command. The per-field limits keep one issue
/// from consuming the whole agent context while retaining its identity and
/// useful description.
pub(crate) fn issue_fix_prompt(repository: &GithubRepository, issue: &GithubIssue) -> String {
    let url = if issue.url.trim().is_empty() {
        repository.issue_url(issue.number)
    } else {
        issue.url.trim().to_string()
    };
    let labels = issue
        .labels
        .iter()
        .map(|label| label.name.as_str())
        .filter(|label| !label.trim().is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let body = issue.body.as_deref().unwrap_or("");

    let mut prompt = format!(
        "Fix GitHub issue #{number} in repository {repository}.\n\n\
         Repository: {repository}\n\
         Issue number: #{number}\n\
         Issue URL: {url}\n\
         Title: {title}\n\
         Labels: {labels}\n\n\
         Issue body:\n{body}\n\n\
         Inspect the repository, implement the requested fix, and verify the result with the\n\
         appropriate tests or checks.\n",
        repository = repository.canonical(),
        number = issue.number,
        url = bound_text(&url, 500),
        title = bound_text(&issue.title, 800),
        labels = bound_text(&labels, 1_000),
        body = bound_text(body, 9_000),
    );
    if prompt.chars().count() > ISSUE_FIX_PROMPT_MAX_CHARS {
        prompt = prompt.chars().take(ISSUE_FIX_PROMPT_MAX_CHARS).collect();
    }
    prompt
}

fn bound_text(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueBrowserStatus {
    Loading,
    Ready,
    Error(String),
}

/// Full-screen issue list state. The repository and project identity travel
/// with the view so a result from another project cannot be applied to it.
#[derive(Debug, Clone)]
pub struct IssueBrowserState {
    pub project_index: usize,
    pub workdir: PathBuf,
    pub repository: GithubRepository,
    pub entries: Vec<GithubIssue>,
    pub selected: usize,
    pub page: u32,
    pub per_page: u32,
    pub has_next_page: bool,
    pub status: IssueBrowserStatus,
    pub request_id: u64,
}

/// Setup dialog state retained after an issue is selected. The browser is
/// carried along so Esc returns to the exact page and selection, while the
/// prompt editor preserves user changes through validation and retry paths.
#[derive(Debug, Clone)]
pub(crate) struct IssueSetupState {
    pub browser: IssueBrowserState,
    pub project_name: String,
    pub feature_name: String,
    pub settings: FeatureSetupState,
    pub use_worktree: bool,
    pub plan_mode: bool,
    pub quick_plan: bool,
    pub create_terminal: bool,
    pub steering_enabled: bool,
    pub remote_control: bool,
    pub session_name: String,
    pub prompt: TextEditor,
    pub prompt_editing: bool,
    pub row: IssueSetupRow,
    pub error: Option<String>,
    pub duplicate_override: bool,
    pub submitting: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IssueFeatureMatch {
    pub project_name: String,
    pub feature_name: String,
}

#[derive(Debug, Clone)]
pub(crate) struct IssueDuplicateWarningState {
    pub setup: IssueSetupState,
    pub matches: Vec<IssueFeatureMatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IssueSetupRow {
    FeatureName,
    Branch,
    Preset,
    Harness,
    Mode,
    Review,
    Chrome,
    Worktree,
    Plan,
    Prompt,
}

impl IssueSetupRow {
    pub(crate) const ALL: [Self; 10] = [
        Self::FeatureName,
        Self::Branch,
        Self::Preset,
        Self::Harness,
        Self::Mode,
        Self::Review,
        Self::Chrome,
        Self::Worktree,
        Self::Plan,
        Self::Prompt,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::FeatureName => "Feature name",
            Self::Branch => "Branch",
            Self::Preset => "Preset",
            Self::Harness => "Harness",
            Self::Mode => "Vibe mode",
            Self::Review => "Review mode",
            Self::Chrome => "Chrome",
            Self::Worktree => "Worktree",
            Self::Plan => "Guided plan",
            Self::Prompt => "Prompt",
        }
    }
}

impl IssueSetupState {
    pub(crate) fn row_index(&self) -> usize {
        Self::row_index_for(self.row)
    }

    fn row_index_for(row: IssueSetupRow) -> usize {
        IssueSetupRow::ALL
            .iter()
            .position(|candidate| *candidate == row)
            .unwrap_or(0)
    }

    pub(crate) fn move_row(&mut self, delta: isize) {
        let len = IssueSetupRow::ALL.len() as isize;
        let index = (self.row_index() as isize + delta).rem_euclid(len) as usize;
        self.row = IssueSetupRow::ALL[index];
    }

    pub(crate) fn focused_is_text(&self) -> bool {
        matches!(self.row, IssueSetupRow::FeatureName | IssueSetupRow::Branch)
    }

    pub(crate) fn focused_is_prompt(&self) -> bool {
        self.row == IssueSetupRow::Prompt
    }

    pub(crate) fn text_push(&mut self, c: char) {
        match self.row {
            IssueSetupRow::FeatureName => self.feature_name.push(c),
            IssueSetupRow::Branch => self.settings.branch.push(c),
            _ => {}
        }
        self.error = None;
    }

    pub(crate) fn text_backspace(&mut self) {
        match self.row {
            IssueSetupRow::FeatureName => {
                self.feature_name.pop();
            }
            IssueSetupRow::Branch => {
                self.settings.branch.pop();
            }
            _ => {}
        }
        self.error = None;
    }

    pub(crate) fn adjust(&mut self, delta: isize) {
        self.error = None;
        match self.row {
            IssueSetupRow::Preset => {
                self.settings.row = crate::app::FeatureSetupRow::ALL
                    .iter()
                    .position(|row| *row == crate::app::FeatureSetupRow::Preset)
                    .unwrap_or(0);
                let _ = self.settings.adjust(delta);
            }
            IssueSetupRow::Harness => {
                self.settings.row = crate::app::FeatureSetupRow::ALL
                    .iter()
                    .position(|row| *row == crate::app::FeatureSetupRow::Harness)
                    .unwrap_or(0);
                let _ = self.settings.adjust(delta);
                self.session_name = format!("{} 1", self.agent().display_name());
            }
            IssueSetupRow::Mode => {
                self.settings.row = crate::app::FeatureSetupRow::ALL
                    .iter()
                    .position(|row| *row == crate::app::FeatureSetupRow::Mode)
                    .unwrap_or(0);
                let _ = self.settings.adjust(delta);
            }
            IssueSetupRow::Review => {
                self.settings.row = crate::app::FeatureSetupRow::ALL
                    .iter()
                    .position(|row| *row == crate::app::FeatureSetupRow::Review)
                    .unwrap_or(0);
                let _ = self.settings.adjust(delta);
            }
            IssueSetupRow::Chrome => {
                self.settings.row = crate::app::FeatureSetupRow::ALL
                    .iter()
                    .position(|row| *row == crate::app::FeatureSetupRow::Chrome)
                    .unwrap_or(0);
                let _ = self.settings.adjust(delta);
            }
            IssueSetupRow::Worktree => self.use_worktree = !self.use_worktree,
            IssueSetupRow::Plan => {
                let current = match (self.plan_mode, self.quick_plan) {
                    (false, _) => 0,
                    (true, false) => 1,
                    (true, true) => 2,
                };
                match (current as isize + delta).rem_euclid(3) {
                    0 => {
                        self.plan_mode = false;
                        self.quick_plan = false;
                    }
                    1 => {
                        self.plan_mode = true;
                        self.quick_plan = false;
                    }
                    _ => {
                        self.plan_mode = true;
                        self.quick_plan = true;
                    }
                }
            }
            IssueSetupRow::FeatureName | IssueSetupRow::Branch | IssueSetupRow::Prompt => {}
        }
    }

    pub(crate) fn agent(&self) -> AgentKind {
        self.settings.agent()
    }

    pub(crate) fn mode(&self) -> VibeMode {
        self.settings.mode.clone()
    }
}

struct IssueLoadResult {
    request_id: u64,
    project_index: usize,
    workdir: PathBuf,
    repository: GithubRepository,
    page: u32,
    result: Result<GithubIssuePage>,
}

/// Owns the receiver for the current issue request. Dropping the receiver is
/// cancellation; the worker may finish, but its result can no longer be
/// observed or applied by the app.
#[derive(Default)]
pub(crate) struct IssueWork {
    next_request_id: u64,
    receiver: Option<Receiver<IssueLoadResult>>,
}

struct IssueCommentResult {
    feature_id: String,
    source: crate::project::IssueSource,
    result: Result<()>,
}

/// Background comment attempts. Each attempt reconciles the stable marker
/// before posting, so retrying an ambiguous timeout cannot create duplicates.
#[derive(Default)]
pub(crate) struct IssueCommentWork {
    receivers: Vec<Receiver<IssueCommentResult>>,
}

impl IssueCommentWork {
    fn begin(
        &mut self,
        feature_id: String,
        workdir: PathBuf,
        source: crate::project::IssueSource,
        body: String,
    ) {
        self.begin_with_transport(feature_id, workdir, source, body, Arc::new(GhCli));
    }

    fn begin_with_transport(
        &mut self,
        feature_id: String,
        workdir: PathBuf,
        source: crate::project::IssueSource,
        body: String,
        transport: Arc<dyn GithubTransport + Send + Sync>,
    ) {
        let (tx, rx) = channel();
        let result_source = source.clone();
        thread::spawn(move || {
            let repository = GithubRepository {
                host: source.host.clone(),
                owner: source.owner.clone(),
                name: source.repository.clone(),
            };
            let marker = issue_comment_marker(&feature_id);
            let result = transport
                .check_available()
                .and_then(|_| transport.check_auth())
                .and_then(|_| transport.issue_comment_bodies(&workdir, &repository, source.number))
                .and_then(|comments| {
                    if comments.iter().any(|comment| comment.contains(&marker)) {
                        Ok(())
                    } else {
                        transport.post_issue_comment(&workdir, &repository, source.number, &body)
                    }
                });
            let _ = tx.send(IssueCommentResult {
                feature_id,
                source: result_source,
                result,
            });
        });
        self.receivers.push(rx);
    }

    fn poll(&mut self) -> Vec<IssueCommentResult> {
        let mut completed = Vec::new();
        let mut pending = Vec::new();
        for receiver in self.receivers.drain(..) {
            match receiver.try_recv() {
                Ok(result) => completed.push(result),
                Err(TryRecvError::Empty) => pending.push(receiver),
                Err(TryRecvError::Disconnected) => {}
            }
        }
        self.receivers = pending;
        completed
    }

    pub(crate) fn pending(&self) -> bool {
        !self.receivers.is_empty()
    }
}

fn issue_comment_marker(feature_id: &str) -> String {
    format!("<!-- amf-issue-feature:{feature_id} -->")
}

fn issue_feature_comment(feature_id: &str, feature_name: &str, branch: &str) -> String {
    format!(
        "AMF created feature `{feature_name}` on branch `{branch}` to work on this issue.\n\n{}",
        issue_comment_marker(feature_id)
    )
}

impl IssueWork {
    fn begin(
        &mut self,
        project_index: usize,
        workdir: PathBuf,
        repository: GithubRepository,
        page: u32,
    ) -> u64 {
        self.begin_with_transport(project_index, workdir, repository, page, Arc::new(GhCli))
    }

    fn begin_with_transport(
        &mut self,
        project_index: usize,
        workdir: PathBuf,
        repository: GithubRepository,
        page: u32,
        client: Arc<dyn GithubTransport + Send + Sync>,
    ) -> u64 {
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let request_id = self.next_request_id;
        let worker_repository = repository.clone();
        let result_workdir = workdir.clone();
        let (tx, rx) = channel();
        thread::spawn(move || {
            let result = client
                .check_available()
                .and_then(|_| client.check_auth())
                .and_then(|_| {
                    client.list_issues(&workdir, &worker_repository, page, ISSUE_PAGE_SIZE)
                });
            // The caller owns cancellation. A dropped receiver is expected
            // when the browser closes or a newer request supersedes this one.
            let _ = tx.send(IssueLoadResult {
                request_id,
                project_index,
                workdir: result_workdir,
                repository,
                page,
                result,
            });
        });
        self.receiver = Some(rx);
        request_id
    }

    fn poll(&self) -> Option<Result<IssueLoadResult, TryRecvError>> {
        self.receiver.as_ref().map(Receiver::try_recv)
    }

    fn cancel(&mut self) {
        self.receiver = None;
    }

    pub(crate) fn pending(&self) -> bool {
        self.receiver.is_some()
    }
}

impl App {
    fn recoverable_issue_comment_indices(&self) -> Vec<(usize, usize)> {
        self.store
            .projects
            .iter()
            .enumerate()
            .flat_map(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .filter_map(move |(fi, feature)| {
                        feature
                            .issue_source
                            .as_ref()
                            .is_some_and(|source| {
                                !matches!(
                                    source.comment_status,
                                    crate::project::IssueCommentStatus::Posted
                                )
                            })
                            .then_some((pi, fi))
                    })
            })
            .collect()
    }

    pub(crate) fn queue_recoverable_issue_comments(&mut self) {
        for (pi, fi) in self.recoverable_issue_comment_indices() {
            self.queue_issue_comment_for_feature(pi, fi);
        }
    }

    /// Open the issue browser for a project selected from the dashboard.
    /// Repository resolution is local and cheap; network/auth work starts on
    /// the worker immediately after the view enters its loading state.
    pub fn open_issue_browser_for_project(&mut self, project_index: usize) {
        let repository = match self.github_repository_for_project(project_index) {
            Ok(repository) => repository,
            Err(error) => {
                self.show_error(error.into());
                return;
            }
        };
        let workdir = self.store.projects[project_index].repo.clone();
        self.issue_work.cancel();
        self.mode = crate::app::AppMode::IssueBrowser(IssueBrowserState {
            project_index,
            workdir,
            repository: repository.clone(),
            entries: Vec::new(),
            selected: 0,
            page: 1,
            per_page: ISSUE_PAGE_SIZE,
            has_next_page: false,
            status: IssueBrowserStatus::Loading,
            request_id: 0,
        });
        self.start_issue_page_load(repository, 1);
    }

    /// Open the compact PR-Triage-style setup dialog for the highlighted
    /// issue. The browser state stays attached to the dialog so cancellation
    /// returns without losing the page, selection, or loaded snapshot.
    pub(crate) fn issue_browser_open_setup(&mut self) {
        let browser = match &self.mode {
            crate::app::AppMode::IssueBrowser(state)
                if matches!(state.status, IssueBrowserStatus::Ready)
                    && !state.entries.is_empty() =>
            {
                state.clone()
            }
            crate::app::AppMode::IssueBrowser(_) => {
                self.message = Some("No loaded issue is selected".to_string());
                return;
            }
            _ => return,
        };
        let Some(issue) = browser.entries.get(browser.selected).cloned() else {
            self.message = Some("No loaded issue is selected".to_string());
            return;
        };
        let Some(project) = self.store.projects.get(browser.project_index) else {
            self.show_error(anyhow::anyhow!("Selected project no longer exists"));
            return;
        };
        let project_name = project.name.clone();
        let project_repo = project.repo.clone();
        let preferred_agent = project.preferred_agent.clone();
        let agents = self.allowed_agents_for_repo(&project_repo);
        if agents.is_empty() {
            self.message = Some("No harnesses configured for this workspace".to_string());
            return;
        }
        let (agent, agent_index) = self.normalize_agent_for_repo(&project_repo, &preferred_agent);
        let title_slug = crate::project::normalized_feature_name(&issue.title);
        let title_suffix = title_slug
            .chars()
            .take(48)
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let branch = if title_suffix.is_empty() {
            format!("issue/{}", issue.number)
        } else {
            format!("issue/{}-{}", issue.number, title_suffix)
        };
        let settings = FeatureSetupState {
            presets: self
                .extension_for_repo(&project_repo)
                .allowed_feature_presets(),
            preset_index: 0,
            agents,
            agent_index,
            mode: VibeMode::Vibeless,
            review: false,
            enable_chrome: false,
            branch,
            row: 0,
            error: None,
            pending_batch: false,
        };
        let prompt_repository = browser.repository.clone();
        let prompt = issue_fix_prompt(&prompt_repository, &issue);
        self.issue_work.cancel();
        self.mode = crate::app::AppMode::IssueSetup(IssueSetupState {
            browser,
            project_name,
            feature_name: format!("fix-issue-{}", issue.number),
            settings,
            use_worktree: true,
            plan_mode: false,
            quick_plan: false,
            create_terminal: false,
            steering_enabled: false,
            remote_control: self.config.remote_control_default,
            session_name: App::default_session_name_for_agent(&agent),
            prompt: TextEditor::new(prompt),
            prompt_editing: false,
            row: IssueSetupRow::FeatureName,
            error: None,
            duplicate_override: false,
            submitting: false,
        });
        self.message = None;
    }

    pub(crate) fn issue_setup_cancel(&mut self) {
        let state = match std::mem::replace(&mut self.mode, crate::app::AppMode::Normal) {
            crate::app::AppMode::IssueSetup(state) => state,
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = crate::app::AppMode::IssueBrowser(state.browser);
        self.message = None;
    }

    pub(crate) fn issue_setup_move(&mut self, delta: isize) {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode
            && !state.prompt_editing
        {
            state.move_row(delta);
        }
    }

    pub(crate) fn issue_setup_adjust(&mut self, delta: isize) {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode
            && !state.prompt_editing
        {
            state.adjust(delta);
        }
    }

    pub(crate) fn issue_setup_text_push(&mut self, c: char) {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode
            && !state.prompt_editing
            && state.focused_is_text()
        {
            state.text_push(c);
        }
    }

    pub(crate) fn issue_setup_text_backspace(&mut self) {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode
            && !state.prompt_editing
            && state.focused_is_text()
        {
            state.text_backspace();
        }
    }

    pub(crate) fn issue_setup_prompt_key(&mut self, key: crossterm::event::KeyEvent) {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode
            && state.prompt_editing
        {
            let _ = state.prompt.handle_key(key);
        }
    }

    pub(crate) fn issue_setup_toggle_prompt_editing(&mut self) {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode
            && state.focused_is_prompt()
        {
            state.prompt_editing = !state.prompt_editing;
        }
    }

    pub(crate) fn issue_setup_confirm(&mut self) -> Result<()> {
        if let crate::app::AppMode::IssueSetup(state) = &mut self.mode {
            if state.submitting {
                return Ok(());
            }
            state.submitting = true;
        }
        let state = match std::mem::replace(&mut self.mode, crate::app::AppMode::Normal) {
            crate::app::AppMode::IssueSetup(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };
        if state.prompt_editing {
            let mut state = state;
            state.submitting = false;
            self.mode = crate::app::AppMode::IssueSetup(state);
            return Ok(());
        }
        let feature_name = state.feature_name.trim().to_string();
        let branch = state.settings.branch.trim().to_string();
        let error = if feature_name.is_empty() {
            Some("Feature name cannot be empty".to_string())
        } else if branch.is_empty() {
            Some("Branch cannot be empty".to_string())
        } else if self
            .store
            .projects
            .get(state.browser.project_index)
            .is_some_and(|project| {
                let normalized = crate::project::normalized_feature_name(&feature_name);
                project.features.iter().any(|feature| {
                    crate::project::normalized_feature_name(&feature.name) == normalized
                })
            })
        {
            Some(format!("Feature '{}' already exists", feature_name))
        } else {
            None
        };
        if let Some(error) = error {
            let mut state = state;
            state.error = Some(error);
            state.submitting = false;
            self.mode = crate::app::AppMode::IssueSetup(state);
            return Ok(());
        }

        let issue_source = state
            .browser
            .entries
            .get(state.browser.selected)
            .map(|issue| crate::project::IssueSource {
                host: state.browser.repository.host.clone(),
                owner: state.browser.repository.owner.clone(),
                repository: state.browser.repository.name.clone(),
                number: issue.number,
                comment_status: crate::project::IssueCommentStatus::Pending,
            });
        if !state.duplicate_override
            && let Some(source) = &issue_source
        {
            let matches = self.features_for_issue(source);
            if !matches.is_empty() {
                let mut state = state;
                state.submitting = false;
                self.mode =
                    crate::app::AppMode::IssueDuplicateWarning(IssueDuplicateWarningState {
                        setup: state,
                        matches,
                    });
                return Ok(());
            }
        }

        self.selection = crate::app::Selection::Project(state.browser.project_index);
        self.start_create_feature();
        let crate::app::AppMode::CreatingFeature(create) = &mut self.mode else {
            return Ok(());
        };
        create.feature_name = Some(feature_name);
        create.issue_source = issue_source;
        create.branch = branch;
        create.source_index = 0;
        create.use_worktree = state.use_worktree;
        create.agent = state.agent();
        create.agent_index = state.settings.agent_index;
        create.mode = state.mode();
        create.review = state.settings.review;
        create.enable_chrome = state.settings.enable_chrome;
        create.remote_control = state.remote_control;
        create.steering_enabled = state.steering_enabled;
        create.plan_mode = state.plan_mode;
        create.quick_plan = state.quick_plan;
        create.create_terminal = state.create_terminal;
        create.session_name = state.session_name;
        create.task_prompt = state.prompt.text().to_string();
        create.refresh_prompt_analysis();
        create.step = crate::app::CreateFeatureStep::SessionName;
        self.create_feature()
    }

    fn features_for_issue(&self, source: &crate::project::IssueSource) -> Vec<IssueFeatureMatch> {
        self.store
            .projects
            .iter()
            .flat_map(|project| {
                project.features.iter().filter_map(|feature| {
                    let candidate = feature.issue_source.as_ref()?;
                    (candidate.number == source.number
                        && candidate.host.eq_ignore_ascii_case(&source.host)
                        && candidate.owner.eq_ignore_ascii_case(&source.owner)
                        && candidate
                            .repository
                            .eq_ignore_ascii_case(&source.repository))
                    .then(|| IssueFeatureMatch {
                        project_name: project.name.clone(),
                        feature_name: feature.name.clone(),
                    })
                })
            })
            .collect()
    }

    pub(crate) fn issue_duplicate_cancel(&mut self) {
        let warning = match std::mem::replace(&mut self.mode, crate::app::AppMode::Normal) {
            crate::app::AppMode::IssueDuplicateWarning(warning) => warning,
            other => {
                self.mode = other;
                return;
            }
        };
        self.mode = crate::app::AppMode::IssueSetup(warning.setup);
    }

    pub(crate) fn issue_duplicate_override(&mut self) -> Result<()> {
        let mut warning = match std::mem::replace(&mut self.mode, crate::app::AppMode::Normal) {
            crate::app::AppMode::IssueDuplicateWarning(warning) => warning,
            other => {
                self.mode = other;
                return Ok(());
            }
        };
        warning.setup.duplicate_override = true;
        self.mode = crate::app::AppMode::IssueSetup(warning.setup);
        self.issue_setup_confirm()
    }

    fn start_issue_page_load(&mut self, repository: GithubRepository, page: u32) {
        let project_index = match &self.mode {
            crate::app::AppMode::IssueBrowser(state) => state.project_index,
            _ => return,
        };
        let workdir = match &self.mode {
            crate::app::AppMode::IssueBrowser(state) => state.workdir.clone(),
            _ => return,
        };
        let request_id = self
            .issue_work
            .begin(project_index, workdir, repository, page);
        if let crate::app::AppMode::IssueBrowser(state) = &mut self.mode {
            state.page = page;
            state.selected = 0;
            state.status = IssueBrowserStatus::Loading;
            state.request_id = request_id;
        }
    }

    pub fn retry_issue_browser(&mut self) {
        let Some((repository, page)) = (match &self.mode {
            crate::app::AppMode::IssueBrowser(state) => {
                Some((state.repository.clone(), state.page))
            }
            _ => None,
        }) else {
            return;
        };
        self.start_issue_page_load(repository, page);
    }

    pub fn refresh_issue_browser(&mut self) {
        self.retry_issue_browser();
    }

    pub fn issue_browser_select_next(&mut self) {
        if let crate::app::AppMode::IssueBrowser(state) = &mut self.mode {
            state.selected = (state.selected + 1).min(state.entries.len().saturating_sub(1));
        }
    }

    pub fn issue_browser_select_prev(&mut self) {
        if let crate::app::AppMode::IssueBrowser(state) = &mut self.mode {
            state.selected = state.selected.saturating_sub(1);
        }
    }

    pub fn issue_browser_next_page(&mut self) {
        let Some((repository, page)) = (match &self.mode {
            crate::app::AppMode::IssueBrowser(state) if state.has_next_page => {
                Some((state.repository.clone(), state.page + 1))
            }
            _ => None,
        }) else {
            return;
        };
        self.start_issue_page_load(repository, page);
    }

    pub fn issue_browser_previous_page(&mut self) {
        let Some((repository, page)) = (match &self.mode {
            crate::app::AppMode::IssueBrowser(state) if state.page > 1 => {
                Some((state.repository.clone(), state.page - 1))
            }
            _ => None,
        }) else {
            return;
        };
        self.start_issue_page_load(repository, page);
    }

    pub fn close_issue_browser(&mut self) {
        self.issue_work.cancel();
        if matches!(self.mode, crate::app::AppMode::IssueBrowser(_)) {
            self.mode = crate::app::AppMode::Normal;
        }
    }

    /// Apply one worker result only to the request identity that is still
    /// visible. A disconnected worker becomes a retryable failure.
    pub fn poll_issue_browser_bg(&mut self) -> bool {
        let Some(polled) = self.issue_work.poll() else {
            return false;
        };
        let result = match polled {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.issue_work.cancel();
                if let crate::app::AppMode::IssueBrowser(state) = &mut self.mode {
                    state.status = IssueBrowserStatus::Error(
                        "Issue request stopped unexpectedly; press r to retry".to_string(),
                    );
                }
                return true;
            }
        };
        self.issue_work.cancel();

        let crate::app::AppMode::IssueBrowser(state) = &mut self.mode else {
            return false;
        };
        if state.request_id != result.request_id
            || state.project_index != result.project_index
            || state.workdir != result.workdir
            || state.repository != result.repository
            || state.page != result.page
        {
            return false;
        }
        match result.result {
            Ok(page) => {
                state.entries = page.issues;
                state.per_page = page.per_page;
                state.has_next_page = page.has_next_page;
                state.status = IssueBrowserStatus::Ready;
            }
            Err(error) => state.status = IssueBrowserStatus::Error(error.to_string()),
        }
        true
    }

    pub(crate) fn queue_issue_comment_for_feature(&mut self, pi: usize, fi: usize) {
        let Some((feature_id, feature_name, branch, workdir, source)) =
            self.store.projects.get(pi).and_then(|project| {
                project.features.get(fi).and_then(|feature| {
                    feature.issue_source.clone().map(|source| {
                        (
                            feature.id.clone(),
                            feature.name.clone(),
                            feature.branch.clone(),
                            project.repo.clone(),
                            source,
                        )
                    })
                })
            })
        else {
            return;
        };
        if matches!(
            source.comment_status,
            crate::project::IssueCommentStatus::Posted
        ) {
            return;
        }
        let body = issue_feature_comment(&feature_id, &feature_name, &branch);
        self.issue_comment_work
            .begin(feature_id, workdir, source, body);
    }

    pub(crate) fn poll_issue_comment_bg(&mut self) -> bool {
        let completed = self.issue_comment_work.poll();
        if completed.is_empty() {
            return false;
        }
        for result in completed {
            let notice = {
                let Some(feature) = self.store.projects.iter_mut().find_map(|project| {
                    project
                        .features
                        .iter_mut()
                        .find(|feature| feature.id == result.feature_id)
                }) else {
                    continue;
                };
                let Some(source) = feature.issue_source.as_mut() else {
                    continue;
                };
                if source.host != result.source.host
                    || source.owner != result.source.owner
                    || source.repository != result.source.repository
                    || source.number != result.source.number
                {
                    continue;
                }
                let number = source.number;
                match result.result {
                    Ok(()) => {
                        source.comment_status = crate::project::IssueCommentStatus::Posted;
                        Ok(format!("Posted issue #{number} feature comment"))
                    }
                    Err(error) => {
                        let message = error.to_string();
                        source.comment_status =
                            crate::project::IssueCommentStatus::Failed(message.clone());
                        Err(format!(
                            "Feature kept, but issue #{number} comment failed: {message}"
                        ))
                    }
                }
            };
            match notice {
                Ok(message) => self.push_toast_success(message),
                Err(message) => self.push_toast_warning(message),
            }
        }
        if let Err(error) = self.save() {
            self.log_warn(
                "issue_comment",
                format!("failed to persist issue comment status: {error}"),
            );
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeTransport {
        result: Result<GithubIssuePage>,
        comments: Vec<String>,
        post_error: Option<String>,
        posts: Arc<AtomicUsize>,
    }

    impl FakeTransport {
        fn issues(result: Result<GithubIssuePage>) -> Self {
            Self {
                result,
                comments: Vec::new(),
                post_error: None,
                posts: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn comments(
            comments: Vec<String>,
            post_error: Option<&str>,
            posts: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                result: Ok(GithubIssuePage {
                    issues: Vec::new(),
                    page: 1,
                    per_page: ISSUE_PAGE_SIZE,
                    has_next_page: false,
                }),
                comments,
                post_error: post_error.map(str::to_string),
                posts,
            }
        }
    }

    impl GithubTransport for FakeTransport {
        fn check_available(&self) -> Result<()> {
            Ok(())
        }

        fn check_auth(&self) -> Result<()> {
            Ok(())
        }

        fn current_user(&self, _workdir: &std::path::Path) -> Result<String> {
            Ok("test-user".to_string())
        }

        fn list_prs(
            &self,
            _workdir: &std::path::Path,
            _include_closed: bool,
        ) -> Result<Vec<crate::github::PrListEntry>> {
            Ok(Vec::new())
        }

        fn resolve_pr(&self, _workdir: &std::path::Path) -> Result<crate::github::PrResolution> {
            anyhow::bail!("unused in issue test")
        }

        fn fetch_pr_by_number(
            &self,
            _workdir: &std::path::Path,
            _number: u32,
        ) -> Result<crate::github::PrRef> {
            anyhow::bail!("unused in issue test")
        }

        fn list_issues(
            &self,
            _workdir: &std::path::Path,
            _repository: &GithubRepository,
            _page: u32,
            _per_page: u32,
        ) -> Result<GithubIssuePage> {
            match &self.result {
                Ok(page) => Ok(page.clone()),
                Err(error) => Err(anyhow::anyhow!(error.to_string())),
            }
        }

        fn issue_comment_bodies(
            &self,
            _workdir: &std::path::Path,
            _repository: &GithubRepository,
            _number: u32,
        ) -> Result<Vec<String>> {
            Ok(self.comments.clone())
        }

        fn post_issue_comment(
            &self,
            _workdir: &std::path::Path,
            _repository: &GithubRepository,
            _number: u32,
            _body: &str,
        ) -> Result<()> {
            self.posts.fetch_add(1, Ordering::SeqCst);
            match &self.post_error {
                Some(error) => Err(anyhow::anyhow!(error.clone())),
                None => Ok(()),
            }
        }
    }

    fn repository() -> GithubRepository {
        GithubRepository {
            host: "github.com".to_string(),
            owner: "acme".to_string(),
            name: "widget".to_string(),
        }
    }

    fn issue(number: u32) -> GithubIssue {
        GithubIssue {
            number,
            title: "Handle Unicode 🚀 safely".to_string(),
            body: Some("first line\nsecond line".to_string()),
            url: format!("https://github.com/acme/widget/issues/{number}"),
            labels: vec![crate::github::GithubIssueLabel {
                name: "bug".to_string(),
            }],
            updated_at: String::new(),
            pull_request: None,
        }
    }

    fn app_with_issue_browser(repo: &std::path::Path) -> App {
        let project = crate::project::Project {
            id: "project-1".to_string(),
            name: "widget".to_string(),
            repo: repo.to_path_buf(),
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent: AgentKind::Claude,
            is_git: true,
        };
        let store = crate::project::ProjectStore {
            version: crate::project::CURRENT_PROJECT_STORE_VERSION,
            projects: vec![project],
            session_bookmarks: Vec::new(),
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: std::collections::HashMap::new(),
        };
        let mut app = App::new_for_test(
            store,
            Box::new(crate::traits::MockTmuxOps::new()),
            Box::new(crate::traits::MockWorktreeOps::new()),
        );
        app.mode = crate::app::AppMode::IssueBrowser(IssueBrowserState {
            project_index: 0,
            workdir: repo.to_path_buf(),
            repository: repository(),
            entries: vec![issue(42)],
            selected: 0,
            page: 1,
            per_page: ISSUE_PAGE_SIZE,
            has_next_page: false,
            status: IssueBrowserStatus::Ready,
            request_id: 0,
        });
        app
    }

    fn wait_for_result(work: &IssueWork) -> IssueLoadResult {
        for _ in 0..100 {
            match work.poll() {
                Some(Ok(result)) => return result,
                Some(Err(TryRecvError::Empty)) | None => {
                    thread::sleep(std::time::Duration::from_millis(1))
                }
                Some(Err(TryRecvError::Disconnected)) => panic!("worker disconnected"),
            }
        }
        panic!("worker did not finish")
    }

    fn issue_source(status: crate::project::IssueCommentStatus) -> crate::project::IssueSource {
        crate::project::IssueSource {
            host: "github.com".to_string(),
            owner: "acme".to_string(),
            repository: "widget".to_string(),
            number: 42,
            comment_status: status,
        }
    }

    fn wait_for_comment_result(work: &mut IssueCommentWork) -> IssueCommentResult {
        for _ in 0..100 {
            if let Some(result) = work.poll().into_iter().next() {
                return result;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("comment worker did not finish")
    }

    #[test]
    fn issue_worker_preserves_page_identity_and_successful_empty_pages() {
        let page = GithubIssuePage {
            issues: Vec::new(),
            page: 3,
            per_page: ISSUE_PAGE_SIZE,
            has_next_page: false,
        };
        let mut work = IssueWork::default();
        let id = work.begin_with_transport(
            4,
            PathBuf::from("/project"),
            repository(),
            3,
            Arc::new(FakeTransport::issues(Ok(page))),
        );
        let result = wait_for_result(&work);
        assert_eq!(result.request_id, id);
        assert_eq!(result.project_index, 4);
        assert_eq!(result.page, 3);
        assert!(result.result.unwrap().issues.is_empty());
    }

    #[test]
    fn issue_prompt_is_bounded_and_keeps_issue_metadata_as_literal_text() {
        let issue = GithubIssue {
            number: 42,
            title: "Unicode 🚀 title".to_string(),
            body: Some("line one\nline two; $(echo should-not-run)".repeat(1_000)),
            url: String::new(),
            labels: vec![
                crate::github::GithubIssueLabel {
                    name: "bug".to_string(),
                },
                crate::github::GithubIssueLabel {
                    name: "needs-help".to_string(),
                },
            ],
            updated_at: String::new(),
            pull_request: None,
        };
        let prompt = issue_fix_prompt(&repository(), &issue);
        assert!(prompt.chars().count() <= ISSUE_FIX_PROMPT_MAX_CHARS);
        assert!(prompt.contains("github.com/acme/widget"));
        assert!(prompt.contains("Issue URL: https://github.com/acme/widget/issues/42"));
        assert!(prompt.contains("Unicode 🚀 title"));
        assert!(prompt.contains("line one\nline two; $(echo should-not-run)"));
        assert!(prompt.contains("bug, needs-help"));
    }

    #[test]
    fn issue_prompt_handles_empty_body_without_losing_identity() {
        let issue = GithubIssue {
            number: 9,
            title: String::new(),
            body: None,
            url: "https://github.com/acme/widget/issues/9".to_string(),
            labels: Vec::new(),
            updated_at: String::new(),
            pull_request: None,
        };
        let prompt = issue_fix_prompt(&repository(), &issue);
        assert!(prompt.contains("Issue number: #9"));
        assert!(prompt.contains("Issue body:\n\n"));
    }

    #[test]
    fn issue_worker_keeps_failures_retryable() {
        let mut work = IssueWork::default();
        work.begin_with_transport(
            0,
            PathBuf::from("/project"),
            repository(),
            1,
            Arc::new(FakeTransport::issues(Err(anyhow::anyhow!(
                "GitHub rate limit exceeded"
            )))),
        );
        let result = wait_for_result(&work);
        assert!(
            result
                .result
                .unwrap_err()
                .to_string()
                .contains("rate limit")
        );

        // The receiver is cleared only by the poll/apply boundary, so a
        // caller can immediately start the same request again after showing
        // the error.
        work.cancel();
        assert!(!work.pending());
    }

    #[test]
    fn issue_comment_reconciles_marker_before_posting() {
        let feature_id = "feature-42";
        let marker = issue_comment_marker(feature_id);
        let posts = Arc::new(AtomicUsize::new(0));
        let mut work = IssueCommentWork::default();
        let body = issue_feature_comment(feature_id, "fix-issue-42", "issue/42");
        assert!(body.contains("feature `fix-issue-42`"));
        assert!(body.contains("branch `issue/42`"));
        assert!(!body.contains("http://"));
        assert!(!body.contains("https://"));
        work.begin_with_transport(
            feature_id.to_string(),
            PathBuf::from("/project"),
            issue_source(crate::project::IssueCommentStatus::Pending),
            body,
            Arc::new(FakeTransport::comments(
                vec![format!("already posted\n\n{marker}")],
                None,
                Arc::clone(&posts),
            )),
        );

        let result = wait_for_comment_result(&mut work);
        assert!(result.result.is_ok());
        assert_eq!(posts.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn issue_comment_failure_keeps_feature_and_records_retry_state() {
        let repo = tempfile::tempdir().unwrap();
        let mut app = app_with_issue_browser(repo.path());
        let mut feature = crate::project::Feature::new(
            "fix-issue-42".to_string(),
            "issue/42".to_string(),
            repo.path().to_path_buf(),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        let feature_id = feature.id.clone();
        let source = issue_source(crate::project::IssueCommentStatus::Pending);
        feature.issue_source = Some(source.clone());
        app.store.projects[0].features.push(feature);
        let posts = Arc::new(AtomicUsize::new(0));
        app.issue_comment_work.begin_with_transport(
            feature_id.clone(),
            repo.path().to_path_buf(),
            source,
            issue_feature_comment(&feature_id, "fix-issue-42", "issue/42"),
            Arc::new(FakeTransport::comments(
                Vec::new(),
                Some("request timed out after GitHub accepted it"),
                Arc::clone(&posts),
            )),
        );

        for _ in 0..100 {
            if app.poll_issue_comment_bg() {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(1));
        }

        assert_eq!(app.store.projects[0].features.len(), 1);
        assert_eq!(app.store.projects[0].features[0].id, feature_id);
        assert!(matches!(
            app.store.projects[0].features[0]
                .issue_source
                .as_ref()
                .map(|source| &source.comment_status),
            Some(crate::project::IssueCommentStatus::Failed(message))
                if message.contains("timed out")
        ));
        assert_eq!(posts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn startup_recovery_selects_pending_and_failed_comments_but_not_posted_ones() {
        let repo = tempfile::tempdir().unwrap();
        let mut app = app_with_issue_browser(repo.path());
        for (index, status) in [
            crate::project::IssueCommentStatus::Pending,
            crate::project::IssueCommentStatus::Failed("network timeout".to_string()),
            crate::project::IssueCommentStatus::Posted,
        ]
        .into_iter()
        .enumerate()
        {
            let mut feature = crate::project::Feature::new(
                format!("feature-{index}"),
                format!("issue/{index}"),
                repo.path().join(format!("feature-{index}")),
                true,
                VibeMode::Vibeless,
                false,
                false,
                AgentKind::Claude,
                false,
                false,
            );
            let mut source = issue_source(status);
            source.number += index as u32;
            feature.issue_source = Some(source);
            app.store.projects[0].features.push(feature);
        }

        assert_eq!(
            app.recoverable_issue_comment_indices(),
            vec![(0, 0), (0, 1)]
        );
    }

    #[test]
    fn issue_result_is_ignored_after_the_view_identity_changes() {
        let page = GithubIssuePage {
            issues: Vec::new(),
            page: 1,
            per_page: ISSUE_PAGE_SIZE,
            has_next_page: false,
        };
        let mut work = IssueWork::default();
        let request_id = work.begin_with_transport(
            1,
            PathBuf::from("/project-two"),
            repository(),
            1,
            Arc::new(FakeTransport::issues(Ok(page))),
        );

        let mut app = App::new_for_test(
            crate::project::ProjectStore::empty(),
            Box::new(crate::traits::MockTmuxOps::new()),
            Box::new(crate::traits::MockWorktreeOps::new()),
        );
        app.issue_work = work;
        app.mode = crate::app::AppMode::IssueBrowser(IssueBrowserState {
            project_index: 1,
            workdir: PathBuf::from("/project-one"),
            repository: repository(),
            entries: Vec::new(),
            selected: 0,
            page: 1,
            per_page: ISSUE_PAGE_SIZE,
            has_next_page: false,
            status: IssueBrowserStatus::Loading,
            request_id,
        });
        // The workdir changed even though the project index, repository, and
        // page still look the same. This is a different project context and
        // must not accept the old result.
        for _ in 0..100 {
            if !app.issue_work.pending() {
                break;
            }
            app.poll_issue_browser_bg();
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(matches!(
            app.mode,
            crate::app::AppMode::IssueBrowser(IssueBrowserState {
                status: IssueBrowserStatus::Loading,
                entries,
                ..
            }) if entries.is_empty()
        ));
    }

    #[test]
    fn issue_setup_preserves_prompt_edits_after_validation_and_into_plan_launch() {
        let repo = tempfile::tempdir().unwrap();
        let mut app = app_with_issue_browser(repo.path());
        app.issue_browser_open_setup();

        let edited_prompt = if let crate::app::AppMode::IssueSetup(state) = &mut app.mode {
            state.prompt_editing = true;
            state
                .prompt
                .handle_key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
            state.prompt_editing = false;
            state.feature_name.clear();
            state.prompt.text().to_string()
        } else {
            panic!("issue setup did not open");
        };
        app.issue_setup_confirm().unwrap();
        assert!(matches!(
            &app.mode,
            crate::app::AppMode::IssueSetup(state)
                if state.prompt.text() == edited_prompt
                    && state.error.as_deref() == Some("Feature name cannot be empty")
        ));

        if let crate::app::AppMode::IssueSetup(state) = &mut app.mode {
            state.feature_name = "issue-fix-display-name".to_string();
            state.settings.branch = "issue/42-custom-branch".to_string();
            state.settings.review = true;
            state.settings.enable_chrome = true;
            state.use_worktree = false;
            state.plan_mode = true;
            state.quick_plan = false;
        }
        app.issue_setup_confirm().unwrap();

        let crate::app::AppMode::PlanInterview(interview) = &app.mode else {
            panic!("accepted issue setup did not enter guided planning");
        };
        let prepared = interview.pending_launch.as_ref().unwrap();
        assert_eq!(
            prepared.feature_name.as_deref(),
            Some("issue-fix-display-name")
        );
        assert_eq!(prepared.branch, "issue/42-custom-branch");
        assert_eq!(prepared.agent, AgentKind::Claude);
        assert!(prepared.review);
        assert!(prepared.enable_chrome);
        assert!(!prepared.is_worktree);
        assert_eq!(
            prepared.startup_prompt.as_deref(),
            Some(edited_prompt.as_str())
        );
        assert_eq!(
            prepared.issue_source,
            Some(crate::project::IssueSource {
                host: "github.com".to_string(),
                owner: "acme".to_string(),
                repository: "widget".to_string(),
                number: 42,
                comment_status: crate::project::IssueCommentStatus::Pending,
            })
        );
        assert_eq!(interview.editor.text(), edited_prompt);
    }

    #[test]
    fn duplicate_warning_preserves_edits_and_explicit_override_allows_second_launch() {
        let repo = tempfile::tempdir().unwrap();
        let mut app = app_with_issue_browser(repo.path());
        let mut existing = crate::project::Feature::new(
            "existing-issue-fix".to_string(),
            "issue/42-existing".to_string(),
            repo.path().join("existing"),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        existing.issue_source = Some(crate::project::IssueSource {
            host: "GITHUB.COM".to_string(),
            owner: "ACME".to_string(),
            repository: "WIDGET".to_string(),
            number: 42,
            comment_status: crate::project::IssueCommentStatus::Posted,
        });
        app.store.projects[0].features.push(existing);
        app.issue_browser_open_setup();
        let edited_prompt = "Keep this duplicate-warning edit".to_string();
        if let crate::app::AppMode::IssueSetup(state) = &mut app.mode {
            state.prompt = TextEditor::new(edited_prompt.clone());
            state.use_worktree = false;
            state.plan_mode = true;
        }

        app.issue_setup_confirm().unwrap();
        assert!(matches!(
            &app.mode,
            crate::app::AppMode::IssueDuplicateWarning(warning)
                if warning.matches == vec![IssueFeatureMatch {
                    project_name: "widget".to_string(),
                    feature_name: "existing-issue-fix".to_string(),
                }]
        ));
        app.issue_duplicate_cancel();
        assert!(matches!(
            &app.mode,
            crate::app::AppMode::IssueSetup(state) if state.prompt.text() == edited_prompt
        ));

        app.issue_setup_confirm().unwrap();
        app.issue_duplicate_override().unwrap();
        assert!(matches!(
            &app.mode,
            crate::app::AppMode::PlanInterview(interview)
                if interview
                    .pending_launch
                    .as_ref()
                    .is_some_and(|prepared| prepared.issue_source.as_ref().is_some_and(|source| source.number == 42))
        ));
    }

    #[test]
    fn failed_feature_store_write_rolls_back_issue_row_for_retry() {
        let repo = tempfile::tempdir().unwrap();
        let mut app = app_with_issue_browser(repo.path());
        app.mode = crate::app::AppMode::Normal;
        // A directory cannot be replaced by ProjectStore::save's file write.
        app.store_path = repo.path().to_path_buf();
        let prepared = crate::app::PreparedFeatureLaunch {
            project_name: "widget".to_string(),
            feature_name: Some("fix-issue-42".to_string()),
            branch: "issue/42".to_string(),
            workdir: repo.path().to_path_buf(),
            is_worktree: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            quick_plan: false,
            agent: AgentKind::Claude,
            create_terminal: false,
            session_name: "Claude 1".to_string(),
            enable_chrome: false,
            remote_control: false,
            steering_enabled: false,
            hook_succeeded: None,
            startup_prompt: Some("edited prompt".to_string()),
            todo_origin: None,
            issue_source: Some(crate::project::IssueSource {
                host: "github.com".to_string(),
                owner: "acme".to_string(),
                repository: "widget".to_string(),
                number: 42,
                comment_status: crate::project::IssueCommentStatus::Pending,
            }),
        };

        let error = app
            .finish_feature_launch_without_interview(prepared)
            .unwrap_err();
        assert!(error.to_string().contains("feature was not saved"));
        assert!(app.store.projects[0].features.is_empty());
    }

    #[test]
    fn finalizing_a_pending_issue_feature_comments_on_it_not_on_a_later_feature() {
        let repo = tempfile::tempdir().unwrap();
        let project = crate::project::Project {
            id: "project-1".to_string(),
            name: "widget".to_string(),
            repo: repo.path().to_path_buf(),
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent: AgentKind::Claude,
            is_git: true,
        };
        let store = crate::project::ProjectStore {
            version: crate::project::CURRENT_PROJECT_STORE_VERSION,
            projects: vec![project],
            session_bookmarks: Vec::new(),
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: std::collections::HashMap::new(),
        };
        // Forces `check_start_preconditions` down its `NeedsConfirm` path (via
        // the memory half of the gate, independent of any harness session
        // count) so `autostart_allowed` returns false and the launch never
        // reaches `ensure_feature_running` — this test is only about which
        // feature the comment gets queued against, not about starting agents.
        let mut tmux = crate::traits::MockTmuxOps::new();
        tmux.expect_list_panes().return_const(Vec::new());
        let mut app = App::new_for_test(
            store,
            Box::new(tmux),
            Box::new(crate::traits::MockWorktreeOps::new()),
        );
        app.config.low_memory_warn_mb = u64::MAX;
        app.mode = crate::app::AppMode::Normal;

        // The issue-fixer feature was queued first, as a pending-worktree-script
        // row awaiting its hook...
        let mut pending = crate::project::Feature::new(
            "fix-issue-42".to_string(),
            "issue/42".to_string(),
            repo.path().join("fix-issue-42"),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        pending.pending_worktree_script = true;
        let pending_id = pending.id.clone();
        app.store.projects[0].features.push(pending);

        // ...but another, unrelated feature was created and appended after it
        // before this launch was finalized. Locating "the last feature in the
        // project" here would find this one instead of the pending row.
        let mut other = crate::project::Feature::new(
            "unrelated".to_string(),
            "unrelated".to_string(),
            repo.path().join("unrelated"),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        other.issue_source = Some(crate::project::IssueSource {
            host: "github.com".to_string(),
            owner: "acme".to_string(),
            repository: "widget".to_string(),
            number: 99,
            comment_status: crate::project::IssueCommentStatus::Pending,
        });
        app.store.projects[0].features.push(other);

        let prepared = crate::app::PreparedFeatureLaunch {
            project_name: "widget".to_string(),
            feature_name: Some("fix-issue-42".to_string()),
            branch: "issue/42".to_string(),
            workdir: repo.path().join("fix-issue-42"),
            is_worktree: true,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            quick_plan: false,
            agent: AgentKind::Claude,
            create_terminal: false,
            session_name: "Claude 1".to_string(),
            enable_chrome: false,
            remote_control: false,
            steering_enabled: false,
            hook_succeeded: None,
            startup_prompt: None,
            todo_origin: None,
            issue_source: Some(issue_source(crate::project::IssueCommentStatus::Pending)),
        };

        app.finish_feature_launch_without_interview(prepared)
            .unwrap();

        assert_eq!(app.store.projects[0].features.len(), 2);
        assert_eq!(app.store.projects[0].features[0].id, pending_id);
        assert!(matches!(
            app.selection,
            crate::app::Selection::Feature(0, 0)
        ));

        // This goes through the real `gh` transport (network/auth calls),
        // unlike the other tests here that inject a `FakeTransport`, so it is
        // given much more patience than `wait_for_comment_result`'s default.
        let mut result = None;
        for _ in 0..2000 {
            if let Some(r) = app.issue_comment_work.poll().into_iter().next() {
                result = Some(r);
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        let result = result.expect("comment worker did not finish");
        assert_eq!(
            result.feature_id, pending_id,
            "the comment must be queued for the feature the issue was created for, not whatever feature happens to be last in the project"
        );
        assert_eq!(result.source.number, 42);

        // The unrelated feature's own issue association is untouched.
        assert_eq!(
            app.store.projects[0].features[1]
                .issue_source
                .as_ref()
                .map(|source| source.number),
            Some(99)
        );
    }

    #[test]
    fn cancelling_issue_setup_has_no_creation_or_comment_side_effects() {
        let repo = tempfile::tempdir().unwrap();
        let before = std::fs::read_dir(repo.path()).unwrap().count();
        let mut app = app_with_issue_browser(repo.path());
        app.issue_browser_open_setup();
        if let crate::app::AppMode::IssueSetup(state) = &mut app.mode {
            state.feature_name = "edited-but-cancelled".to_string();
            state.settings.branch = "issue/42-cancelled".to_string();
            state.prompt = TextEditor::new("edited prompt that must not be sent".to_string());
        } else {
            panic!("issue setup did not open");
        }

        crate::handlers::handle_issue_setup_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .unwrap();

        assert!(matches!(app.mode, crate::app::AppMode::IssueBrowser(_)));
        assert!(app.store.projects[0].features.is_empty());
        assert!(!app.issue_comment_work.pending());
        assert_eq!(std::fs::read_dir(repo.path()).unwrap().count(), before);

        crate::handlers::handle_issue_browser_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        )
        .unwrap();
        assert!(matches!(app.mode, crate::app::AppMode::Normal));
        assert!(app.store.projects[0].features.is_empty());
    }

    #[test]
    fn dashboard_g_routes_the_selected_project_to_issue_repository_resolution() {
        let repo = tempfile::tempdir().unwrap();
        let mut app = app_with_issue_browser(repo.path());
        app.mode = crate::app::AppMode::Normal;
        app.selection = crate::app::Selection::Project(0);

        crate::handlers::handle_normal_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE),
        )
        .unwrap();

        assert!(matches!(app.mode, crate::app::AppMode::Normal));
        assert!(
            app.message
                .as_deref()
                .is_some_and(|message| message.starts_with("Error:")),
            "the g binding should attempt project-scoped repository resolution"
        );
        assert!(app.store.projects[0].features.is_empty());
        assert!(!app.issue_comment_work.pending());
    }
}
