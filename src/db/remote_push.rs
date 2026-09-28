//! SQLite persistence for Remote Control Web Push: the subscriptions each
//! paired browser registers, and the server's one VAPID signing key. See
//! `MIGRATION_043` for the schema and why it is shaped this way.

use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

/// One browser's push subscription, as `PushSubscription.toJSON()` reports
/// it: an endpoint on the browser vendor's push service plus the two keys
/// payloads are encrypted to (both base64url, unpadded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushSubscription {
    pub endpoint: String,
    pub device_id: String,
    pub p256dh: String,
    pub auth: String,
}

/// Insert a subscription, or re-point an existing endpoint at new keys and
/// the device that just registered it.
pub fn upsert(conn: &Connection, subscription: &PushSubscription) -> Result<()> {
    conn.execute(
        "INSERT INTO remote_push_subscriptions (endpoint, device_id, p256dh, auth, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(endpoint) DO UPDATE SET
             device_id = excluded.device_id,
             p256dh = excluded.p256dh,
             auth = excluded.auth",
        params![
            subscription.endpoint,
            subscription.device_id,
            subscription.p256dh,
            subscription.auth,
            Utc::now().to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Every subscription belonging to a device that is still allowed in — a
/// revoked device keeps its rows (revocation never deletes) but stops
/// receiving pushes here.
pub fn list_active(conn: &Connection) -> Result<Vec<PushSubscription>> {
    let mut stmt = conn.prepare(
        "SELECT s.endpoint, s.device_id, s.p256dh, s.auth
         FROM remote_push_subscriptions s
         JOIN remote_devices d ON d.id = s.device_id
         WHERE d.revoked = 0
         ORDER BY s.created_at",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(PushSubscription {
            endpoint: row.get(0)?,
            device_id: row.get(1)?,
            p256dh: row.get(2)?,
            auth: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Forget a subscription — used when the push service reports it gone.
pub fn delete(conn: &Connection, endpoint: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM remote_push_subscriptions WHERE endpoint = ?1",
        params![endpoint],
    )?;
    Ok(())
}

/// The VAPID private key (base64url raw scalar), if one has been minted.
pub fn vapid_private_key(conn: &Connection) -> Result<Option<String>> {
    conn.query_row(
        "SELECT private_key FROM remote_push_vapid WHERE id = 1",
        [],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Store the VAPID private key. Only ever called once per database: every
/// existing subscription is bound to the matching public key.
pub fn set_vapid_private_key(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO remote_push_vapid (id, private_key, created_at) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET private_key = excluded.private_key",
        params![key, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PushSubscription;
    use crate::db::AmfDb;
    use tempfile::NamedTempFile;

    fn open_temp_db() -> (NamedTempFile, AmfDb) {
        let tmp = NamedTempFile::new().unwrap();
        let db = AmfDb::open(tmp.path()).unwrap();
        (tmp, db)
    }

    fn subscription(endpoint: &str, device_id: &str) -> PushSubscription {
        PushSubscription {
            endpoint: endpoint.to_string(),
            device_id: device_id.to_string(),
            p256dh: "p256dh".to_string(),
            auth: "auth".to_string(),
        }
    }

    #[test]
    fn upsert_replaces_an_existing_endpoint() {
        let (_tmp, db) = open_temp_db();
        let a = db.create_remote_device("A", "hash-a").unwrap();
        let b = db.create_remote_device("B", "hash-b").unwrap();

        db.upsert_push_subscription(&subscription("https://push/1", &a.id))
            .unwrap();
        let mut moved = subscription("https://push/1", &b.id);
        moved.auth = "new-auth".to_string();
        db.upsert_push_subscription(&moved).unwrap();

        assert_eq!(db.list_active_push_subscriptions().unwrap(), vec![moved]);
    }

    #[test]
    fn revoked_devices_are_not_listed() {
        let (_tmp, db) = open_temp_db();
        let kept = db.create_remote_device("Kept", "hash-k").unwrap();
        let revoked = db.create_remote_device("Revoked", "hash-r").unwrap();
        db.upsert_push_subscription(&subscription("https://push/k", &kept.id))
            .unwrap();
        db.upsert_push_subscription(&subscription("https://push/r", &revoked.id))
            .unwrap();

        db.revoke_remote_device(&revoked.id).unwrap();

        let active = db.list_active_push_subscriptions().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].device_id, kept.id);
    }

    #[test]
    fn delete_forgets_a_subscription() {
        let (_tmp, db) = open_temp_db();
        let device = db.create_remote_device("A", "hash-a").unwrap();
        db.upsert_push_subscription(&subscription("https://push/1", &device.id))
            .unwrap();

        db.delete_push_subscription("https://push/1").unwrap();

        assert!(db.list_active_push_subscriptions().unwrap().is_empty());
    }

    #[test]
    fn vapid_key_round_trips() {
        let (_tmp, db) = open_temp_db();
        assert_eq!(db.vapid_private_key().unwrap(), None);

        db.set_vapid_private_key("secret").unwrap();

        assert_eq!(db.vapid_private_key().unwrap().as_deref(), Some("secret"));
    }
}
