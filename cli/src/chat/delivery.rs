use super::{
    store::Store,
    types::{Attempt, ClientEvent, Limits, SessionRef},
};
use anyhow::Result;

pub(super) const HOOK_UNCONFIRMED: &str = "Codex hook output and native acceptance are unconfirmed";
pub(super) const HOOK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Called only after native binding and observation validation. Epochs persist
/// across daemon restarts. A new native observation is required for every claim;
/// finishing NotSubmitted never calls this function recursively.
pub fn request(
    store: &mut Store,
    recipient: &SessionRef,
    event: ClientEvent,
    epoch: u64,
    limits: Limits,
) -> Result<Option<Attempt>> {
    if !store.observe(recipient, event, epoch)? {
        return Ok(None);
    }
    match event {
        ClientEvent::Idle | ClientEvent::Hook => store.claim(recipient, limits),
        ClientEvent::Busy | ClientEvent::Disconnected => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::types::Handoff;
    use crate::chat::{
        config::DEFAULT_LIMITS,
        types::{Actor, Draft, SessionRef, Target},
    };

    fn fixture() -> (tempfile::TempDir, Store, SessionRef) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        store.set_machine("pc").unwrap();
        (
            dir,
            store,
            SessionRef {
                machine: "pc".into(),
                incarnation: "dev".into(),
            },
        )
    }
    fn send(store: &mut Store, recipient: &SessionRef, key: &str, body: &str) -> String {
        store
            .send(
                &Actor::Human {
                    machine: "pc".into(),
                },
                &Draft {
                    key: key.into(),
                    project: "lam".into(),
                    to: vec![Target::Agent(recipient.clone())],
                    body: body.into(),
                    reply_to: None,
                },
            )
            .unwrap()
            .id
    }

    #[test]
    fn fake_client_can_process_busy_idle_hook_and_arrivals_without_owner_lock() {
        struct FakeClient;
        impl FakeClient {
            fn handoff(&self, attempt: &Attempt, during_io: impl FnOnce()) -> Handoff {
                assert!(!attempt.batch.text.is_empty());
                during_io();
                Handoff::NotSubmitted {
                    reason: "native busy before submission".into(),
                }
            }
        }
        let (dir, mut store, recipient) = fixture();
        let first = send(&mut store, &recipient, "one", "hello");
        assert!(
            request(&mut store, &recipient, ClientEvent::Busy, 1, DEFAULT_LIMITS)
                .unwrap()
                .is_none()
        );
        let attempt = request(&mut store, &recipient, ClientEvent::Idle, 2, DEFAULT_LIMITS)
            .unwrap()
            .unwrap();
        let result = FakeClient.handoff(&attempt, || {
            assert!(
                request(&mut store, &recipient, ClientEvent::Hook, 3, DEFAULT_LIMITS)
                    .unwrap()
                    .is_none()
            );
            send(&mut store, &recipient, "two", "new arrival");
            // A separate reader can observe the durable claim while client I/O runs.
            let reader = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
            assert_eq!(reader.attempt_recipient(&attempt.id).unwrap(), recipient);
        });
        store.finish(&attempt.id, result.clone()).unwrap();
        store.finish(&attempt.id, result).unwrap();
        assert!(
            request(&mut store, &recipient, ClientEvent::Idle, 2, DEFAULT_LIMITS)
                .unwrap()
                .is_none()
        );
        assert!(
            request(&mut store, &recipient, ClientEvent::Hook, 3, DEFAULT_LIMITS)
                .unwrap()
                .is_none()
        );
        let next = request(&mut store, &recipient, ClientEvent::Idle, 4, DEFAULT_LIMITS)
            .unwrap()
            .unwrap();
        assert_eq!(next.batch.full_ids.len(), 2);
        assert_eq!(next.batch.full_ids[0], first);
        store
            .finish(
                &next.id,
                Handoff::Refused {
                    reason: "native policy refusal".into(),
                },
            )
            .unwrap();
        assert!(
            request(&mut store, &recipient, ClientEvent::Idle, 5, DEFAULT_LIMITS)
                .unwrap()
                .is_none()
        );
        drop(store);
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        send(&mut store, &recipient, "three", "pending");
        assert!(
            request(&mut store, &recipient, ClientEvent::Idle, 4, DEFAULT_LIMITS)
                .unwrap()
                .is_none()
        );
        assert!(request(
            &mut store,
            &recipient,
            ClientEvent::Disconnected,
            6,
            DEFAULT_LIMITS
        )
        .unwrap()
        .is_none());
        assert!(
            request(&mut store, &recipient, ClientEvent::Hook, 7, DEFAULT_LIMITS)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn receipt_feed_is_typed_durable_and_fetch_does_not_accept_handoff() {
        let (dir, mut store, recipient) = fixture();
        let id = send(&mut store, &recipient, "one", "hello");
        let attempt = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        store
            .finish(
                &attempt.id,
                Handoff::Unknown {
                    reason: "stdout written without native acknowledgement".into(),
                },
            )
            .unwrap();
        store.record_fetch(&recipient, &[id]).unwrap();
        drop(store);
        let store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        let events = store.feed("lam", None, 0, 100, false).unwrap();
        let data = serde_json::to_value(events).unwrap();
        assert_eq!(data.as_array().unwrap().len(), 4);
        assert_eq!(data[2][1]["kind"], "receipt");
        assert_eq!(data[2][1]["state"], "unknown");
        assert_eq!(data[3][1]["kind"], "fetched");
    }

    #[test]
    fn claims_are_exclusive_and_arrival_during_handoff_stays_pending() {
        let (_dir, mut store, recipient) = fixture();
        let first = send(&mut store, &recipient, "one", "hello");
        let attempt = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        assert_eq!(attempt.batch.full_ids, [first]);
        assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
        let next = send(&mut store, &recipient, "two", "next");
        store
            .finish(
                &attempt.id,
                Handoff::Accepted {
                    receipt: "native-turn-1".into(),
                },
            )
            .unwrap();
        let second = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        assert_eq!(second.batch.full_ids, [next]);
        assert!(store
            .finish(
                &attempt.id,
                Handoff::Unknown {
                    reason: "late timeout".into()
                }
            )
            .is_err());
    }

    #[test]
    fn restart_uncertainty_requires_explicit_recipient_bound_retry() {
        let (dir, mut store, recipient) = fixture();
        let id = send(&mut store, &recipient, "one", "hello");
        let attempt = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        drop(store);
        let mut store = Store::open(&dir.path().join("chat.sqlite3")).unwrap();
        assert_eq!(store.recover_submitting().unwrap(), 1);
        assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
        assert!(store
            .retry_delivery(
                &SessionRef {
                    incarnation: "other".into(),
                    ..recipient.clone()
                },
                &id
            )
            .is_err());
        store.retry_delivery(&recipient, &id).unwrap();
        let retried = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        assert_ne!(attempt.id, retried.id);
    }

    #[test]
    fn explicit_retry_after_full_fetch_selects_unknown_and_refused_handoffs() {
        use crate::chat::types::FeedEvent;
        for outcome in [
            Handoff::Unknown {
                reason: "lost native response".into(),
            },
            Handoff::Refused {
                reason: "native refused".into(),
            },
        ] {
            let (_dir, mut store, recipient) = fixture();
            let id = send(&mut store, &recipient, "one", "hello");
            let original = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
            store.finish(&original.id, outcome).unwrap();
            store
                .record_fetch(&recipient, std::slice::from_ref(&id))
                .unwrap();
            let other = SessionRef {
                incarnation: "other".into(),
                ..recipient.clone()
            };
            assert!(store.retry_delivery(&other, &id).is_err());
            assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
            store.retry_delivery(&recipient, &id).unwrap();
            let retry = store
                .claim(&recipient, DEFAULT_LIMITS)
                .unwrap()
                .expect("explicit retry must override automatic fetch suppression");
            assert_ne!(retry.id, original.id);
            assert_eq!(retry.recipient, recipient);
            assert_eq!(retry.batch.full_ids.as_slice(), std::slice::from_ref(&id));
            store
                .finish(
                    &retry.id,
                    Handoff::NotSubmitted {
                        reason: "busy before native I/O".into(),
                    },
                )
                .unwrap();
            let later = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
            store
                .finish(
                    &later.id,
                    Handoff::Accepted {
                        receipt: "native accepted explicit retry".into(),
                    },
                )
                .unwrap();
            assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
            store.record_fetch(&recipient, &[id]).unwrap();
            let events = store.feed("lam", None, 0, 100, false).unwrap();
            assert_eq!(
                events
                    .iter()
                    .filter(|(_, e)| matches!(e, FeedEvent::Fetched { .. }))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn accepted_preview_does_not_starve_later_messages_or_repeat_reminders() {
        let (_dir, mut store, recipient) = fixture();
        let first = send(&mut store, &recipient, "one", &"long".repeat(4000));
        let second = send(&mut store, &recipient, "two", "next");
        let attempt = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
        assert!(attempt.batch.preview_ids.contains(&first));
        store
            .finish(
                &attempt.id,
                Handoff::Accepted {
                    receipt: "native-turn-1".into(),
                },
            )
            .unwrap();
        if !attempt.batch.full_ids.contains(&second) {
            let next = store.claim(&recipient, DEFAULT_LIMITS).unwrap().unwrap();
            assert_eq!(next.batch.full_ids, [second]);
            store
                .finish(
                    &next.id,
                    Handoff::Accepted {
                        receipt: "native-turn-2".into(),
                    },
                )
                .unwrap();
        }
        assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
        store
            .record_fetch(&recipient, std::slice::from_ref(&first))
            .unwrap();
        assert!(store.claim(&recipient, DEFAULT_LIMITS).unwrap().is_none());
    }
}
