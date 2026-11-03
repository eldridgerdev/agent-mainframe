//! Plain-language GitHub failures for the GUI's PR panels.
//!
//! PR Triage, PR Review and PR Workspace read GitHub through the `gh` CLI,
//! which reports failures only as text. [`classify_github_failure`] sorts that
//! text into the few causes a user can fix themselves (no `gh`, not signed in,
//! rate limited, offline, no GitHub remote), and [`GithubProblem`] carries the
//! cause to the frontend as a typed kind with the next step, so a panel can
//! show "run `gh auth login`, then Retry" instead of `gh`'s raw stderr.
//!
//! Anything unrecognised is `None`: the caller keeps showing the original
//! message, so a classification gap never hides what went wrong.

use serde::Serialize;

use crate::github::redact_secrets;

/// A GitHub failure the user can fix, sorted by what they need to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubProblemKind {
    /// `gh` is not installed, or could not be started.
    CliMissing,
    /// `gh` is installed but not signed in, or its token was rejected.
    NotSignedIn,
    /// GitHub's API rate limit was reached.
    RateLimited,
    /// GitHub could not be reached over the network.
    Offline,
    /// The project has no GitHub remote for `gh` to read.
    NoRemote,
}

/// What a PR panel shows for a classified GitHub failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GithubProblem {
    pub kind: GithubProblemKind,
    /// One-line headline: what is wrong.
    pub title: &'static str,
    /// What to do about it, in plain words.
    pub message: &'static str,
    /// A command to run in a terminal, when there is one.
    pub command: Option<&'static str>,
    /// Where to get help or the missing tool, when there is one.
    pub link: Option<&'static str>,
    /// The original failure with any credential removed, for "Details".
    pub detail: String,
}

impl GithubProblemKind {
    pub fn title(self) -> &'static str {
        match self {
            Self::CliMissing => "The GitHub CLI (gh) is not installed",
            Self::NotSignedIn => "The GitHub CLI (gh) is not signed in",
            Self::RateLimited => "GitHub's rate limit was reached",
            Self::Offline => "GitHub could not be reached",
            Self::NoRemote => "This project is not linked to GitHub",
        }
    }

    /// What to do, without "then press Retry": a panel adds that only when it
    /// offers Retry, and [`GithubProblem::summary`] says "try again" instead.
    /// A message ending in `:` is followed by [`Self::command`].
    pub fn message(self) -> &'static str {
        match self {
            Self::CliMissing => {
                "Pull requests are read with the GitHub CLI (gh), and AMF could not run it. \
                 Install it from cli.github.com."
            }
            Self::NotSignedIn => {
                "Pull requests are read with the GitHub CLI (gh). It is installed but not signed in \
                 to GitHub. Run this in a terminal:"
            }
            Self::RateLimited => {
                "GitHub is limiting requests from your account for a while. Wait a few minutes."
            }
            Self::Offline => "Check your internet connection, VPN or proxy.",
            Self::NoRemote => {
                "Pull requests need a GitHub remote on this repository. Add one in a terminal:"
            }
        }
    }

    pub fn command(self) -> Option<&'static str> {
        match self {
            Self::NotSignedIn => Some("gh auth login"),
            Self::NoRemote => Some("git remote add origin https://github.com/OWNER/REPO.git"),
            Self::CliMissing | Self::RateLimited | Self::Offline => None,
        }
    }

    pub fn link(self) -> Option<&'static str> {
        match self {
            Self::CliMissing => Some("https://cli.github.com"),
            _ => None,
        }
    }
}

const NETWORK_NEEDLES: &[&str] = &[
    "could not resolve host",
    "network is unreachable",
    "no route to host",
    "connection refused",
    "connection reset",
    "i/o timeout",
    "timed out",
    "dial tcp",
    "tls handshake",
    "error connecting to",
];

/// Sort a GitHub failure's text into a cause the user can fix. Pure and
/// heuristic: `gh` has no machine-readable error codes. Every rule but the
/// first two needs the text to mention `gh` or GitHub, so an unrelated
/// failure that happens to say "timed out" or "rate limit" (an agent's
/// own limit, say) is never blamed on GitHub.
pub fn classify_github_failure(text: &str) -> Option<GithubProblemKind> {
    let lower = text.to_ascii_lowercase();
    // `.context("Failed to run `gh ...`")` wraps only a spawn failure: the
    // binary is missing or not executable.
    if lower.contains("failed to run `gh")
        || lower.contains("github cli (`gh`) not found")
        || lower.contains("gh: command not found")
        || lower.contains("gh: not found")
    {
        return Some(GithubProblemKind::CliMissing);
    }
    // Checked before sign-in: gh's own no-remote message ends with "use
    // `gh auth login`", which would otherwise read as a sign-in problem.
    if lower.contains("no git remotes found")
        || lower.contains("none of the git remotes")
        || lower.contains("no github remote")
    {
        return Some(GithubProblemKind::NoRemote);
    }
    if !mentions_github(&lower) {
        return None;
    }
    if lower.contains("gh auth login")
        || lower.contains("not authenticated")
        || lower.contains("not logged in")
        || lower.contains("bad credentials")
        || lower.contains("http 401")
        || lower.contains("requires authentication")
    {
        return Some(GithubProblemKind::NotSignedIn);
    }
    if lower.contains("rate limit") {
        return Some(GithubProblemKind::RateLimited);
    }
    if NETWORK_NEEDLES.iter().any(|needle| lower.contains(needle)) {
        return Some(GithubProblemKind::Offline);
    }
    None
}

/// Whether `lower` talks about `gh` or GitHub, not merely about a file whose
/// path happens to contain the word: every error in the GUI is classified, and
/// a worktree under `~/github/` or a user named `gh` must not turn an agent's
/// "not logged in" into a GitHub sign-in problem. A path token counts only when
/// it names a GitHub host. `gh` must be a whole word, so "through" or "high"
/// never count.
fn mentions_github(lower: &str) -> bool {
    lower.split_whitespace().any(|token| {
        if token.contains('/') {
            return token.contains("github.com");
        }
        token.contains("github")
            || token.contains("graphql")
            || token
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == "gh")
    })
}

impl GithubProblem {
    /// One self-contained sentence for surfaces that show only
    /// [`GuiError::message`](crate::gui_contract::GuiError): the headline,
    /// the step with its command inline, and no reference to a Retry button
    /// that surface may not have.
    pub fn summary(&self) -> String {
        match self.command {
            Some(command) => format!(
                "{}. {} {command}, then try again.",
                self.title, self.message
            ),
            None => format!("{}. {} Then try again.", self.title, self.message),
        }
    }
}

/// The panel view of `text`, when it is a GitHub failure the user can fix.
pub fn github_problem(text: &str) -> Option<GithubProblem> {
    classify_github_failure(text).map(|kind| GithubProblem {
        kind,
        title: kind.title(),
        message: kind.message(),
        command: kind.command(),
        link: kind.link(),
        detail: redact_secrets(text.trim()),
    })
}

/// [`github_problem`] over an error's whole cause chain. `gh` spawn
/// failures say what went wrong only in the context, and the OS error only
/// in the source, so the outermost message alone is not enough.
pub fn github_problem_of(error: &anyhow::Error) -> Option<GithubProblem> {
    github_problem(&format!("{error:#}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use GithubProblemKind as K;

    #[test]
    fn a_missing_gh_binary_is_cli_missing() {
        for text in [
            "Failed to run `gh pr list`.: No such file or directory (os error 2)",
            "Failed to run `gh`.",
            "GitHub CLI (`gh`) not found. Install it from https://cli.github.com to use PR Triage.",
            "sh: gh: command not found",
        ] {
            assert_eq!(classify_github_failure(text), Some(K::CliMissing), "{text}");
        }
    }

    #[test]
    fn sign_in_failures_are_not_signed_in() {
        for text in [
            "`gh` is not authenticated. Run `! gh auth login` to sign in (login is interactive).",
            "`gh pr list` failed: To get started with GitHub CLI, please run:  gh auth login",
            "`gh api user` failed: HTTP 401: Bad credentials (https://api.github.com/user)",
            "Could not load PR #5: gh: Requires authentication",
        ] {
            assert_eq!(
                classify_github_failure(text),
                Some(K::NotSignedIn),
                "{text}"
            );
        }
    }

    #[test]
    fn no_remote_wins_over_its_own_gh_auth_login_hint() {
        let text = "`gh pr view` failed: none of the git remotes configured for this repository \
                    point to a known GitHub host. To tell gh about a new GitHub host, please use \
                    `gh auth login`";
        assert_eq!(classify_github_failure(text), Some(K::NoRemote));
        assert_eq!(
            classify_github_failure(
                "No GitHub remote found for this repository. PR Triage needs a GitHub-hosted repo."
            ),
            Some(K::NoRemote)
        );
        assert_eq!(
            classify_github_failure("`gh pr list` failed: no git remotes found"),
            Some(K::NoRemote)
        );
    }

    #[test]
    fn rate_limits_and_network_failures_need_a_github_mention() {
        assert_eq!(
            classify_github_failure("`gh api graphql` failed: GraphQL: API rate limit exceeded"),
            Some(K::RateLimited)
        );
        assert_eq!(
            classify_github_failure(
                "`gh pr list` failed: error connecting to api.github.com\ncheck your internet connection"
            ),
            Some(K::Offline)
        );
        assert_eq!(
            classify_github_failure(
                "Could not load PR #3: dial tcp: lookup api.github.com: i/o timeout"
            ),
            Some(K::Offline)
        );
        // An agent's own limit or a local timeout is not GitHub's.
        assert_eq!(
            classify_github_failure("claude: rate limit reached, retry later"),
            None
        );
        assert_eq!(classify_github_failure("tmux send-keys timed out"), None);
        // "gh" inside another word is not a mention of the CLI.
        assert_eq!(
            classify_github_failure("connection reset while reading through the high-water mark"),
            None
        );
    }

    #[test]
    fn unrecognised_failures_are_left_alone() {
        for text in [
            "`gh pr view` failed: no pull requests found for branch \"x\"",
            "Could not load PR #9: GraphQL: Could not resolve to a PullRequest with the number of 9.",
            "PR Triage is no longer open",
            "",
        ] {
            assert_eq!(classify_github_failure(text), None, "{text}");
        }
    }

    #[test]
    fn the_problem_names_the_fix_and_redacts_the_detail() {
        let problem = github_problem(
            "`gh api user` failed: HTTP 401: Bad credentials token=ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        )
        .unwrap();
        assert_eq!(problem.kind, K::NotSignedIn);
        assert_eq!(problem.command, Some("gh auth login"));
        assert!(!problem.message.contains("Retry"), "the panel adds Retry");
        assert_eq!(
            problem.summary(),
            "The GitHub CLI (gh) is not signed in. Pull requests are read with the GitHub CLI \
             (gh). It is installed but not signed in to GitHub. Run this in a terminal: \
             gh auth login, then try again."
        );
        assert!(
            !problem
                .detail
                .contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789")
        );
        let missing = github_problem("Failed to run `gh pr list`.").unwrap();
        assert_eq!(missing.link, Some("https://cli.github.com"));
        assert_eq!(missing.command, None);
        assert!(
            missing
                .summary()
                .ends_with("Install it from cli.github.com. Then try again.")
        );
    }

    #[test]
    fn a_path_that_contains_github_is_not_a_mention_of_github() {
        // An agent's own sign-in failure in a worktree under ~/github/.
        assert_eq!(
            classify_github_failure("claude: not logged in (cwd /home/u/github/app)"),
            None
        );
        assert_eq!(
            classify_github_failure("connection refused: /home/gh/socket"),
            None
        );
        // A GitHub URL still counts.
        assert_eq!(
            classify_github_failure(
                "request failed: HTTP 401: Bad credentials (https://api.github.com/user)"
            ),
            Some(K::NotSignedIn)
        );
    }

    #[test]
    fn the_cause_chain_is_classified_not_just_the_outer_message() {
        let error = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::NotFound))
            .context("Failed to run `gh pr list`.")
            .context("Could not list pull requests");
        assert_eq!(
            github_problem_of(&error).map(|p| p.kind),
            Some(K::CliMissing)
        );
    }

    #[test]
    fn kinds_serialize_in_snake_case() {
        assert_eq!(
            serde_json::to_string(&K::NotSignedIn).unwrap(),
            "\"not_signed_in\""
        );
        assert_eq!(
            serde_json::to_string(&K::CliMissing).unwrap(),
            "\"cli_missing\""
        );
    }
}
