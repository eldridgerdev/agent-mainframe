use super::AiReviewAttribution;

/// Opening of the collapsible usage block, up to where the compact label
/// starts. [`strip_ai_review_attribution`] recognizes the block by it.
pub(super) const AI_REVIEW_USAGE_OPEN: &str = "<details>\n<summary>AI review usage · ";

/// The `<summary>` line is raw HTML, so a model name like `a<b>` would
/// otherwise be parsed as a tag and swallow the rest of the label.
pub(super) fn escape_summary_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn format_elapsed(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1_000;
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

/// `cached_is_additive` distinguishes Anthropic's `cache_read_input_tokens`
/// (a separate count on top of `input_tokens`) from other providers' cache
/// figures, which are commonly already a subset of `input_tokens`; only in
/// the former case does folding `cached_tokens` into `cache_read_tokens` and
/// `total_tokens` avoid double-counting them.
pub(super) fn session_usage_from_counts(
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cached_is_additive: bool,
) -> crate::token_tracking::SessionTokenUsage {
    let additive_cached = if cached_is_additive { cached_tokens } else { 0 };
    crate::token_tracking::SessionTokenUsage {
        // Only the token counts feed `format_token_cost`; the source label is
        // never read for pricing.
        source: crate::token_tracking::TokenUsageSource {
            provider: crate::token_tracking::TokenUsageProvider::Claude,
            id: "ai-review".to_string(),
        },
        input_tokens,
        output_tokens,
        cache_read_tokens: additive_cached,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: input_tokens
            .saturating_add(output_tokens)
            .saturating_add(additive_cached),
    }
}

/// Attribution for an AI review finding or summary posted to GitHub: the
/// stable `— AI review via AMF` marker, preceded by a
/// [`AiReviewAttribution::usage_summary`] (model, elapsed time, tokens, and
/// estimated cost) when a run's provenance is available. Legacy callers with
/// no attribution still get the bare marker.
pub(super) fn append_ai_review_attribution(
    body: &str,
    attribution: Option<&AiReviewAttribution>,
) -> String {
    match attribution {
        Some(attribution) => format!(
            "{}\n\n{}\n\n{}",
            body.trim_end(),
            attribution.usage_summary(),
            crate::app::pr_review::AI_REVIEW_ATTRIBUTION_FOOTER
        ),
        None => format!(
            "{}\n\n{}",
            body.trim_end(),
            crate::app::pr_review::AI_REVIEW_ATTRIBUTION_FOOTER
        ),
    }
}

/// Guarantee the attribution survives into the posted body even though the
/// confirm dialog's summary editor is free-form text the user can edit —
/// including deleting the disclosure line and footer [`build_ai_review`]
/// seeded it with. Called right before [`GhCli::create_review`] rather than
/// trusted from dialog build time, so an edited-out attribution is restored
/// instead of silently publishing an unattributed review. Any existing
/// trailing marker (and a recognized disclosure line above it) is stripped
/// first so the attribution is never doubled.
pub(super) fn ensure_ai_review_attribution(
    body: &str,
    attribution: Option<&AiReviewAttribution>,
) -> String {
    append_ai_review_attribution(strip_ai_review_attribution(body), attribution)
}

/// Remove a trailing [`AI_REVIEW_ATTRIBUTION_FOOTER`] and, if present directly
/// above it, a disclosure line matching `attribution` — leaving the editable
/// core body.
pub(super) fn strip_ai_review_attribution(body: &str) -> &str {
    let footer = crate::app::pr_review::AI_REVIEW_ATTRIBUTION_FOOTER;
    let trimmed = body.trim_end();
    let core = trimmed.strip_suffix(footer).map_or(trimmed, str::trim_end);
    // The usage block is always the trailing `<details>` element, opened
    // after a blank line (see `append_ai_review_attribution`), so only an
    // opening that starts a paragraph *and* whose element runs to the end of
    // the body counts as the real block — not one a finding's own text merely
    // mentions or quotes somewhere earlier.
    if !core.ends_with("</details>") {
        return core;
    }
    // An empty summary seeds a body that is *only* the usage block, so it
    // opens at the very start rather than after a blank line.
    let block_start = core
        .rfind(&format!("\n\n{AI_REVIEW_USAGE_OPEN}"))
        .map(|i| i + 2)
        .or_else(|| core.starts_with(AI_REVIEW_USAGE_OPEN).then_some(0));
    match block_start {
        Some(start) if !core[start..].contains("\n</details>\n") => core[..start].trim_end(),
        _ => core,
    }
}
