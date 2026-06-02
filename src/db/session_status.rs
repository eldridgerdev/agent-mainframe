use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn load(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    let mut stmt =
        conn.prepare_cached("SELECT status_text FROM session_status WHERE session_id = ?1")?;
    let result = stmt
        .query_row([session_id], |row| row.get::<_, String>(0))
        .optional()?;
    Ok(result)
}

/// Returns `(status_text, file_mtime_nanos)` for the cached entry, if any.
pub(super) fn load_with_mtime(
    conn: &Connection,
    session_id: &str,
) -> Result<Option<(String, Option<u64>)>> {
    let mut stmt = conn.prepare_cached(
        "SELECT status_text, file_mtime_nanos FROM session_status WHERE session_id = ?1",
    )?;
    let result = stmt
        .query_row([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
            ))
        })
        .optional()?;
    Ok(result)
}

pub(super) fn upsert(
    conn: &Connection,
    session_id: &str,
    feature_id: &str,
    status_text: &str,
    file_mtime_nanos: Option<u64>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_status (session_id, feature_id, status_text, file_mtime_nanos, updated_at)
         VALUES (?1, ?2, ?3, ?4, datetime('now'))
         ON CONFLICT(session_id) DO UPDATE SET
             feature_id       = excluded.feature_id,
             status_text      = excluded.status_text,
             file_mtime_nanos = excluded.file_mtime_nanos,
             updated_at       = excluded.updated_at",
        params![session_id, feature_id, status_text, file_mtime_nanos.map(|v| v as i64)],
    )?;
    Ok(())
}

pub(super) fn delete_session(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM session_status WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}

pub(super) fn delete_feature(conn: &Connection, feature_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM session_status WHERE feature_id = ?1",
        [feature_id],
    )?;
    Ok(())
}
