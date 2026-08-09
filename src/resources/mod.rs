//! Host resource probes backing AMF's soft agent limits: how much memory is
//! actually available before another harness is started, and how many
//! harnesses are already running.
//!
//! Everything here is read-only and best-effort. A probe that cannot get a
//! trustworthy answer returns `None` rather than guessing, and callers treat
//! `None` as "no gate" — a missing signal must never block a start.

pub mod doctor;
pub mod limits;
pub mod mem;
pub mod procs;
