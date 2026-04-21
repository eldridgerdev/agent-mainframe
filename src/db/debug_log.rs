use anyhow::Result;
use rusqlite::{Connection, params};

use crate::debug::{LogEntry, LogLevel};

fn level_to_str(l: LogLevel) -> &'static str {
    match l {
        LogLevel::Debug => "DEBUG",
        LogLevel::Info => "INFO",
        LogLevel::Warn => "WARN",
        LogLevel::Error => "ERROR",
    }
}

fn level_from_str(s: &str) -> LogLevel {
    match s {
        "INFO" => LogLevel::Info,
        "WARN" => LogLevel::Warn,
        "ERROR" => LogLevel::Error,
        _ => LogLevel::Debug,
    }
}

pub fn append(conn: &Connection, entry: &LogEntry) -> Result<()> {
    conn.execute(
        "INSERT INTO debug_log (ts, level, context, message) VALUES (?1, ?2, ?3, ?4)",
        params![
            entry.timestamp.to_rfc3339(),
            level_to_str(entry.level),
            entry.context,
            entry.message,
        ],
    )?;
    Ok(())
}

/// Load the most recent `limit` entries in chronological order.
pub fn load_recent(conn: &Connection, limit: usize) -> Result<Vec<LogEntry>> {
    let mut stmt = conn.prepare(
        "SELECT ts, level, context, message
         FROM debug_log
         ORDER BY id DESC
         LIMIT ?1",
    )?;

    let mut entries: Vec<LogEntry> = stmt
        .query_map(params![limit as i64], |row| {
            let ts_str: String = row.get(0)?;
            let level_str: String = row.get(1)?;
            let context: String = row.get(2)?;
            let message: String = row.get(3)?;
            Ok((ts_str, level_str, context, message))
        })?
        .filter_map(|r| r.ok())
        .filter_map(|(ts_str, level_str, context, message)| {
            let timestamp = ts_str.parse().ok()?;
            Some(LogEntry {
                timestamp,
                level: level_from_str(&level_str),
                context,
                message,
            })
        })
        .collect();

    // Reverse so oldest comes first (matching chronological VecDeque order).
    entries.reverse();
    Ok(entries)
}
