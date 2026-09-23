//! Shared library behind the `amf` binary (`src/main.rs` -> `cli::run`).
//!
//! The crate is split into a library and a thin binary so another front end
//! (the planned desktop GUI) can link AMF's workspace, session, and TODO
//! logic directly. Only `cli` is `pub` today. Everything else stays a plain
//! `mod`, which keeps it fully visible within this crate without changing
//! its *effective* visibility from what it was as part of the `amf` binary:
//! widening a module to `pub mod` pulls its whole public surface into
//! clippy's API-hygiene lints (`should_implement_trait` and friends) even
//! where nothing outside the crate calls it yet. Widen deliberately, module
//! by module, as a consumer needs it.

pub mod cli;

mod app;
mod automation;
mod claude;
mod codex;
mod codex_config;
mod context_collectors;
mod context_display;
mod context_tracking;
mod custom_session_icons;
mod db;
mod debug;
mod diff;
mod diff_split;
mod editor;
mod extension;
mod fswatch;
mod github;
mod handlers;
mod headless;
mod highlight;
mod hook_payload;
mod http_client;
mod ipc;
mod markdown;
mod perf;
mod pi;
mod plan_interview;
mod project;
mod prompt_library;
mod prompts;
mod resources;
mod review_batch;
mod summary;
mod theme;
mod tmux;
mod tmux_observer;
mod token_tracking;
mod traits;
mod transcript;
mod ui;
mod upgrade;
mod usage;
mod worddiff;
mod worktree;
