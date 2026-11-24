use super::*;
use anyhow::bail;
use serde_json::json;
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// A transport that deterministically redelivers unacknowledged references.
#[derive(Default)]
struct TestBus {
    stream: Mutex<Vec<String>>,
    acknowledged: Mutex<HashSet<(String, String)>>,
    lose_publish_reply: AtomicBool,
    lose_ack: AtomicBool,
}

impl EventBus for TestBus {
    fn publish<'a>(&'a self, id: &'a str) -> BusFuture<'a, ()> {
        Box::pin(async move {
            self.stream.lock().unwrap().push(id.into());
            if self.lose_publish_reply.swap(false, Ordering::SeqCst) {
                bail!("publish reply lost");
            }
            Ok(())
        })
    }
    fn receive<'a>(&'a self, group: &'a str, _: &'a str, _: &'a str) -> BusFuture<'a, Batch> {
        Box::pin(async move {
            let acknowledgements = self.acknowledged.lock().unwrap();
            let entries = self
                .stream
                .lock()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(i, id)| Entry {
                    receipt: i.to_string(),
                    event_id: id.clone(),
                })
                .filter(|e| !acknowledgements.contains(&(group.into(), e.receipt.clone())))
                .collect();
            Ok(Batch {
                entries,
                next_pending_cursor: "0-0".into(),
            })
        })
    }
    fn ack<'a>(&'a self, group: &'a str, receipt: &'a str) -> BusFuture<'a, ()> {
        Box::pin(async move {
            if self.lose_ack.swap(false, Ordering::SeqCst) {
                bail!("process stopped before ack");
            }
            self.acknowledged
                .lock()
                .unwrap()
                .insert((group.into(), receipt.into()));
            Ok(())
        })
    }
}

#[tokio::test]
async fn repeated_publication_and_lost_ack_keep_each_subscription_independent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("loom.db");
    let store = Store::open(&path)?;
    let event = store.post_event("team", json!({"type":"report"}), None)?;
    let id = event_key(&event)?;
    let transport = TestBus::default();
    transport.lose_publish_reply.store(true, Ordering::SeqCst);
    assert!(publish_pending(&store, &transport).await.is_err());
    drop(store);
    let store = Store::open(&path)?;
    assert_eq!(publish_pending(&store, &transport).await?, 1);
    assert_eq!(publish_pending(&store, &transport).await?, 0);
    assert_eq!(*transport.stream.lock().unwrap(), [id, id]);

    let first = Subscription::runtime();
    let audit = Subscription::scoped("audit", "team", vec!["report".into()]);
    let mut cursor = "0-0".into();
    transport.lose_ack.store(true, Ordering::SeqCst);
    assert!(
        consume_batch(&store, &transport, &first, "worker-1", &mut cursor)
            .await
            .is_err()
    );
    assert_eq!(
        consume_batch(&store, &transport, &audit, "worker-2", &mut cursor).await?,
        2
    );
    assert_eq!(
        consume_batch(&store, &transport, &first, "worker-2", &mut cursor).await?,
        1
    );
    assert!(!store.consume_event(first.group(), id)?);
    assert!(!store.consume_event(audit.group(), id)?);
    Ok(())
}

#[tokio::test]
async fn scoped_subscription_reloads_authoritative_facts_and_skips_invisible_rows() -> Result<()> {
    let store = Store::open(":memory:")?;
    let keep = store.post_event("team", json!({"type":"report"}), None)?;
    let other_owner = store.post_event("other", json!({"type":"report"}), None)?;
    let other_type = store.post_event("team", json!({"type":"settings.changed"}), None)?;
    let bus = TestBus::default();
    let sub = Subscription::scoped("a", "team", vec!["report".into()]);
    assert_eq!(publish_pending(&store, &bus).await?, 3);
    bus.publish("foreign-stream-entry").await?;
    assert_eq!(
        consume_batch(&store, &bus, &sub, "w", &mut "0-0".into()).await?,
        4
    );
    assert!(!store.consume_event(sub.group(), event_key(&keep)?)?);
    // The first registration of these markers succeeds: neither was consumed by the subscription.
    assert!(store.consume_event(sub.group(), event_key(&other_owner)?)?);
    assert!(store.consume_event(sub.group(), event_key(&other_type)?)?);
    Ok(())
}

#[tokio::test]
#[ignore = "set LOOM_TEST_REDIS_URL to a Redis 6.2+ test instance and run with --ignored"]
async fn redis_workers_compete_within_group_and_reclaim_an_unacked_entry() -> Result<()> {
    let url = std::env::var("LOOM_TEST_REDIS_URL")?;
    let stream = format!("loom:bus:test:{}", uuid::Uuid::new_v4());
    let bus = RedisBus::new(&url, &stream)?;
    bus.publish("e1").await?;
    let batch = bus.receive("runtime", "old", "0-0").await?;
    assert_eq!(batch.entries.len(), 1);
    assert!(
        bus.receive("runtime", "new", "0-0")
            .await?
            .entries
            .is_empty()
    );
    let audit = bus.receive("audit", "audit-1", "0-0").await?;
    assert_eq!(audit.entries[0].event_id, "e1");
    bus.ack("audit", &audit.entries[0].receipt).await?;
    // Give the abandoned lease time to expire. The old worker never ACKs it.
    tokio::time::sleep(Duration::from_secs(11)).await;
    let reclaimed = bus.receive("runtime", "new", "0-0").await?;
    assert_eq!(reclaimed.entries[0].receipt, batch.entries[0].receipt);
    bus.ack("runtime", &reclaimed.entries[0].receipt).await?;
    assert!(
        bus.receive("audit", "audit-2", "0-0")
            .await?
            .entries
            .is_empty()
    );
    let mut conn = redis::Client::open(url)?
        .get_multiplexed_async_connection()
        .await?;
    redis::cmd("DEL")
        .arg(stream)
        .query_async::<usize>(&mut conn)
        .await?;
    Ok(())
}

#[tokio::test]
async fn installed_lua_subscription_persists_only_its_authorized_result() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let catalog = crate::catalog::Catalog::open(dir.path()).map_err(anyhow::Error::msg)?;
    let store = Store::open(dir.path().join("events.db"))?;
    let code = r#"
        plugin:subscribe({id="report-log", types={"job.finished"}, source="ci"}, function(ctx,event)
            return {error=event.error, instance=ctx.plugin_instance}
        end)
    "#;
    catalog
        .manage(
            "admin",
            true,
            "POST",
            "/api/plugins/audit/versions",
            json!({"version":"1","manifest":{},"code":code}),
        )
        .await
        .map_err(anyhow::Error::msg)?;
    catalog.manage("admin", true, "POST", "/api/plugins",
        json!({"id":"audit-team","package":"audit","version":"1","enabled":true,"owner":"team"}))
        .await.map_err(anyhow::Error::msg)?;
    let transport = Arc::new(TestBus::default());
    let event = store.post_event(
        "team",
        json!({"source":"ci","type":"job.finished","error":"E421"}),
        None,
    )?;
    let excluded =
        store.post_event("team", json!({"source":"docs","type":"job.finished"}), None)?;
    let foreign = store.post_event("other", json!({"source":"ci","type":"job.finished"}), None)?;
    publish_pending(&store, &*transport).await?;
    transport.publish(event_key(&event)?).await?;
    let handle = spawn_plugin_bus(
        store.clone(),
        transport.clone(),
        "w".into(),
        catalog.clone(),
    );
    let group = subscription_group("team", "audit-team", "report-log");
    let read = |event: &Value| store.subscription_result(&group, event_key(event).unwrap());
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if transport.acknowledged.lock().unwrap().len() == 4 {
                break;
            }
            sleep(Duration::from_millis(30)).await;
        }
    })
    .await;
    handle.abort();
    result?;
    assert_eq!(
        read(&event)?.unwrap()["result"],
        json!({"error":"E421","instance":"audit-team"})
    );
    assert!(read(&excluded)?.is_none());
    assert!(read(&foreign)?.is_none());
    catalog
        .manage(
            "admin",
            true,
            "PATCH",
            "/api/plugins/audit-team",
            json!({"enabled":false}),
        )
        .await
        .map_err(anyhow::Error::msg)?;
    assert!(
        catalog
            .subscription_plugins()
            .map_err(anyhow::Error::msg)?
            .is_empty()
    );
    Ok(())
}
