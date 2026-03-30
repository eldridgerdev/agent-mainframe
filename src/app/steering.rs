#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptConstraint {
    FileScope,
    AcceptanceCriteria,
    Invariants,
    ValidationCommands,
    Risks,
}

impl PromptConstraint {
    pub const ALL: [PromptConstraint; 5] = [
        PromptConstraint::FileScope,
        PromptConstraint::AcceptanceCriteria,
        PromptConstraint::Invariants,
        PromptConstraint::ValidationCommands,
        PromptConstraint::Risks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            PromptConstraint::FileScope => "File scope",
            PromptConstraint::AcceptanceCriteria => "Acceptance criteria",
            PromptConstraint::Invariants => "Invariants",
            PromptConstraint::ValidationCommands => "Validation commands",
            PromptConstraint::Risks => "Risks",
        }
    }

    pub fn missing_explanation(self) -> &'static str {
        match self {
            PromptConstraint::FileScope => {
                "The prompt does not clearly name what files or directories are in scope."
            }
            PromptConstraint::AcceptanceCriteria => {
                "The prompt does not say what \"done\" looks like."
            }
            PromptConstraint::Invariants => {
                "The prompt does not pin down behavior that must stay unchanged."
            }
            PromptConstraint::ValidationCommands => {
                "The prompt does not tell the agent how to verify the change."
            }
            PromptConstraint::Risks => {
                "The prompt does not call out sharp edges or likely regressions."
            }
        }
    }

    pub fn teaching_tip(self) -> &'static str {
        match self {
            PromptConstraint::FileScope => {
                "Add a line like: `Touch only src/app/... and src/ui/...; leave handlers unchanged unless needed.`"
            }
            PromptConstraint::AcceptanceCriteria => {
                "Add a line like: `Done when the create-feature flow shows coaching before launch and still reaches cargo check.`"
            }
            PromptConstraint::Invariants => {
                "Add a line like: `Keep the existing command/dialog flow and do not bypass the current create_feature path.`"
            }
            PromptConstraint::ValidationCommands => {
                "Add an explicit command like: `Run cargo check before stopping.`"
            }
            PromptConstraint::Risks => {
                "Add a watch-out like: `Be careful not to break SuperVibe confirmation or session launch defaults.`"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCheck {
    pub constraint: PromptConstraint,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAnalysis {
    pub score: u8,
    pub max_score: u8,
    pub summary: String,
    pub checks: Vec<PromptCheck>,
    pub teaching_tips: Vec<String>,
}

impl PromptAnalysis {
    pub fn present_checks(&self) -> impl Iterator<Item = &PromptCheck> {
        self.checks.iter().filter(|check| check.present)
    }

    pub fn missing_checks(&self) -> impl Iterator<Item = &PromptCheck> {
        self.checks.iter().filter(|check| !check.present)
    }
}

pub fn analyze_prompt(prompt: &str) -> PromptAnalysis {
    let trimmed = prompt.trim();
    let lowercase = trimmed.to_lowercase();

    let checks = PromptConstraint::ALL
        .iter()
        .copied()
        .map(|constraint| PromptCheck {
            constraint,
            present: check_constraint(constraint, trimmed, &lowercase),
        })
        .collect::<Vec<_>>();

    let score = checks.iter().filter(|check| check.present).count() as u8 * 2;
    let max_score = (PromptConstraint::ALL.len() as u8) * 2;
    let missing = checks.iter().filter(|check| !check.present).count();

    let summary = if trimmed.is_empty() {
        "Start with the concrete task, then add scope, success criteria, validation, and watch-outs before launch."
            .to_string()
    } else if missing == 0 {
        "Strong steering draft. The prompt names the work, the boundaries, and how to validate it."
            .to_string()
    } else if missing <= 2 {
        "Usable draft. Add the missing constraints so the agent does less guessing once it starts."
            .to_string()
    } else {
        "Thin draft. Add missing constraints now so the first agent pass is less likely to wander or regress behavior."
            .to_string()
    };

    let teaching_tips = checks
        .iter()
        .filter(|check| !check.present)
        .map(|check| check.constraint.teaching_tip().to_string())
        .take(3)
        .collect();

    PromptAnalysis {
        score,
        max_score,
        summary,
        checks,
        teaching_tips,
    }
}

fn check_constraint(constraint: PromptConstraint, prompt: &str, lowercase: &str) -> bool {
    match constraint {
        PromptConstraint::FileScope => has_file_scope(prompt, lowercase),
        PromptConstraint::AcceptanceCriteria => has_acceptance_criteria(lowercase),
        PromptConstraint::Invariants => has_invariants(lowercase),
        PromptConstraint::ValidationCommands => has_validation_commands(prompt, lowercase),
        PromptConstraint::Risks => has_risks(lowercase),
    }
}

fn has_file_scope(prompt: &str, lowercase: &str) -> bool {
    if contains_any(
        lowercase,
        &[
            "file scope",
            "touch only",
            "only touch",
            "only edit",
            "limit changes to",
            "keep changes in",
            "in scope",
            "out of scope",
        ],
    ) {
        return true;
    }

    prompt.split_whitespace().any(|word| {
        let token = word.trim_matches(|c: char| "`'\",:;()[]{}".contains(c));
        token.contains('/')
            || token.starts_with("src")
            || token.ends_with(".rs")
            || token.ends_with(".toml")
            || token.ends_with(".md")
            || token.ends_with(".json")
            || token.ends_with(".yaml")
            || token.ends_with(".yml")
    })
}

fn has_acceptance_criteria(lowercase: &str) -> bool {
    contains_any(
        lowercase,
        &[
            "acceptance criteria",
            "done when",
            "success looks like",
            "should ",
            "must ",
            "expected result",
            "expected behavior",
            "so that ",
            "when i ",
        ],
    )
}

fn has_invariants(lowercase: &str) -> bool {
    contains_any(
        lowercase,
        &[
            "must not",
            "do not",
            "don't",
            "without changing",
            "preserve",
            "keep existing",
            "leave ",
            "avoid regression",
            "invariant",
            "non-goal",
        ],
    )
}

fn has_validation_commands(prompt: &str, lowercase: &str) -> bool {
    contains_any(
        lowercase,
        &[
            "cargo check",
            "cargo test",
            "cargo clippy",
            "npm test",
            "pnpm test",
            "pytest",
            "go test",
            "just ",
            "run ",
            "verify with",
        ],
    ) || prompt.contains('`')
        && contains_any(
            lowercase,
            &[
                "cargo", "npm", "pnpm", "pytest", "go test", "just", "uv run",
            ],
        )
}

fn has_risks(lowercase: &str) -> bool {
    contains_any(
        lowercase,
        &[
            "risk",
            "risks",
            "watch out",
            "careful",
            "edge case",
            "gotcha",
            "backward compatibility",
            "backwards compatibility",
            "regression",
            "performance",
            "perf",
            "migration",
            "hooks",
            "tmux",
        ],
    )
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
