//! The pre-start resource gate: one check, run before any agent harness is
//! spawned, that decides whether the machine is in a state worth warning
//! about.
//!
//! Both halves — too many agents already running, too little memory left —
//! are **soft** gates. They never refuse a start; they ask for one explicit
//! confirmation and then get out of the way. A missing signal (no memory
//! probe on this platform, limit disabled in config) is silently no gate at
//! all.

use crate::app::{AppMode, PendingStart, ResourceConfirmState, ViewState};
use crate::app::{App, AppConfig};
use crate::resources::limits::{self, LiveWindows};
use crate::resources::mem::{self, MemorySnapshot};

/// The concurrency half of the gate: how many agents are already running
/// against the configured cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverLimit {
    /// Harness sessions plus in-flight headless runs.
    pub active: usize,
    pub limit: usize,
}

/// The memory half of the gate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LowMemory {
    pub snapshot: MemorySnapshot,
    pub threshold_mb: u64,
}

/// Outcome of the pre-start check. `NeedsConfirm` always carries at least one
/// tripped gate, and both can be present at once — they are shown in a single
/// dialog rather than as two prompts in a row.
#[derive(Debug, Clone, PartialEq)]
pub enum StartPreconditions {
    Ok,
    NeedsConfirm {
        over_limit: Option<OverLimit>,
        low_memory: Option<LowMemory>,
    },
}

/// "1 agent" / "3 agents" — this shows up in a dialog and a warning, and
/// "1 agents already running" reads like a bug in the code that wrote it.
fn agents_running(count: usize) -> String {
    format!("{count} agent{}", if count == 1 { "" } else { "s" })
}

/// Decide the outcome from already-gathered inputs.
///
/// `memory` is `None` on platforms with no usable signal — there the
/// concurrency cap is the only guard, by design.
pub fn evaluate_start_preconditions(
    config: &AppConfig,
    active_agents: usize,
    memory: Option<MemorySnapshot>,
) -> StartPreconditions {
    // `active >= limit` rather than `>`: the question is whether the agent
    // about to start would push the machine past the cap, not whether it is
    // already past it.
    let over_limit = config
        .agent_concurrency_limit()
        .filter(|limit| active_agents >= *limit)
        .map(|limit| OverLimit {
            active: active_agents,
            limit,
        });

    let low_memory = config
        .low_memory_threshold_mb()
        .zip(memory)
        .filter(|(threshold, snapshot)| snapshot.is_low(*threshold))
        .map(|(threshold_mb, snapshot)| LowMemory {
            snapshot,
            threshold_mb,
        });

    if over_limit.is_none() && low_memory.is_none() {
        StartPreconditions::Ok
    } else {
        StartPreconditions::NeedsConfirm {
            over_limit,
            low_memory,
        }
    }
}

impl App {
    /// Gather the live inputs and run the pre-start gate.
    ///
    /// Costs a tmux census plus a `/proc` read, so it belongs on start paths,
    /// not in the render loop.
    pub fn check_start_preconditions(&self) -> StartPreconditions {
        let memory = mem::probe();
        let memory_is_fine = self
            .config
            .low_memory_threshold_mb()
            .zip(memory)
            .is_none_or(|(threshold, snapshot)| !snapshot.is_low(threshold));

        // The census costs a couple of tmux calls, and most starts happen on a
        // quiet machine. The store gives a free upper bound on running
        // harnesses — if even that is under the limit, and memory is fine,
        // there is nothing to ask about.
        let ceiling = limits::max_possible_harness_sessions(&self.store)
            + limits::in_flight_headless_runs();
        let certainly_under_limit = self
            .config
            .agent_concurrency_limit()
            .is_none_or(|limit| ceiling < limit);
        if memory_is_fine && certainly_under_limit {
            return StartPreconditions::Ok;
        }

        let live = LiveWindows::probe(self.tmux.as_ref());
        let active = limits::total_active_agents(&self.store, &live);
        evaluate_start_preconditions(&self.config, active, memory)
    }

    /// Run the gate for `pending`. Returns `true` when the caller should stop
    /// and let the confirmation dialog take over; `false` means proceed.
    pub fn gate_start(&mut self, pending: PendingStart) -> bool {
        match self.check_start_preconditions() {
            StartPreconditions::Ok => false,
            StartPreconditions::NeedsConfirm {
                over_limit,
                low_memory,
            } => {
                if let Some(over) = over_limit {
                    self.log_warn(
                        "limits",
                        format!(
                            "{} already running (limit {}) - asking before starting another",
                            agents_running(over.active),
                            over.limit
                        ),
                    );
                }
                if let Some(low) = low_memory {
                    self.log_warn(
                        "limits",
                        format!(
                            "{} MiB available, below the {} MiB warn threshold ({})",
                            low.snapshot.available_mb,
                            low.threshold_mb,
                            low.snapshot.source.label()
                        ),
                    );
                }
                self.mode = AppMode::ConfirmResourceStart(Box::new(ResourceConfirmState {
                    over_limit,
                    low_memory,
                    pending,
                    from_view: None,
                }));
                true
            }
        }
    }

    /// Whether a freshly-created feature may auto-start its agent.
    ///
    /// Creation paths do **not** raise the confirmation dialog: batch creation
    /// would queue one modal per feature, and the automation API has no user to
    /// answer them. Instead a tripped gate skips the auto-start and says so —
    /// the feature is created and left stopped, and starting it by hand goes
    /// through the interactive gate, which asks exactly once.
    pub(crate) fn autostart_allowed(&mut self, feature_name: &str) -> bool {
        match self.check_start_preconditions() {
            StartPreconditions::Ok => true,
            StartPreconditions::NeedsConfirm {
                over_limit,
                low_memory,
            } => {
                let reason = match (over_limit, low_memory) {
                    (Some(over), Some(low)) => format!(
                        "{} already running (limit {}) and only {} MiB free",
                        agents_running(over.active),
                        over.limit,
                        low.snapshot.available_mb
                    ),
                    (Some(over), None) => format!(
                        "{} already running (limit {})",
                        agents_running(over.active),
                        over.limit
                    ),
                    (None, Some(low)) => {
                        format!("only {} MiB memory available", low.snapshot.available_mb)
                    }
                    (None, None) => unreachable!("NeedsConfirm always carries a tripped gate"),
                };
                let notice = format!(
                    "'{feature_name}' created but not started: {reason}. Press c to start it."
                );
                self.log_warn("limits", notice.clone());
                self.push_toast_warning(notice.clone());
                self.message = Some(notice);
                false
            }
        }
    }

    /// Record where to return once the confirmation is answered. Called by the
    /// session-picker path, whose start may have been initiated from inside an
    /// embedded session view.
    pub fn set_resource_confirm_return_view(&mut self, view: Option<ViewState>) {
        if let AppMode::ConfirmResourceStart(state) = &mut self.mode {
            state.from_view = view;
        }
    }

    /// Proceed with the stashed start, bypassing the gate this time.
    pub fn confirm_pending_start(&mut self) -> anyhow::Result<()> {
        let AppMode::ConfirmResourceStart(state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        let ResourceConfirmState {
            pending, from_view, ..
        } = *state;

        let result = match pending {
            PendingStart::Feature { pi, fi } => self.begin_start_feature(pi, fi),
            PendingStart::BuiltinSession {
                pi,
                fi,
                kind,
                label,
            } => {
                // The picker's own "Added …" toast was suppressed when the add
                // was parked, so report it from here instead.
                let name = label.clone().unwrap_or_else(|| {
                    self.store
                        .projects
                        .get(pi)
                        .and_then(|project| project.features.get(fi))
                        .map(|feature| feature.next_label(&kind))
                        .unwrap_or_else(|| "session".to_string())
                });
                let added = self.add_builtin_session_unchecked(pi, fi, kind, label);
                if added.is_ok() {
                    self.push_toast_success(format!("Added '{name}'"));
                }
                added
            }
        };

        // Returning to the view the start came from is what the ungated path
        // would have done, so do it whether or not the start itself failed.
        if let Some(view) = from_view
            && !matches!(self.mode, AppMode::Viewing(_))
        {
            self.mode = AppMode::Viewing(view);
        }

        result
    }

    /// Drop the stashed start; nothing was spawned.
    pub fn cancel_pending_start(&mut self) {
        let AppMode::ConfirmResourceStart(state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return;
        };
        if let Some(view) = state.from_view {
            self.mode = AppMode::Viewing(view);
        }
        self.push_toast_info("Start cancelled");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::mem::MemorySource;

    fn snapshot(available_mb: u64) -> MemorySnapshot {
        MemorySnapshot {
            available_mb,
            total_mb: 16384,
            swap_free_mb: Some(2048),
            swap_total_mb: Some(2048),
            source: MemorySource::ProcMeminfo,
        }
    }

    fn config() -> AppConfig {
        AppConfig {
            max_concurrent_agents: 4,
            low_memory_warn_mb: 1536,
            ..AppConfig::default()
        }
    }

    #[test]
    fn quiet_machine_passes() {
        assert_eq!(
            evaluate_start_preconditions(&config(), 2, Some(snapshot(8000))),
            StartPreconditions::Ok
        );
    }

    #[test]
    fn confirms_at_the_limit_not_only_past_it() {
        // Four running with a cap of four: the fifth is the one that needs a
        // decision.
        let result = evaluate_start_preconditions(&config(), 4, Some(snapshot(8000)));
        assert_eq!(
            result,
            StartPreconditions::NeedsConfirm {
                over_limit: Some(OverLimit {
                    active: 4,
                    limit: 4
                }),
                low_memory: None,
            }
        );
        assert_eq!(
            evaluate_start_preconditions(&config(), 3, Some(snapshot(8000))),
            StartPreconditions::Ok
        );
    }

    #[test]
    fn confirms_on_low_memory_alone() {
        let result = evaluate_start_preconditions(&config(), 1, Some(snapshot(900)));
        match result {
            StartPreconditions::NeedsConfirm {
                over_limit,
                low_memory,
            } => {
                assert!(over_limit.is_none());
                let low = low_memory.expect("low memory gate should trip");
                assert_eq!(low.threshold_mb, 1536);
                assert_eq!(low.snapshot.available_mb, 900);
            }
            other => panic!("expected a confirm, got {other:?}"),
        }
    }

    #[test]
    fn both_gates_trip_into_one_confirmation() {
        let result = evaluate_start_preconditions(&config(), 9, Some(snapshot(100)));
        match result {
            StartPreconditions::NeedsConfirm {
                over_limit,
                low_memory,
            } => {
                assert!(over_limit.is_some());
                assert!(low_memory.is_some());
            }
            other => panic!("expected a confirm, got {other:?}"),
        }
    }

    #[test]
    fn no_memory_signal_leaves_the_concurrency_cap_as_the_only_guard() {
        // The macOS/unsupported-platform fallback: `None` must never be read
        // as "0 MiB available".
        assert_eq!(
            evaluate_start_preconditions(&config(), 1, None),
            StartPreconditions::Ok
        );
        assert!(matches!(
            evaluate_start_preconditions(&config(), 4, None),
            StartPreconditions::NeedsConfirm { .. }
        ));
    }

    #[test]
    fn zeroed_config_disables_each_gate() {
        let disabled = AppConfig {
            max_concurrent_agents: 0,
            low_memory_warn_mb: 0,
            ..AppConfig::default()
        };
        assert_eq!(
            evaluate_start_preconditions(&disabled, 99, Some(snapshot(1))),
            StartPreconditions::Ok
        );

        let memory_only = AppConfig {
            max_concurrent_agents: 0,
            ..config()
        };
        assert!(matches!(
            evaluate_start_preconditions(&memory_only, 99, Some(snapshot(1))),
            StartPreconditions::NeedsConfirm { .. }
        ));
    }
}
