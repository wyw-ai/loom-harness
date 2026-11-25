use super::*;
use anyhow::Result;
use rusqlite::Connection;
use serde_json::json;
use std::{
    collections::HashSet,
    sync::{Arc, Barrier},
};

#[tokio::test]
async fn groups_replay_committed_events_across_backend_changes_and_restart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("loom.db");
    let store = Store::open(&path)?;
    let old = store.post_event("team", json!({"type":"report"}), None)?;
    let old_id = old["id"].as_str().unwrap();
    store.published(old_id)?;
    let new = store.post_event("team", json!({"type":"report"}), None)?;
    let new_id = new["id"].as_str().unwrap();
    let bus = LocalBus::new(store);
    let entries = bus.receive("runtime", "old", "").await?.entries;
    assert_eq!(
        entries
            .iter()
            .map(|e| e.event_id.as_str())
            .collect::<Vec<_>>(),
        [old_id, new_id]
    );
    // A worker died after claiming both but acknowledged the later event first.
    bus.ack("runtime", &entries[1].receipt).await?;
    drop(bus);

    let store = Store::open(&path)?;
    let db = Connection::open(&path)?;
    let resumed = LocalBus::new(store.clone());
    let audit = resumed.receive("audit", "new", "").await?.entries;
    assert_eq!(audit.len(), 2);
    for entry in audit {
        resumed.ack("audit", &entry.receipt).await?;
    }
    assert!(
        resumed
            .receive("runtime", "new", "")
            .await?
            .entries
            .is_empty()
    );
    db.execute(
        "UPDATE local_bus_pending SET expires_ms=0 WHERE consumer='runtime'",
        [],
    )?;
    let redelivered = resumed.receive("runtime", "new", "").await?.entries;
    assert_eq!(redelivered.len(), 1);
    assert_eq!(redelivered[0].event_id, old_id);
    assert_ne!(redelivered[0].receipt, entries[0].receipt);
    resumed.ack("runtime", &entries[0].receipt).await?;
    let unresolved: i64 =
        db.query_row("SELECT count(*) FROM local_bus_pending", [], |r| r.get(0))?;
    assert_eq!(unresolved, 1, "late ack must not release the new claim");
    resumed.ack("runtime", &redelivered[0].receipt).await?;
    // Once drained, neither group needs the writer slot to poll for events.
    db.execute_batch("BEGIN IMMEDIATE")?;
    for group in ["runtime", "audit"] {
        assert!(resumed.receive(group, "new", "").await?.entries.is_empty());
    }
    db.execute_batch("ROLLBACK")?;
    assert_eq!(store.pending_events(10)?[0]["id"], new_id);
    Ok(())
}

#[test]
fn workers_claim_disjoint_batches_across_connections() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("loom.db");
    let first = Store::open(&path)?;
    let second = Store::open(&path)?;
    for _ in 0..BATCH * 2 {
        first.post_event("team", json!({"type":"report"}), None)?;
    }
    let barrier = Arc::new(Barrier::new(2));
    let workers = [first, second].map(|store| {
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            store.claim_local_events("runtime", BATCH, 10_000)
        })
    });
    let mut seen = HashSet::new();
    for worker in workers {
        let claimed = worker.join().unwrap()?;
        assert_eq!(claimed.len(), BATCH);
        for (_, _, event_id) in claimed {
            assert!(seen.insert(event_id));
        }
    }
    Ok(())
}
