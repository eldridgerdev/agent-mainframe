//! Shared project-scoped GitHub workflow context.

use crate::app::App;
use crate::github::{GithubRepository, GithubRepositoryError, resolve_github_repository};

impl App {
    /// Resolve GitHub identity from a selected project's configured repository.
    /// Keeping this lookup project-scoped prevents an issue/PR view opened from
    /// one project from accidentally using the process directory's remote.
    pub(crate) fn github_repository_for_project(
        &self,
        project_index: usize,
    ) -> Result<GithubRepository, GithubRepositoryError> {
        let project = self
            .store
            .projects
            .get(project_index)
            .ok_or(GithubRepositoryError::NotGitRepository)?;
        resolve_github_repository(&project.repo)
    }
}
