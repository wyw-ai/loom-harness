use super::*;

fn setup(store: &Store) -> (String, String, Work) {
    let session = store
        .create_session("u", json!({"agent_id":"reader"}))
        .unwrap();
    let event = store
        .post_event(
            "u",
            json!({"type":"message","text":"check CI"}),
            Some("first"),
        )
        .unwrap();
    let d = store
        .deliver(
            "u",
            event["id"].as_str().unwrap(),
            json!({"target":{"session_id":session["id"]},
        "action":"start_run","agent_snapshot":{"id":"reader","revision":12}}),
            Some("start"),
        )
        .unwrap();
    let work = store.claim("one", 120).unwrap().unwrap();
    assert_eq!(work.run["agent_snapshot"]["revision"], 12);
    (
        session["id"].as_str().unwrap().into(),
        d["run_id"].as_str().unwrap().into(),
        work,
    )
}

#[test]
fn retries_and_tenant_reads_do_not_duplicate_runs_or_history() {
    let s = Store::open(":memory:").unwrap();
    let (sid, rid, _) = setup(&s);
    let event = s
        .post_event(
            "u",
            json!({"type":"message","text":"check CI"}),
            Some("first"),
        )
        .unwrap();
    let d = s
        .deliver(
            "u",
            event["id"].as_str().unwrap(),
            json!({"target":{"session_id":sid},
        "action":"start_run","agent_snapshot":{"revision":13}}),
            Some("start"),
        )
        .unwrap();
    assert_eq!(d["run_id"], rid);
    assert_eq!(s.history("u", &sid).unwrap().as_array().unwrap().len(), 1);
    assert!(matches!(s.get("v", "run", &rid), Err(StoreError::NotFound)));
    assert!(matches!(
        s.post_event("u", json!({"text":"oops"}), Some("first")),
        Err(StoreError::Conflict(_))
    ));
    let id = event["id"].as_str().unwrap();
    assert!(s.pending_events(100).unwrap().iter().any(|e| e["id"] == id));
    assert!(s.consume_event("audit", id).unwrap());
    assert!(!s.consume_event("audit", id).unwrap());
    assert!(s.consume_event("runner", id).unwrap());
    s.published(id).unwrap();
    assert!(!s.pending_events(100).unwrap().iter().any(|e| e["id"] == id));
}

#[test]
fn recovery_reclaims_once_and_fences_old_worker() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("loom.db");
    let a = Store::open(&p).unwrap();
    let (_, _, old) = setup(&a);
    let b = Store::open(&p).unwrap();
    assert!(b.claim("two", 120).unwrap().is_none());
    a.tx(|c| {
        c.execute("UPDATE workq SET expires=unixepoch()-1", [])?;
        Ok(())
    })
    .unwrap();
    let resumed = b.claim("two", 120).unwrap().unwrap();
    assert_eq!(old.run["id"], resumed.run["id"]);
    assert_ne!(old.token, resumed.token);
    assert!(!a.renew(&old, 60).unwrap());
    assert!(matches!(
        a.finish(&old, "completed", json!({})),
        Err(StoreError::StaleLease)
    ));
    b.finish(&resumed, "completed", json!({"output":"verified"}))
        .unwrap();
    assert!(a.claim("one", 120).unwrap().is_none());
    let history = a
        .history("u", resumed.run["session_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(history[1]["content"], "verified");
}

#[test]
fn concurrent_workers_claim_one_ready_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("loom.db");
    let store = Store::open(&path).unwrap();
    let (_, _, old) = setup(&store);
    store
        .tx(|c| {
            c.execute("UPDATE workq SET expires=unixepoch()-1", [])?;
            Ok(())
        })
        .unwrap();

    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let workers: Vec<_> = ["one", "two"]
        .into_iter()
        .map(|name| {
            let db = Store::open(&path).unwrap();
            let start = start.clone();
            std::thread::spawn(move || {
                start.wait();
                db.claim(name, 60).unwrap()
            })
        })
        .collect();
    let winners: Vec<_> = workers
        .into_iter()
        .filter_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(winners.len(), 1);
    assert_eq!(winners[0].run["step"], old.run["step"]);
    assert!(matches!(
        store.finish(&old, "completed", json!({})),
        Err(StoreError::StaleLease)
    ));
    store
        .finish(&winners[0], "completed", json!({"output":"one answer"}))
        .unwrap();
}

#[test]
fn early_and_late_receipts_awaken_only_when_all_children_terminate() {
    let s = Store::open(":memory:").unwrap();
    let (_, rid, w) = setup(&s);
    let first = s
        .record_call(&w, "tool_call", json!({"call_key":"0:a"}))
        .unwrap();
    let same = s
        .record_call(&w, "tool_call", json!({"call_key":"0:a"}))
        .unwrap();
    assert_eq!(same["id"], first["id"]);
    assert_eq!(same["_created"], false);
    let a = first["id"].as_str().unwrap();
    let second = s
        .record_call(&w, "tool_call", json!({"call_key":"0:b"}))
        .unwrap();
    let b = second["id"].as_str().unwrap();
    s.receive_tool_result("u", a, json!({"result":2}), "r1")
        .unwrap();
    s.receive_tool_result("u", b, json!({"status":"accepted","job_id":"T42"}), "r2")
        .unwrap();
    assert_eq!(
        s.finish(&w, "waiting", json!({"waiting_for":[a,b]}))
            .unwrap()["status"],
        "waiting"
    );
    assert!(s.claim("two", 120).unwrap().is_none());
    let result = json!({"status":"failed","error":"E421"});
    s.receive_tool_result("u", b, result.clone(), "r3").unwrap();
    s.receive_tool_result("u", b, result, "another-delivery")
        .unwrap();
    assert!(s.claim("two", 120).unwrap().is_none());
    for entry in s.pending_events(100).unwrap() {
        s.consume_event("runtime", entry["id"].as_str().unwrap())
            .unwrap();
    }
    let next = s.claim("two", 120).unwrap().unwrap();
    assert_eq!(next.run["id"], rid);
    assert_eq!(next.run["wake_reason"], "tool_result");
    let events = s.list("u", "event", None).unwrap();
    assert_eq!(
        events
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["type"] == "run.wait_satisfied")
            .count(),
        1
    );
}

#[test]
fn cancellation_fences_worker_and_late_receipt_only_updates_call() {
    let s = Store::open(":memory:").unwrap();
    let (sid, rid, w) = setup(&s);
    let call = s
        .record_call(&w, "tool_call", json!({"name":"ci"}))
        .unwrap();
    let stop = s.post_event("u", json!({"text":"stop"}), None).unwrap();
    s.deliver(
        "u",
        stop["id"].as_str().unwrap(),
        json!({"run_id":rid,"target":{"session_id":sid},"action":"request_cancel"}),
        None,
    )
    .unwrap();
    assert!(matches!(
        s.record_call(&w, "tool_call", json!({"name":"more"})),
        Err(StoreError::StaleLease)
    ));
    s.receive_tool_result(
        "u",
        call["id"].as_str().unwrap(),
        json!({"status":"failed","error":"CI failed"}),
        "late",
    )
    .unwrap();
    assert!(s.claim("two", 120).unwrap().is_none());
    assert_eq!(s.get("u", "run", &rid).unwrap()["status"], "cancelled");
}

#[test]
fn approval_arriving_before_wait_is_not_lost() {
    let store = Store::open(":memory:").unwrap();
    let (_, _, work) = setup(&store);
    let args = json!({"environment":"test"});
    let call = store
        .record_call(
            &work,
            "tool_call",
            json!({"arguments":args,"status":"awaiting_approval"}),
        )
        .unwrap();
    let cid = call["id"].as_str().unwrap();
    store
        .decide_approval("u", cid, "approver", true, &args, "once")
        .unwrap();
    let saved = store
        .finish(&work, "waiting", json!({"waiting_for":[cid]}))
        .unwrap();
    assert_eq!(saved["status"], "queued");
    let next = store.claim("other", 20).unwrap().unwrap();
    assert_eq!(next.run["approved_tool_call_id"], cid);
    assert_eq!(next.run["wake_reason"], "approval");
}
