//! Pure half of the Design Review packet (Phase 2 of
//! `docs/backlog/design-review-plan.md`): staff roles and model resolution,
//! budgeted prompt-context builders, Mermaid vetting and the mechanical
//! unexplained-change detectors.
//!
//! Nothing here touches `App`, persistence or processes. Phase 2b wires it
//! into staff runs, the artifact cache and the ledger.

// Phase 2b (staff runs, cache, ledger) is the first caller.
#![allow(dead_code)]

pub mod context;
pub mod detectors;
pub mod diagram;
pub mod staff;
