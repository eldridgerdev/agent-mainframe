use crate::project::AgentKind;

// ---------------------------------------------------------------------------
// Prompt-size estimation
//
// A pre-send soft gate for the batched-review orchestrator: if a rendered
// prompt is estimated to overflow the harness it is split into bounded
// prompts *before* the call rather than after a "prompt too long" failure.
// The estimate is deliberately crude — real tokenizers and context windows
// are not exposed by any of the four CLIs — so it fails safe toward splitting,
// and the adaptive halving in `crate::review_batch` is the hard backstop.
//
// `allow(dead_code)`: the callers are `crate::review_batch` and the review
// call sites; a couple of the entry points here (`will_overflow`,
// `PROMPT_ESTIMATE_BYTES_PER_TOKEN`) have no caller yet.
// ---------------------------------------------------------------------------

/// Bytes per token for the size estimate. GPT/Claude-family tokenizers land
/// near four bytes per token on source and prose; four keeps the estimate a
/// touch pessimistic on dense code.
#[allow(dead_code)]
pub const PROMPT_ESTIMATE_BYTES_PER_TOKEN: usize = 4;

/// Rough token count of `prompt`, rounding up. Uses byte length rather than
/// `chars().count()` on purpose: multibyte text then estimates *higher*, which
/// errs toward splitting instead of toward an over-long prompt.
#[allow(dead_code)]
pub fn estimate_prompt_tokens(prompt: &str) -> usize {
    prompt.len().div_ceil(PROMPT_ESTIMATE_BYTES_PER_TOKEN)
}

/// Conservative ceiling on the *prompt* tokens a single one-shot run for
/// `harness` should carry. Set well under each harness's real context window
/// so there is room for its system prompt, tool schemas, and the response.
/// These are only the built-in defaults; `AppConfig` /
/// `ExtensionConfig::review_prompt_budget_tokens` override them without a
/// rebuild (resolved by `App::review_prompt_budget`).
#[allow(dead_code)]
pub const fn default_prompt_budget_tokens(harness: &AgentKind) -> usize {
    match harness {
        // ~200k-token context; keep a wide margin for tools + output.
        AgentKind::Claude => 128_000,
        // Comparable large context through `codex exec`.
        AgentKind::Codex => 128_000,
        // Model-dependent and frequently smaller — stay further back.
        AgentKind::Opencode => 96_000,
        AgentKind::Pi => 96_000,
    }
}

/// Whether `prompt` should be assumed to overflow `harness` and be split
/// before sending, using [`default_prompt_budget_tokens`]. Callers with a
/// configured budget use [`will_overflow_with_budget`] directly.
#[allow(dead_code)]
pub fn will_overflow(prompt: &str, harness: &AgentKind) -> bool {
    will_overflow_with_budget(prompt, default_prompt_budget_tokens(harness))
}

/// Whether `prompt`'s estimated token count exceeds `budget_tokens`. A budget
/// of `0` disables the gate (never reports overflow) so config can opt out of
/// pre-send splitting and lean entirely on adaptive halving.
#[allow(dead_code)]
pub fn will_overflow_with_budget(prompt: &str, budget_tokens: usize) -> bool {
    budget_tokens != 0 && estimate_prompt_tokens(prompt) > budget_tokens
}

// ---------------------------------------------------------------------------
// Overflow-error classification
//
// The hard backstop behind the soft size gate: when a harness rejects a
// prompt as too long *despite* the estimate clearing the budget, the run
// fails with a typed [`PromptTooLong`] instead of a generic error string so
// `crate::review_batch` can halve that batch and retry rather than
// abandoning coverage. Any failure the classifier does not recognize stays a
// plain `anyhow` error — see AMF_PLAN.md's risk note on unverified CLI
// phrasings.
// ---------------------------------------------------------------------------

/// A one-shot run rejected because the prompt exceeded the model's context
/// window. Carried through `anyhow` so a caller can `downcast` / use
/// [`as_prompt_too_long`] to tell it apart from auth, quota, and network
/// failures, which no amount of splitting would fix.
#[derive(Debug, Clone)]
pub struct PromptTooLong {
    pub harness: AgentKind,
    /// The provider/CLI text that was classified as an overflow, trimmed.
    pub detail: String,
}

impl std::fmt::Display for PromptTooLong {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} rejected the prompt as too long for its context window{}{}",
            self.harness.display_name(),
            if self.detail.is_empty() { "" } else { ": " },
            self.detail
        )
    }
}

impl std::error::Error for PromptTooLong {}

/// Build the typed error for `harness` from the classified `detail` text.
pub fn prompt_too_long_error(harness: &AgentKind, detail: &str) -> anyhow::Error {
    anyhow::Error::new(PromptTooLong {
        harness: harness.clone(),
        detail: detail.trim().to_string(),
    })
}

/// The [`PromptTooLong`] in `err`'s chain, if any — set even when the error
/// has since been wrapped with extra `.context(...)`.
#[allow(dead_code)]
pub fn as_prompt_too_long(err: &anyhow::Error) -> Option<&PromptTooLong> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<PromptTooLong>())
}

/// Best-effort detection of a "prompt exceeds the context window" failure in a
/// harness's error text (stderr on non-zero exit, or a structured event
/// error). Matches the phrasings each CLI/provider is known to emit:
/// Anthropic/Claude Code, OpenAI/Codex, and the assorted providers OpenCode
/// and Pi route to. An unrecognized phrasing falls through and surfaces as an
/// ordinary failure until it is verified against that CLI.
pub fn is_prompt_too_long_message(text: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        // Anthropic / Claude Code
        "prompt is too long",
        "prompt too long",
        // OpenAI / Codex
        "maximum context length",
        "context_length_exceeded",
        "reduce the length of the messages",
        // Generic provider phrasings (OpenCode model routing, Pi)
        "context length exceeded",
        "context window exceeded",
        "exceeds the context window",
        "exceed the context window",
        "input is too long",
        "input too long",
        "too many tokens",
        "too many input tokens",
    ];
    if NEEDLES.iter().any(|needle| lowered.contains(needle)) {
        return true;
    }
    // "... 210000 tokens > 200000 maximum ..." style, where no fixed phrase
    // is present but the shape is unambiguous.
    lowered.contains("tokens >") && lowered.contains("maximum")
}
