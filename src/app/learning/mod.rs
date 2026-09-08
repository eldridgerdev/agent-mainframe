//! Learning sessions, browsing, question workers and follow-up workflows.
//!
//! Lifecycle owns opening, closing and persistence; navigation owns file and
//! selection transforms; workers owns questions/results; follow_up owns TODO
//! and agent-session handoffs. Display state remains owned by AppMode.
//!
//! Browsing never writes source files. Session/Q&A history is persisted through
//! the Learning DB module; follow-ups delegate to existing TODO/session APIs.
#![allow(dead_code)]

mod follow_up;
mod lifecycle;
mod navigation;
mod workers;

#[cfg(test)]
pub(crate) mod tests;
pub use workers::{LearningAnswer, STARTER_QUESTIONS};

pub(crate) mod state;

pub(crate) mod runtime;
