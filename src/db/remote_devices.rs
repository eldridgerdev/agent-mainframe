//! SQLite persistence for devices paired to the Remote Control companion
//! app (see `docs/backlog/remote-control-companion-app-plan.md`, Epic 2).
//!
//! This module only stores and looks up hashed tokens — it has no opinion
//! on how a token is minted or hashed. That belongs to the pairing flow
//! (Epic 4), which is the only caller expected to see a plaintext token.
//! Once issued, every subsequent request authenticates by hashing the
//! presented token the same way and looking it up here.
//!
//! Epic 4 (pairing flow) is the first real caller, so this module is
//! exercised only by its own tests for now.
#![allow(dead_code)]

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

/// One paired device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDevice {
    pub id: String,
    /// A human-readable label (e.g. "Ryan's iPhone"). Empty until the
    /// pairing flow collects one.
    pub name: String,
    pub token_hash: String,
    pub paired_at: DateTime<Utc>,
    /// `None` until the device makes its first authenticated request.
    pub last_seen_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

/// Register a newly paired device. `token_hash` must already be hashed —
/// this module never sees or stores a plaintext token.
pub fn create(conn: &Connection, name: &str, token_hash: &str) -> Result<RemoteDevice> {
    let device = RemoteDevice {
        id: Uuid::new_v4().to_string(),
        name: name.to_string(),
        token_hash: token_hash.to_string(),
        paired_at: Utc::now(),
        last_seen_at: None,
        revoked: false,
    };

    conn.execute(
        "INSERT INTO remote_devices (id, name, token_hash, paired_at, last_seen_at, revoked)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            device.id,
            device.name,
            device.token_hash,
            device.paired_at.to_rfc3339(),
            device.last_seen_at.map(|dt| dt.to_rfc3339()),
            device.revoked as i64,
        ],
    )?;

    Ok(device)
}

/// Look up a device by id (for the desktop paired-devices list acting on a
/// selection, e.g. revoke).
pub fn find_by_id(conn: &Connection, id: &str) -> Result<Option<RemoteDevice>> {
    conn.query_row(
        "SELECT id, name, token_hash, paired_at, last_seen_at, revoked
         FROM remote_devices WHERE id = ?1",
        params![id],
        row_to_device,
    )
    .optional()
    .map_err(Into::into)
}

/// Look up a device by its token's hash — the authentication path every
/// request other than pairing itself will use. Returns a revoked device
/// too (rather than hiding it as "not found"), so callers can distinguish
/// an unknown token from a revoked one and respond/log accordingly.
pub fn find_by_token_hash(conn: &Connection, token_hash: &str) -> Result<Option<RemoteDevice>> {
    conn.query_row(
        "SELECT id, name, token_hash, paired_at, last_seen_at, revoked
         FROM remote_devices WHERE token_hash = ?1",
        params![token_hash],
        row_to_device,
    )
    .optional()
    .map_err(Into::into)
}

/// Every paired device, most recently paired first — the desktop
/// paired-devices list.
pub fn list_all(conn: &Connection) -> Result<Vec<RemoteDevice>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, token_hash, paired_at, last_seen_at, revoked
         FROM remote_devices
         ORDER BY paired_at DESC",
    )?;
    let rows = stmt.query_map([], row_to_device)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Record that a device just made an authenticated request.
pub fn touch_last_seen(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE remote_devices SET last_seen_at = ?2 WHERE id = ?1",
        params![id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Revoke a device. Revocation is permanent from here — there is no
/// un-revoke; pairing again issues a new device row with a new token.
pub fn revoke(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE remote_devices SET revoked = 1 WHERE id = ?1",
        params![id],
    )?;
    Ok(())
}

fn row_to_device(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteDevice> {
    let paired_at: String = row.get(3)?;
    let last_seen_at: Option<String> = row.get(4)?;
    let revoked: i64 = row.get(5)?;
    Ok(RemoteDevice {
        id: row.get(0)?,
        name: row.get(1)?,
        token_hash: row.get(2)?,
        paired_at: DateTime::parse_from_rfc3339(&paired_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        last_seen_at: last_seen_at.and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        }),
        revoked: revoked != 0,
    })
}

#[cfg(test)]
mod tests {
    use crate::db::AmfDb;
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    #[test]
    fn creates_and_finds_by_id() {
        let (_tmp, db) = open_temp_db();
        let device = db.create_remote_device("Ryan's iPhone", "hash-1").unwrap();

        let found = db.find_remote_device_by_id(&device.id).unwrap();
        assert_eq!(found, Some(device.clone()));
        assert_eq!(device.name, "Ryan's iPhone");
        assert!(!device.revoked);
        assert!(device.last_seen_at.is_none());
    }

    #[test]
    fn finds_by_token_hash() {
        let (_tmp, db) = open_temp_db();
        let device = db.create_remote_device("phone", "hash-abc").unwrap();

        let found = db.find_remote_device_by_token_hash("hash-abc").unwrap();
        assert_eq!(found, Some(device));

        assert!(
            db.find_remote_device_by_token_hash("no-such-hash")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unknown_id_and_token_hash_are_none_not_an_error() {
        let (_tmp, db) = open_temp_db();
        assert!(db.find_remote_device_by_id("missing").unwrap().is_none());
        assert!(
            db.find_remote_device_by_token_hash("missing")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn revoke_flips_the_flag_but_leaves_the_device_findable() {
        let (_tmp, db) = open_temp_db();
        let device = db.create_remote_device("phone", "hash-1").unwrap();
        assert!(!device.revoked);

        db.revoke_remote_device(&device.id).unwrap();

        let found = db.find_remote_device_by_id(&device.id).unwrap().unwrap();
        assert!(found.revoked, "revoke should set the flag");

        // A revoked device is still returned by token-hash lookup — the
        // caller (auth) decides what a revoked token means, rather than
        // this module hiding it as "not found".
        let by_hash = db
            .find_remote_device_by_token_hash("hash-1")
            .unwrap()
            .unwrap();
        assert!(by_hash.revoked);
    }

    #[test]
    fn touch_last_seen_sets_the_timestamp() {
        let (_tmp, db) = open_temp_db();
        let device = db.create_remote_device("phone", "hash-1").unwrap();
        assert!(device.last_seen_at.is_none());

        db.touch_remote_device_last_seen(&device.id).unwrap();

        let found = db.find_remote_device_by_id(&device.id).unwrap().unwrap();
        assert!(found.last_seen_at.is_some());
    }

    #[test]
    fn list_all_returns_every_paired_device() {
        let (_tmp, db) = open_temp_db();
        db.create_remote_device("phone-a", "hash-a").unwrap();
        db.create_remote_device("phone-b", "hash-b").unwrap();

        let all = db.list_remote_devices().unwrap();
        assert_eq!(all.len(), 2);
        let names: Vec<&str> = all.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"phone-a"));
        assert!(names.contains(&"phone-b"));
    }

    #[test]
    fn token_hash_is_unique() {
        let (_tmp, db) = open_temp_db();
        db.create_remote_device("phone-a", "same-hash").unwrap();
        let result = db.create_remote_device("phone-b", "same-hash");
        assert!(result.is_err(), "two devices must never share a token hash");
    }
}
