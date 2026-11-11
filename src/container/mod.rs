//! Container sandbox for projects cloned from an untrusted URL.
//!
//! A project with `containerized = true` never runs its code on the host: the
//! clone, every agent and terminal session, and the egress proxy all run in
//! containers built from one image, with only the project's checkout mounted.
//! See `docs/container-projects.md` for the threat model and its limits.
//!
//! Everything here that builds a command line is pure (returns an argv) so it
//! can be tested without Docker; only [`image`] and the callers spawn.

pub mod args;
pub mod egress;
pub mod image;
pub mod runtime;
pub mod settings;
pub mod tmux;

pub use settings::ContainerSettings;
