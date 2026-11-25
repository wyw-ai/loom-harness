use super::{BATCH, Batch, BusFuture, Entry, EventBus};
use crate::store::Store;
use anyhow::Context;

/// The committed SQLite outbox is the local stream. The published flag tracks
/// forwarding to an external broker and has no bearing on local visibility.
pub struct LocalBus {
    store: Store,
}

impl LocalBus {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl EventBus for LocalBus {
    fn receive<'a>(&'a self, group: &'a str, _: &'a str, _: &'a str) -> BusFuture<'a, Batch> {
        Box::pin(async move {
            let entries = self
                .store
                .claim_local_events(group, BATCH, 10_000)?
                .into_iter()
                .map(|(seq, lease, event_id)| Entry {
                    receipt: format!("{seq}:{lease}"),
                    event_id,
                })
                .collect();
            Ok(Batch {
                entries,
                next_pending_cursor: String::new(),
            })
        })
    }

    fn ack<'a>(&'a self, group: &'a str, receipt: &'a str) -> BusFuture<'a, ()> {
        Box::pin(async move {
            let (seq, lease) = receipt
                .split_once(':')
                .context("invalid local bus receipt")?;
            self.store.ack_local_event(group, seq.parse()?, lease)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests;
