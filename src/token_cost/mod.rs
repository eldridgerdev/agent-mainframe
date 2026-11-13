//! Accurate per-call token cost, independent of the sidebar's cost code.
//!
//! The sidebar's `cost_usd`/`cost_equivalent_tokens` price a whole session at
//! one global rate (issue #863). This module prices one call at a time, from
//! the call's own model, so the sidebar can adopt it later. Pure library code:
//! no `App`, no persistence, no UI.

// Nothing consumes this yet: the Design Review ledger (Phase 2+) will.
#![allow(dead_code)]

pub mod claude_usage;
pub mod cost;
pub mod ledger;
pub mod pricing;
