//! `App`-side Web Push for the Remote Control PWA: when a feature starts
//! needing attention, notify every subscribed, non-revoked device. See
//! `crate::remote_push` for encryption and delivery.
//!
//! Detection runs every main-loop tick whether or not the remote-control
//! server is on. That is the plan's proposed split between the on-demand
//! server and push: the server is how a phone *reaches* AMF, but a push goes
//! out to the browser vendor's push service and needs nothing listening
//! here. Turning the server off stops pairing and `/status`, not
//! notifications to phones that already subscribed; revoking a device is
//! what stops those.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use crate::db::remote_push::PushSubscription;
use crate::remote_push::{self, DeliveryOutcome, DeliveryReport, PushMessage, VapidKey};
use crate::remote_server::{PushRequest, PushRequestKind};

use super::App;
use super::remote_attention::RemoteAttention;

pub struct RemotePushState {
    /// Loaded (or minted) on first need — see `App::ensure_vapid_key`.
    vapid: Option<VapidKey>,
    /// Cached `list_active_push_subscriptions`, so the per-tick check never
    /// queries the database. `None` means reload on next use; every write
    /// that changes the answer resets it.
    subscriptions: Option<Vec<PushSubscription>>,
    /// What was last announced, by feature id → its
    /// `RemoteAttention::fingerprint`; a changed fingerprint is exactly
    /// "this is news". `None` until the first tick seeds it, so whatever
    /// already needed attention when AMF started isn't re-announced.
    announced: Option<HashMap<String, String>>,
    reports_tx: Sender<DeliveryReport>,
    reports_rx: Receiver<DeliveryReport>,
}

impl Default for RemotePushState {
    fn default() -> Self {
        let (reports_tx, reports_rx) = channel();
        Self {
            vapid: None,
            subscriptions: None,
            announced: None,
            reports_tx,
            reports_rx,
        }
    }
}

impl App {
    /// AMF's VAPID key, loaded from the database or minted and stored on
    /// first use. `None` without a database: a key that doesn't survive a
    /// restart would orphan every subscription made with it.
    pub(super) fn ensure_vapid_key(&mut self) -> Option<&VapidKey> {
        if self.remote_push.vapid.is_none() {
            let db = self.db.as_ref()?;
            let key = match db.vapid_private_key() {
                Ok(Some(stored)) => VapidKey::from_private_key_b64(&stored),
                Ok(None) => {
                    let key = VapidKey::generate();
                    db.set_vapid_private_key(&key.private_key_b64())
                        .map(|()| key)
                }
                Err(e) => Err(e),
            };
            match key {
                Ok(key) => self.remote_push.vapid = Some(key),
                Err(e) => {
                    self.log_error("remote_push", format!("VAPID key unavailable: {e}"));
                    return None;
                }
            }
        }
        self.remote_push.vapid.as_ref()
    }

    fn push_subscriptions(&mut self) -> &[PushSubscription] {
        if self.remote_push.subscriptions.is_none() {
            let loaded = match &self.db {
                Some(db) => db.list_active_push_subscriptions(),
                None => Ok(Vec::new()),
            };
            let loaded = loaded.unwrap_or_else(|e| {
                self.log_error("remote_push", format!("Loading subscriptions: {e}"));
                Vec::new()
            });
            self.remote_push.subscriptions = Some(loaded);
        }
        self.remote_push
            .subscriptions
            .as_deref()
            .unwrap_or_default()
    }

    /// Drop the cached subscription list — call after anything that
    /// changes which subscriptions are active (subscribe, gone, revoke).
    pub(super) fn invalidate_push_subscriptions(&mut self) {
        self.remote_push.subscriptions = None;
    }

    /// Per-tick: act on delivery reports, then announce any attention that
    /// is new since last tick. Never blocks — sending happens on
    /// `remote_push::send_all`'s own thread.
    pub fn poll_remote_push(&mut self) {
        self.drain_push_reports();

        let attention = self.remote_attention_by_feature();
        let current: HashMap<String, String> = attention
            .iter()
            .map(|(feature_id, entry)| (feature_id.clone(), entry.fingerprint.clone()))
            .collect();
        let Some(announced) = self.remote_push.announced.replace(current) else {
            return;
        };
        let mut fresh: Vec<&String> = attention
            .iter()
            .filter(|(feature_id, entry)| announced.get(*feature_id) != Some(&entry.fingerprint))
            .map(|(feature_id, _)| feature_id)
            .collect();
        if fresh.is_empty() || self.push_subscriptions().is_empty() {
            return;
        }
        fresh.sort();

        let messages: Vec<PushMessage> = fresh
            .into_iter()
            .filter_map(|feature_id| {
                self.attention_push_message(feature_id, &attention[feature_id])
            })
            .collect();
        let subscriptions = self.push_subscriptions().to_vec();
        self.send_push(&subscriptions, &messages);
    }

    fn attention_push_message(
        &self,
        feature_id: &str,
        attention: &RemoteAttention,
    ) -> Option<PushMessage> {
        let (project, feature) = self.store.projects.iter().find_map(|project| {
            project
                .features
                .iter()
                .find(|feature| feature.id == feature_id)
                .map(|feature| (project.name.as_str(), feature))
        })?;
        Some(PushMessage {
            title: format!("{} {}", feature.name, attention.phrase),
            body: match &attention.detail {
                Some(detail) => format!("{project} · {}: {detail}", attention.reason),
                None => format!("{project} · {}", attention.reason),
            },
            tag: feature.tmux_session.clone(),
            // Straight to the agent to answer, when there is one; the
            // feature page otherwise.
            url: match feature.sessions.iter().find(|s| s.kind.is_agent_harness()) {
                Some(session) => format!("/#/s/{}", session.id),
                None => format!("/#/f/{}", feature.id),
            },
        })
    }

    fn send_push(&mut self, subscriptions: &[PushSubscription], messages: &[PushMessage]) {
        let Some(key) = self.ensure_vapid_key() else {
            return;
        };
        let mut requests = Vec::new();
        let mut errors = Vec::new();
        for subscription in subscriptions {
            for message in messages {
                match remote_push::build_request(key, subscription, message) {
                    Ok(request) => requests.push((subscription.clone(), request)),
                    Err(e) => errors.push(format!("{}: {e}", subscription.endpoint)),
                }
            }
        }
        for error in errors {
            self.log_error("remote_push", error);
        }
        remote_push::send_all(requests, self.remote_push.reports_tx.clone());
    }

    fn drain_push_reports(&mut self) {
        let reports: Vec<DeliveryReport> = self.remote_push.reports_rx.try_iter().collect();
        for report in reports {
            match report.outcome {
                DeliveryOutcome::Delivered => {}
                DeliveryOutcome::Gone => {
                    self.log_info(
                        "remote_push",
                        format!("Subscription gone, forgetting it: {}", report.endpoint),
                    );
                    if let Some(db) = &self.db
                        && let Err(e) = db.delete_push_subscription(&report.endpoint)
                    {
                        self.log_error("remote_push", format!("Deleting subscription: {e}"));
                    }
                    self.invalidate_push_subscriptions();
                }
                DeliveryOutcome::Failed(e) => {
                    self.log_warn(
                        "remote_push",
                        format!("Push to device {} failed: {e}", report.device_id),
                    );
                }
            }
        }
    }

    /// Answer every Web Push request the server forwarded since last tick.
    pub(super) fn drain_push_requests(&mut self) -> bool {
        let Some(handle) = &mut self.remote_server else {
            return false;
        };
        let mut requests = Vec::new();
        while let Some(request) = handle.try_recv_push_request() {
            requests.push(request);
        }
        let changed = !requests.is_empty();
        for request in requests {
            let PushRequest {
                device_id,
                kind,
                reply,
            } = request;
            let result = self.process_push_request(&device_id, kind);
            let _ = reply.send(result);
        }
        changed
    }

    fn process_push_request(
        &mut self,
        device_id: &str,
        kind: PushRequestKind,
    ) -> Result<(), String> {
        match kind {
            PushRequestKind::Subscribe {
                endpoint,
                p256dh,
                auth,
            } => {
                let Some(db) = &self.db else {
                    return Err("Push needs AMF's database, which isn't available".into());
                };
                db.upsert_push_subscription(&PushSubscription {
                    endpoint,
                    device_id: device_id.to_string(),
                    p256dh,
                    auth,
                })
                .map_err(|e| format!("Couldn't save the subscription: {e}"))?;
                self.invalidate_push_subscriptions();
                self.log_info("remote_push", format!("Device {device_id} subscribed"));
                Ok(())
            }
            PushRequestKind::Test => {
                let own: Vec<PushSubscription> = self
                    .push_subscriptions()
                    .iter()
                    .filter(|subscription| subscription.device_id == device_id)
                    .cloned()
                    .collect();
                if own.is_empty() {
                    return Err("This device hasn't turned on notifications yet".into());
                }
                self.send_push(
                    &own,
                    &[PushMessage {
                        title: "AMF Remote".into(),
                        body: "Test notification — push is working.".into(),
                        tag: "amf-test".into(),
                        url: "/".into(),
                    }],
                );
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::attention::{AttentionRecord, AttentionState};
    use crate::project::AgentKind;

    fn app_with_subscription() -> (tempfile::NamedTempFile, App) {
        let (db_file, mut app) = crate::app::remote_server::tests::test_app_with_feature_and_db();
        let db = app.db.as_ref().unwrap();
        let device = db.create_remote_device("Phone", "hash").unwrap();
        db.upsert_push_subscription(&PushSubscription {
            // The keys don't decode, so `build_request` fails and nothing
            // reaches the network — these tests are about what gets sent
            // when, not delivery (`crate::remote_push` covers that).
            endpoint: "https://push.example.com/phone".into(),
            device_id: device.id,
            p256dh: "unused".into(),
            auth: "unused".into(),
        })
        .unwrap();
        app.poll_remote_push(); // seed
        (db_file, app)
    }

    fn raise(app: &mut App, state: AttentionState) {
        app.attention.insert(
            "amf-my-feature".into(),
            AttentionRecord::new(AgentKind::Claude, state, chrono::Utc::now()),
        );
    }

    fn feature_id(app: &App) -> String {
        app.store.projects[0].features[0].id.clone()
    }

    #[test]
    fn vapid_key_is_minted_once_and_reused() {
        let (_db, mut app) = app_with_subscription();
        let first = app.ensure_vapid_key().unwrap().public_key_b64();
        app.remote_push = RemotePushState::default();
        let reloaded = app.ensure_vapid_key().unwrap().public_key_b64();
        assert_eq!(first, reloaded);
    }

    #[test]
    fn attention_present_at_startup_is_not_announced() {
        let (_db_file, mut app) = crate::app::remote_server::tests::test_app_with_feature_and_db();
        raise(&mut app, AttentionState::Question);
        app.poll_remote_push();
        assert_eq!(
            app.remote_push.announced.as_ref().unwrap().len(),
            1,
            "the first tick only seeds"
        );
    }

    #[test]
    fn new_and_changed_attention_are_announced_once() {
        let (_db, mut app) = app_with_subscription();
        raise(&mut app, AttentionState::Waiting);
        let id = feature_id(&app);
        let attention = app.remote_attention_by_feature();
        let message = app.attention_push_message(&id, &attention[&id]).unwrap();
        assert_eq!(message.title, "my-feature is waiting for you");
        assert_eq!(message.body, "my-project · Waiting");
        assert_eq!(message.tag, "amf-my-feature");
        let feature = &app.store.projects[0].features[0];
        let expected = match feature.sessions.iter().find(|s| s.kind.is_agent_harness()) {
            Some(session) => format!("/#/s/{}", session.id),
            None => format!("/#/f/{}", feature.id),
        };
        assert_eq!(message.url, expected);

        app.poll_remote_push();
        let first = app.remote_push.announced.clone().unwrap();
        app.poll_remote_push();
        assert_eq!(app.remote_push.announced.clone().unwrap(), first);

        std::thread::sleep(std::time::Duration::from_millis(2));
        raise(&mut app, AttentionState::Question);
        app.poll_remote_push();
        assert_ne!(app.remote_push.announced.clone().unwrap(), first);
    }

    #[test]
    fn subscribe_then_test_reaches_only_that_devices_subscriptions() {
        let (_db, mut app) = app_with_subscription();
        let other = app
            .db
            .as_ref()
            .unwrap()
            .create_remote_device("Other", "hash-2")
            .unwrap();
        assert_eq!(
            app.process_push_request(&other.id, PushRequestKind::Test),
            Err("This device hasn't turned on notifications yet".into())
        );

        let result = app.process_push_request(
            &other.id,
            PushRequestKind::Subscribe {
                endpoint: "https://push.example.com/other".into(),
                p256dh: "k".into(),
                auth: "a".into(),
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(app.push_subscriptions().len(), 2);
    }

    #[test]
    fn a_gone_report_deletes_the_subscription() {
        let (_db, mut app) = app_with_subscription();
        let endpoint = app.push_subscriptions()[0].endpoint.clone();
        app.remote_push
            .reports_tx
            .send(DeliveryReport {
                endpoint,
                device_id: "whoever".into(),
                outcome: DeliveryOutcome::Gone,
            })
            .unwrap();

        app.poll_remote_push();

        assert!(app.push_subscriptions().is_empty());
    }

    #[test]
    fn revoking_a_device_stops_its_pushes() {
        let (_db, mut app) = app_with_subscription();
        let device_id = app.push_subscriptions()[0].device_id.clone();
        app.db
            .as_ref()
            .unwrap()
            .revoke_remote_device(&device_id)
            .unwrap();
        app.invalidate_push_subscriptions();
        assert!(app.push_subscriptions().is_empty());
    }
}
