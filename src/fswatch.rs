//! Single filesystem-watcher worker (architecture item 2 of the
//! performance plan).
//!
//! One `notify` (inotify) watcher observes the filesystem state AMF
//! previously discovered by periodic scanning:
//!
//! - `/tmp/amf-thinking` — thinking marker files
//! - `~/.codex/sessions` — codex transcripts (usage stats)
//! - `~/.claude/projects` — claude transcripts (usage stats)
//! - per-feature `.claude` / `.codex` dirs — `latest-prompt.txt`
//!
//! Events set dirty flags consumed by the main loop. Latency-sensitive
//! events (thinking, prompts) also write a byte to the wakeup pipe so
//! the loop leaves `poll()` immediately; transcript appends do not —
//! they fire continuously while agents stream, and usage stats are
//! happy to wait for the next tick. The old scan timers remain as
//! long-interval fallbacks in case inotify is unavailable or events
//! are dropped.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::debug::{LogLevel, log_to_file};

const PROMPT_FILE_NAME: &str = "latest-prompt.txt";

/// Dirty state shared between the notify callback thread and the main
/// loop. Consumed (reset) by the `take_*` accessors.
#[derive(Default)]
pub struct FsWatchFlags {
    thinking_dirty: AtomicBool,
    usage_dirty: AtomicBool,
    prompt_paths: Mutex<HashSet<PathBuf>>,
}

impl FsWatchFlags {
    pub fn take_thinking_dirty(&self) -> bool {
        self.thinking_dirty.swap(false, Ordering::Relaxed)
    }

    pub fn take_usage_dirty(&self) -> bool {
        self.usage_dirty.swap(false, Ordering::Relaxed)
    }

    pub fn take_prompt_paths(&self) -> Vec<PathBuf> {
        let mut paths = self.prompt_paths.lock().unwrap();
        if paths.is_empty() {
            Vec::new()
        } else {
            paths.drain().collect()
        }
    }
}

pub struct FsWatcher {
    watcher: RecommendedWatcher,
    pub flags: Arc<FsWatchFlags>,
    watched_prompt_dirs: HashSet<PathBuf>,
}

impl FsWatcher {
    /// Start the watcher. `wakeup_fd` is the write end of the main
    /// loop's wakeup pipe (non-blocking).
    pub fn start(wakeup_fd: Arc<OwnedFd>) -> notify::Result<Self> {
        let flags = Arc::new(FsWatchFlags::default());

        let thinking_root = PathBuf::from("/tmp/amf-thinking");
        let codex_root = dirs::home_dir().map(|home| home.join(".codex").join("sessions"));
        let claude_root = dirs::home_dir().map(|home| home.join(".claude").join("projects"));

        let handler_flags = flags.clone();
        let handler_thinking_root = thinking_root.clone();
        let handler_codex_root = codex_root.clone();
        let handler_claude_root = claude_root.clone();

        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            // Access events fire whenever AMF itself reads these files;
            // only content changes matter.
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }

            let mut wake = false;
            for path in &event.paths {
                if path.starts_with(&handler_thinking_root) {
                    handler_flags.thinking_dirty.store(true, Ordering::Relaxed);
                    wake = true;
                } else if path
                    .file_name()
                    .is_some_and(|name| name == PROMPT_FILE_NAME)
                {
                    handler_flags
                        .prompt_paths
                        .lock()
                        .unwrap()
                        .insert(path.clone());
                    wake = true;
                } else if handler_codex_root
                    .as_ref()
                    .is_some_and(|root| path.starts_with(root))
                    || handler_claude_root
                        .as_ref()
                        .is_some_and(|root| path.starts_with(root))
                {
                    // No wakeup: transcripts append continuously while
                    // agents stream; the next tick picks this up.
                    handler_flags.usage_dirty.store(true, Ordering::Relaxed);
                }
            }

            if wake {
                let byte = 1u8;
                unsafe {
                    libc::write(wakeup_fd.as_raw_fd(), &byte as *const u8 as *const _, 1);
                }
            }
        })?;

        // The thinking dir may not exist yet; create it so the watch
        // sticks (hook scripts create files inside it later).
        let _ = std::fs::create_dir_all(&thinking_root);
        if let Err(e) = watcher.watch(&thinking_root, RecursiveMode::NonRecursive) {
            log_to_file(
                LogLevel::Warn,
                "fswatch",
                &format!("failed to watch {}: {e}", thinking_root.display()),
            );
        }
        for root in [&codex_root, &claude_root].into_iter().flatten() {
            if root.is_dir()
                && let Err(e) = watcher.watch(root, RecursiveMode::Recursive)
            {
                log_to_file(
                    LogLevel::Warn,
                    "fswatch",
                    &format!("failed to watch {}: {e}", root.display()),
                );
            }
        }

        log_to_file(LogLevel::Info, "fswatch", "filesystem watcher started");

        Ok(Self {
            watcher,
            flags,
            watched_prompt_dirs: HashSet::new(),
        })
    }

    /// Reconcile the per-feature prompt-dir watches with the current
    /// feature set. Cheap to call periodically: it only touches the
    /// watcher when the set actually changed.
    pub fn sync_prompt_watches<I>(&mut self, feature_workdirs: I)
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let desired: HashSet<PathBuf> = feature_workdirs
            .into_iter()
            .flat_map(|workdir| [workdir.join(".claude"), workdir.join(".codex")])
            .filter(|dir| dir.is_dir())
            .collect();

        if desired == self.watched_prompt_dirs {
            return;
        }

        for stale in self.watched_prompt_dirs.difference(&desired) {
            let _ = self.watcher.unwatch(stale);
        }
        for added in desired.difference(&self.watched_prompt_dirs) {
            if let Err(e) = self.watcher.watch(added, RecursiveMode::NonRecursive) {
                log_to_file(
                    LogLevel::Debug,
                    "fswatch",
                    &format!("failed to watch {}: {e}", added.display()),
                );
            }
        }
        self.watched_prompt_dirs = desired;
    }
}
