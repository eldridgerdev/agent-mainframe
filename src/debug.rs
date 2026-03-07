use chrono::{DateTime, Utc};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub const ALL: [LogLevel; 4] = [
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
    ];

    pub fn display(&self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }

    pub fn next(&self) -> Self {
        let all = Self::ALL.as_slice();
        let idx = all.iter().position(|v| v == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }
}

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub context: String,
    pub message: String,
}

pub struct DebugLog {
    entries: VecDeque<LogEntry>,
    max_entries: usize,
    log_file: PathBuf,
}

pub fn global_log_path() -> PathBuf {
    let state_dir = dirs::state_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("amf");
    let _ = fs::create_dir_all(&state_dir);
    state_dir.join("debug.log")
}

impl Default for DebugLog {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl DebugLog {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries),
            max_entries,
            log_file: global_log_path(),
        }
    }

    pub fn log(&mut self, level: LogLevel, context: &str, message: String) {
        let entry = LogEntry {
            timestamp: Utc::now(),
            level,
            context: context.to_string(),
            message,
        };
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(entry.clone());

        self.write_to_file(&entry);
    }

    fn write_to_file(&self, entry: &LogEntry) {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
        {
            let time = entry.timestamp.format("%Y-%m-%d %H:%M:%S%.3f");
            let line = format!(
                "{} [{:<5}] {}: {}\n",
                time,
                entry.level.display(),
                entry.context,
                entry.message
            );
            let _ = file.write_all(line.as_bytes());
        }
    }

    pub fn debug(&mut self, context: &str, message: String) {
        self.log(LogLevel::Debug, context, message);
    }

    pub fn info(&mut self, context: &str, message: String) {
        self.log(LogLevel::Info, context, message);
    }

    pub fn warn(&mut self, context: &str, message: String) {
        self.log(LogLevel::Warn, context, message);
    }

    pub fn error(&mut self, context: &str, message: String) {
        self.log(LogLevel::Error, context, message);
    }

    pub fn entries(&self) -> &VecDeque<LogEntry> {
        &self.entries
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn log_file(&self) -> &PathBuf {
        &self.log_file
    }
}

/// Write a log entry directly to the log file without going through
/// an `App` instance. Intended for background threads (e.g. IPC
/// server) that cannot borrow `App`.
pub fn log_to_file(level: LogLevel, context: &str, message: &str) {
    let path = global_log_path();
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let time = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let line = format!(
            "{} [{:<5}] {}: {}\n",
            time,
            level.display(),
            context,
            message,
        );
        let _ = file.write_all(line.as_bytes());
    }
}
