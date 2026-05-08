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

pub(super) fn upsert(
    conn: &Connection,
    session_id: &str,
    feature_id: &str,
    status_text: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO session_status (session_id, feature_id, status_text, updated_at)
         VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(session_id) DO UPDATE SET
             feature_id = excluded.feature_id,
             status_text = excluded.status_text,
             updated_at  = excluded.updated_at",
        params![session_id, feature_id, status_text],
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
