use super::*;
use crate::store::Store;
use axum::response::{
    Sse,
    sse::{Event, KeepAlive},
};
use futures_util::stream;
use std::{collections::VecDeque, time::Duration};

struct Feed {
    db: Store,
    owner: String,
    run_id: String,
    cursor: i64,
    pending: VecDeque<Event>,
    closed: bool,
    delay: Option<Duration>,
}

impl Feed {
    fn read(&mut self) -> Result<(), StoreError> {
        let page = self
            .db
            .stream_page(&self.owner, &self.run_id, self.cursor)?;
        if page.rows.is_empty() {
            match page.status.as_str() {
                "completed" | "failed" | "cancelled" => {
                    self.closed = true;
                    self.pending.push_back(
                        Event::default()
                            .event("run_end")
                            .id("end")
                            .data(json!({"run_id":self.run_id,"status":page.status}).to_string()),
                    );
                }
                "waiting" => self.delay = Some(Duration::from_millis(150)),
                _ => self.delay = Some(Duration::from_millis(10)),
            }
        }
        for row in page.rows {
            let mut payload = json!({"run_id":self.run_id,"model_call_id":row.model_call_id,
                "attempt_id":row.attempt_id,"provisional":true});
            if let Some(text) = row.text {
                payload["text"] = Value::String(text);
            }
            self.pending.push_back(
                Event::default()
                    .id(row.seq.to_string())
                    .event(row.event)
                    .data(payload.to_string()),
            );
            self.cursor = row.seq;
        }
        Ok(())
    }
}

pub(super) fn open(
    app: &App,
    who: &Identity,
    run_id: &str,
    headers: &HeaderMap,
    after: Option<&String>,
) -> Result<Response, ApiError> {
    let resume = headers
        .get("last-event-id")
        .map(|h| h.to_str().map_err(|_| bad("invalid Last-Event-ID")))
        .transpose()?
        .or(after.map(String::as_str))
        .unwrap_or("0");
    if resume == "end" {
        let run = app.db.get(&who.owner, "run", run_id)?;
        return if matches!(
            run["status"].as_str(),
            Some("completed" | "failed" | "cancelled")
        ) {
            Ok(StatusCode::NO_CONTENT.into_response())
        } else {
            Err(bad("end cursor on active run"))
        };
    }
    let cursor = resume
        .parse::<i64>()
        .ok()
        .filter(|v| *v >= 0)
        .ok_or_else(|| bad("invalid stream cursor"))?;
    let mut feed = Feed {
        db: app.db.clone(),
        owner: who.owner.clone(),
        run_id: run_id.to_owned(),
        cursor,
        pending: VecDeque::new(),
        closed: false,
        delay: None,
    };
    // Resolve authorization and the initial page before sending HTTP headers.
    feed.read()?;
    let events = stream::try_unfold(feed, |mut feed| async move {
        loop {
            if let Some(event) = feed.pending.pop_front() {
                return Ok::<_, StoreError>(Some((event, feed)));
            }
            if feed.closed {
                return Ok(None);
            }
            if let Some(delay) = feed.delay.take() {
                tokio::time::sleep(delay).await;
            }
            feed.read()?;
        }
    });
    Ok((
        [("x-accel-buffering", "no"), ("cache-control", "no-store")],
        Sse::new(events).keep_alive(KeepAlive::default()),
    )
        .into_response())
}
