use crate::{
    app::PrReviewState,
    app::pr_review::{CommentKind, PrComment, PrSortMode, strip_bot_boilerplate},
    project::AgentKind,
};

use super::{
    PrCommentView, PrInvestigationTurnView, PrInvestigationView, PrReviewView, PrThreadReplyView,
    SORTS,
};

fn sort_key(mode: PrSortMode) -> &'static str {
    SORTS
        .iter()
        .find(|(m, _)| *m == mode)
        .map_or("fetch_order", |(_, k)| k)
}

pub(super) fn review_view(
    state: &PrReviewState,
    investigating: Option<(u64, AgentKind)>,
) -> PrReviewView {
    let all = &state.review.comments;
    let head = &state.review.pr.head_sha;
    let comment_view = |c: &PrComment| PrCommentView {
        id: c.id,
        kind: match c.kind {
            CommentKind::Inline => "inline",
            CommentKind::ReviewSummary { .. } => "review_summary",
            CommentKind::Conversation => "conversation",
        },
        review_state: match &c.kind {
            CommentKind::ReviewSummary { state } => Some(state.clone()),
            _ => None,
        },
        author: c.author.clone(),
        is_bot: c.is_bot,
        path: c.path.clone(),
        line: c.line,
        side: c.side.clone(),
        outdated: c.outdated,
        file_level: c.file_level,
        body: if c.is_bot {
            strip_bot_boilerplate(&c.body)
        } else {
            c.body.clone()
        },
        snippet: c.snippet.clone(),
        hunk: c.prompt_hunk().map(|h| h.into_owned()),
        resolved: c.is_resolved,
        can_resolve: c.thread_id.is_some() && !c.is_local_finding(),
        triage: c.triage.as_db_str(),
        local_note: c.local_note.clone(),
        actionable: c.is_actionable(),
        local_finding: c.is_local_finding(),
        replies: c
            .replies_in(all)
            .into_iter()
            .map(|r| PrThreadReplyView {
                id: r.id,
                author: r.author.clone(),
                body: r.body.clone(),
                via_amf: r.is_amf_followup_reply(),
            })
            .collect(),
        investigation: state
            .investigations
            .iter()
            .find(|r| r.comment_id == c.id)
            .map(|r| PrInvestigationView {
                status: r.status.as_db_str(),
                harness: r.harness.clone(),
                answer: r.answer.clone(),
                error: r.error.clone(),
                follow_ups: r
                    .follow_ups
                    .iter()
                    .map(|t| PrInvestigationTurnView {
                        question: t.question.clone(),
                        answer: t.answer.clone(),
                        harness: t.harness.clone(),
                    })
                    .collect(),
                stale_head: !r.head_sha.is_empty() && &r.head_sha != head,
                kept_todo: None,
            }),
    };
    PrReviewView {
        number: state.review.pr.number,
        url: state.review.pr.url.clone(),
        head_sha: state.review.pr.head_sha.clone(),
        head_ref: state.review.pr.head_ref.clone(),
        branch_mismatch: state.branch_mismatch().map(str::to_string),
        fetched_at: state.review.fetched_at.format("%Y-%m-%d %H:%M").to_string(),
        open_count: state.review.open_count(),
        // Collated AMF replies render under their root, not as rows.
        total: all
            .iter()
            .filter(|c| !state.review.is_collated_amf_reply(c))
            .count(),
        hide_resolved: state.hide_resolved,
        sort: sort_key(state.sort_mode),
        hidden_resolved: state.hidden_resolved_count(),
        conversation_start: state.conversation_section_start(),
        comments: state
            .visible_indices()
            .into_iter()
            .map(|i| comment_view(&all[i]))
            .collect(),
        investigating: investigating.as_ref().map(|(id, _)| *id),
        investigating_harness: investigating.map(|(_, h)| h),
    }
}
