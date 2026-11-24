use super::*;
use crate::config::{Config, Provider};
use axum::{Json, Router, routing::post};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::{net::TcpListener, sync::Mutex};

fn open_app(home: &Path, endpoint: &str) -> App {
    let catalog = crate::catalog::Catalog::open(home).unwrap();
    let provider = Provider {
        base_url: endpoint.to_owned(),
        model: "fixture".into(),
        api_key_env: None,
        images: false,
    };
    let cfg = Config {
        home: home.to_owned(),
        bind: String::new(),
        redis: String::new(),
        role: "worker".into(),
        workers: 1,
        lease_seconds: 10,
        tokens: BTreeMap::new(),
        models: BTreeMap::from([("fixture".into(), provider)]),
        default_model: Some("fixture".into()),
        max_rounds: 4,
        note_threshold: 16384,
        note_budget: 65536,
        notification_targets: Vec::new(),
    };
    App {
        cfg: Arc::new(cfg),
        db: crate::store::Store::open(home.join("state/loom.db")).unwrap(),
        catalog,
        http: reqwest::Client::new(),
        notes: Arc::new(crate::context::NoteStore::new(home)),
        ready: Default::default(),
        suggestion_slots: Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

async fn recover_initial_tool_registration(first_child_registered: bool) {
    let home = tempfile::tempdir().unwrap();
    let replies = Arc::new(Mutex::new(Vec::<Value>::new()));
    let observed = replies.clone();
    let model=Router::new().route("/v1/chat/completions",post(move |Json(request):Json<Value>|{
        let observed=observed.clone();
        async move {
            observed.lock().await.push(request);
            Json(json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"Note saved."}}]}))
        }
    }));
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1", socket.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(socket, model).await.unwrap() });
    let code =
        "return tools.call('write_note',{description='recovery witness',body='persist once'})";
    let (session_id, run_id, parent_id, first_id) = {
        let old = open_app(home.path(), &endpoint);
        let agent = old
            .catalog
            .create_agent(
                "u",
                false,
                json!({"id":"reader","template":"standard","model":"fixture"}),
            )
            .unwrap();
        let session = old
            .db
            .create_session("u", json!({"agent_id":"reader"}))
            .unwrap();
        let event = old
            .db
            .post_event("u", json!({"content":"save the note"}), None)
            .unwrap();
        let delivery=old.db.deliver("u",event["id"].as_str().unwrap(),json!({"action":"start_run","target":{"session_id":session["id"]},"agent_snapshot":agent}),None).unwrap();
        let job = old
            .db
            .claim("worker-before-interruption", 1)
            .unwrap()
            .unwrap();
        let directory = old.catalog.tools("u", false, "reader").unwrap();
        let args = json!({"code":code});
        let message = json!({"role":"assistant","content":null,"tool_calls":[{"id":"provider-tool","type":"function",
                           "function":{"name":"code_exec","arguments":args.to_string()}}]});
        let model=old.db.record_call(&job,"model_call",json!({"call_key":"model:0:1:fixture","tool_directory":directory,"status":"pending"})).unwrap();
        old.db
            .update_call(
                &job,
                "model_call",
                model["id"].as_str().unwrap(),
                json!({"status":"completed","message":message}),
            )
            .unwrap();
        let outer=old.db.record_call(&job,"tool_call",json!({"call_key":"tool:0:1:0","tool_id":"code_exec","arguments":args,"entry":"code","directory":directory})).unwrap();
        let parent_id = outer["id"].as_str().unwrap().to_owned();
        let child_id=first_child_registered.then(||{
            let child=old.db.record_call(&job,"tool_call",json!({"call_key":format!("{parent_id}:0"),"parent_id":parent_id,
                "tool_id":"write_note","arguments":{"description":"recovery witness","body":"persist once"},"entry":"program","directory":directory})).unwrap();
            child["id"].as_str().unwrap().to_owned()
        });
        (
            session["id"].as_str().unwrap().to_owned(),
            delivery["run_id"].as_str().unwrap().to_owned(),
            parent_id,
            child_id,
        )
    };
    // No old VM, in-memory App, control input, tool receipt or forced database update survives.
    let app = open_app(home.path(), &endpoint);
    let next = tokio::spawn(worker(app.clone(), "worker-after-interruption".into()));
    let finished = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let run = app.db.get("u", "run", &run_id).unwrap();
            if matches!(
                run["status"].as_str(),
                Some("completed" | "failed" | "cancelled" | "waiting")
            ) {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("new worker did not settle the interrupted step");
    next.abort();
    server.abort();
    assert_eq!(finished["status"], "completed", "{finished}");
    let calls = objects(&app.db.list("u", "tool_call", Some(&run_id)).unwrap());
    assert_eq!(calls.len(), 2, "{calls:?}");
    let child = calls.iter().find(|c| c["tool_id"] == "write_note").unwrap();
    assert_eq!(child["parent_id"], parent_id);
    if let Some(id) = first_id {
        assert_eq!(child["id"], id);
    }
    assert!(calls.iter().all(|c| c["status"] == "completed"));
    let notes = app
        .notes
        .search(&session_id, Some("recovery witness"), None, 10)
        .unwrap();
    assert_eq!(
        notes.items.len(),
        1,
        "recording a call alone must not duplicate its external effect"
    );
    let observed = replies.lock().await;
    assert_eq!(
        observed.len(),
        1,
        "the saved model decision should not be sampled a second time"
    );
    assert!(observed[0]["messages"].as_array().unwrap().iter().any(|m| {
        m["role"] == "tool"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.contains(&notes.items[0].note_ref))
    }));
}

#[tokio::test]
async fn restart_after_outer_registration_runs_the_first_child_once() {
    recover_initial_tool_registration(false).await;
}

#[tokio::test]
async fn restart_after_first_child_registration_completes_that_child_once() {
    recover_initial_tool_registration(true).await;
}
