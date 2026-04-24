use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn get(conn: &Connection, session_id: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare_cached(
        "SELECT status_text FROM session_status WHERE session_id = ?1",
    )?;
    let result = stmt
        .query_row([session_id], |row| row.get::<_, String>(0))
        .optional()?;
    Ok(result)
}

pub(super) fn set(conn: &Connection, session_id: &str, status_text: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO session_status (session_id, status_text, updated_at)
         VALUES (?1, ?2, datetime('now'))
         ON CONFLICT(session_id) DO UPDATE SET
             status_text = excluded.status_text,
             updated_at  = excluded.updated_at",
        params![session_id, status_text],
    )?;
    Ok(())
}

pub(super) fn delete(conn: &Connection, session_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM session_status WHERE session_id = ?1",
        [session_id],
    )?;
    Ok(())
}
