//! Shared library behind AMF's front ends: the `amf` TUI binary
//! (`src/main.rs` -> `cli::run`) and the desktop GUI.
//!
//! Only `cli` and the modules the GUI calls are `pub`: `automation` and
//! `project` for request/response types, and the `gui_*` contract modules,
//! which wrap the same `App` operations the TUI uses. Everything else stays a
//! plain `mod`, which keeps it fully visible within this crate without
//! changing its *effective* visibility from what it was as part of the `amf`
//! binary: widening a module to `pub mod` pulls its whole public surface into
//! clippy's API-hygiene lints (`should_implement_trait` and friends) even
//! where nothing outside the crate calls it yet. Widen deliberately, module
//! by module, as a consumer needs it.

pub mod automation;
pub mod cli;
pub mod gui_bookmarks;
pub mod gui_contract;
pub mod gui_debug_log;
pub mod gui_diff;
pub mod gui_dormancy;
pub mod gui_learning;
pub mod gui_plans;
pub mod gui_pr_review;
pub mod gui_pr_triage;
pub mod gui_prompt_overrides;
pub mod gui_prompts;
pub mod gui_review;
pub mod gui_sessions;
pub mod gui_settings;
pub mod gui_supervised_edits;
pub mod gui_syntax;
pub mod gui_terminal;
pub mod gui_themes;
pub mod gui_todos;
pub mod project;

mod app;
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
pub mod gui_screenshots;
mod handlers;
mod headless;
mod highlight;
mod hook_payload;
mod http_client;
mod ipc;
mod markdown;
mod model_evidence;
mod model_options;
mod perf;
mod pi;
mod plan_interview;
mod prompt_library;
mod prompts;
mod qr;
mod remote_push;
mod remote_server;
mod remote_terminal;
mod resources;
mod review_batch;
mod screenshot_evidence;
mod screenshot_sources;
mod summary;
mod tailscale;
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
