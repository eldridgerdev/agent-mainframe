//! Learning sessions, browsing, question workers and follow-up workflows.
//!
//! Lifecycle owns opening, closing and persistence; navigation owns file and
//! selection transforms; workers owns questions/results; follow_up owns TODO
//! and agent-session handoffs. Display state remains owned by AppMode.
//!
//! Browsing never writes source files. Session/Q&A history is persisted through
//! the Learning DB module; follow-ups delegate to existing TODO/session APIs.

mod follow_up;
pub(crate) use follow_up::{escalation_seed, learning_session_label, todo_body, todo_title_seed};
mod lifecycle;
mod navigation;
pub(crate) use navigation::hunk_span;
mod workers;
pub(crate) use workers::starter_questions_for;

#[cfg(test)]
pub(crate) mod tests;
pub use workers::{LearningAnswer, STARTER_QUESTIONS};

pub(crate) mod state;

pub(crate) mod runtime;
