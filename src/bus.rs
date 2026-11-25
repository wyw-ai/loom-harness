//! Transports deliver stable event references. SQLite owns the outbox and
//! each logical consumer's committed effects.

use crate::store::Store;
use anyhow::{Context, Result};
use serde_json::Value;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{task::JoinHandle, task::JoinSet, time::sleep};

mod redis_stream;
pub use redis_stream::RedisBus;
mod local;
pub use local::LocalBus;
mod lua_subscriptions;
pub(crate) use lua_subscriptions::{
    register_lua_subscription, spawn_plugin_bus, subscription_group,
};
#[cfg(test)]
mod tests;

const BATCH: usize = 32;
type BusFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

pub struct Entry {
    pub receipt: String,
    pub event_id: String,
}

pub struct Batch {
    pub entries: Vec<Entry>,
    pub next_pending_cursor: String,
}

/// External brokers need a relay from the transactional outbox.
pub trait OutboxPublisher: Send + Sync {
    fn publish<'a>(&'a self, event_id: &'a str) -> BusFuture<'a, ()>;
}

/// The transport returns both abandoned deliveries and newly available ones.
/// A receipt is opaque outside its group and is acknowledged only after commit.
pub trait EventBus: Send + Sync {
    fn publisher(self: Arc<Self>) -> Option<Arc<dyn OutboxPublisher>> {
        None
    }
    fn receive<'a>(
        &'a self,
        group: &'a str,
        worker: &'a str,
        pending_cursor: &'a str,
    ) -> BusFuture<'a, Batch>;
    fn ack<'a>(&'a self, group: &'a str, receipt: &'a str) -> BusFuture<'a, ()>;
}

type Commit = dyn Fn(&Store, &str, &Value) -> Result<()> + Send + Sync;

#[derive(Clone)]
pub struct Subscription {
    group: String,
    owner: Option<String>,
    event_types: Vec<String>,
    commit: Arc<Commit>,
}

impl Subscription {
    /// Internal host subscription. No unrestricted constructor is exposed to plugins.
    pub fn runtime() -> Self {
        Self::new("runtime".into(), None, vec![])
    }

    /// `id` includes the plugin instance and its stable subscription identity.
    /// Group names include the authorization scope so two tenants never share progress.
    pub fn scoped(id: &str, owner: &str, event_types: Vec<String>) -> Self {
        let group = serde_json::json!(["subscription", owner, id]).to_string();
        Self::new(group, Some(owner.to_owned()), event_types)
    }

    fn new(group: String, owner: Option<String>, event_types: Vec<String>) -> Self {
        Self {
            group,
            owner,
            event_types,
            commit: Arc::new(|store, group, event| {
                store.consume_event(group, event_key(event)?)?;
                Ok(())
            }),
        }
    }

    /// For a plugin handler, commit its resulting actions and the deduplication
    /// marker together. A transient failure leaves the transport entry pending.
    pub fn with_commit(
        mut self,
        commit: impl Fn(&Store, &str, &Value) -> Result<()> + Send + Sync + 'static,
    ) -> Self {
        self.commit = Arc::new(commit);
        self
    }

    pub fn group(&self) -> &str {
        &self.group
    }

    fn matches(&self, event: &Value) -> bool {
        self.owner
            .as_ref()
            .is_none_or(|owner| event["owner"].as_str() == Some(owner))
            && (self.event_types.is_empty()
                || self
                    .event_types
                    .iter()
                    .any(|ty| event["type"].as_str() == Some(ty)))
    }
}

fn event_key(event: &Value) -> Result<&str> {
    event["event_id"]
        .as_str()
        .or_else(|| event["id"].as_str())
        .context("stored event has no stable ID")
}

pub async fn publish_pending(store: &Store, bus: &dyn OutboxPublisher) -> Result<usize> {
    let pending = store.pending_events(BATCH)?;
    for event in &pending {
        let id = event_key(event)?;
        bus.publish(id).await?;
        store.published(id)?;
    }
    Ok(pending.len())
}

pub async fn consume_batch(
    store: &Store,
    bus: &dyn EventBus,
    subscription: &Subscription,
    worker: &str,
    cursor: &mut String,
) -> Result<usize> {
    let batch = bus.receive(subscription.group(), worker, cursor).await?;
    *cursor = batch.next_pending_cursor;
    let mut first_error = None;
    for entry in &batch.entries {
        // Only trust the stable reference from the bus. Re-read scope, type and payload.
        let result = (|| -> Result<()> {
            if let Some(event) = store.event_by_id(&entry.event_id)?
                && subscription.matches(&event)
            {
                (subscription.commit)(store, subscription.group(), &event)?;
            }
            Ok(())
        })();
        let result = match result {
            Ok(()) => bus.ack(subscription.group(), &entry.receipt).await,
            Err(err) => Err(err),
        };
        if let Err(err) = result {
            first_error.get_or_insert(err);
        }
    }
    if let Some(err) = first_error {
        return Err(err);
    }
    Ok(batch.entries.len())
}

/// Publisher and each group retry independently. Aborting this task drops the
/// JoinSet and stops all of its child loops, including an in-flight retry.
pub fn spawn_bus(
    store: Store,
    bus: Arc<dyn EventBus>,
    worker: String,
    subscriptions: Vec<Subscription>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks: JoinSet<()> = JoinSet::new();
        for subscription in subscriptions {
            let (store, bus, worker) = (store.clone(), bus.clone(), worker.clone());
            tasks.spawn(async move {
                let mut cursor = String::from("0-0");
                let mut failures = 0;
                loop {
                    let result =
                        consume_batch(&store, &*bus, &subscription, &worker, &mut cursor).await;
                    pace(subscription.group(), result, &mut failures).await;
                }
            });
        }
        if let Some(publisher) = bus.publisher() {
            tasks.spawn(async move {
                let mut failures = 0;
                loop {
                    let result = publish_pending(&store, &*publisher).await;
                    pace("outbox", result, &mut failures).await;
                }
            });
        }
        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                eprintln!("event bus task stopped: {err}");
            }
        }
    })
}

async fn pace(task: &str, result: Result<usize>, failures: &mut u32) {
    match result {
        Ok(count) => {
            *failures = 0;
            if count == 0 {
                sleep(Duration::from_millis(150)).await;
            } else {
                // SQLite batches may finish without an async suspension. Let
                // API and run tasks make progress while replaying a backlog.
                tokio::task::yield_now().await;
            }
        }
        Err(err) => {
            *failures = failures.saturating_add(1);
            if failures.is_power_of_two() {
                eprintln!("event bus {task} attempt {failures}: {err:#}");
            }
            sleep(Duration::from_millis(200 * 2u64.pow((*failures).min(5)))).await;
        }
    }
}
