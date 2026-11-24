use super::{work::run_for_lease, *};
use std::time::Instant;

pub struct TextStreamWriter {
    store: Store,
    run_id: String,
    token: String,
    model_call_id: String,
    attempt_id: String,
    pending: String,
    last_write: Option<Instant>,
}

pub(crate) struct StreamRow {
    pub seq: i64,
    pub event: String,
    pub model_call_id: String,
    pub attempt_id: String,
    pub text: Option<String>,
}

pub(crate) struct StreamPage {
    pub rows: Vec<StreamRow>,
    pub status: String,
}

pub(crate) const PAGE_SIZE: usize = 64;

impl Store {
    pub fn start_text_stream(&self, work: &Work, model_call_id: &str) -> Result<TextStreamWriter> {
        let attempt_id = id("attempt");
        let run_id = field(&work.run, "id")?.to_owned();
        self.tx(|c| {
            run_for_lease(c, work)?;
            let eligible: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM objects WHERE kind='model_call' AND id=? AND parent=?
                 AND owner=? AND json_extract(doc,'$.status')='pending')",
                params![model_call_id, run_id, field(&work.run, "owner")?],
                |r| r.get(0),
            )?;
            if !eligible {
                return Err(StoreError::Conflict(
                    "model call is not pending on this run".into(),
                ));
            }
            c.execute(
                "INSERT INTO run_stream(run_id,model_call_id,attempt_id,event)
                 VALUES(?,?,?,'attempt_started')",
                params![run_id, model_call_id, attempt_id],
            )?;
            Ok(())
        })?;
        Ok(TextStreamWriter {
            store: self.clone(),
            run_id,
            token: work.token.clone(),
            model_call_id: model_call_id.to_owned(),
            attempt_id,
            pending: String::new(),
            last_write: None,
        })
    }

    pub(crate) fn stream_page(&self, owner: &str, run_id: &str, after: i64) -> Result<StreamPage> {
        self.read(|c| {
            // Read state and deltas in one SQLite snapshot so a concurrent Run completion
            // can never close the stream ahead of its final committed delta.
            let mut stmt = c.prepare_cached(
                "SELECT CASE WHEN q.ready=1 THEN 'active' ELSE json_extract(r.doc,'$.status') END,
                        s.seq,s.event,s.model_call_id,s.attempt_id,s.text
                 FROM objects r JOIN workq q ON q.run_id=r.id
                 LEFT JOIN run_stream s ON s.run_id=r.id AND s.seq>?
                 WHERE r.kind='run' AND r.owner=? AND r.id=? ORDER BY s.seq LIMIT ?",
            )?;
            let mut result = stmt.query(params![after, owner, run_id, PAGE_SIZE])?;
            let mut rows = Vec::new();
            let mut status = None;
            while let Some(r) = result.next()? {
                if status.is_none() {
                    status = Some(r.get(0)?);
                }
                if let Some(seq) = r.get::<_, Option<i64>>(1)? {
                    rows.push(StreamRow {
                        seq,
                        event: r.get(2)?,
                        model_call_id: r.get(3)?,
                        attempt_id: r.get(4)?,
                        text: r.get(5)?,
                    });
                }
            }
            Ok(StreamPage {
                rows,
                status: status.ok_or(StoreError::NotFound)?,
            })
        })
    }
}

impl TextStreamWriter {
    pub fn push(&mut self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        self.pending.push_str(text);
        if self
            .last_write
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(30))
            || self.pending.len() >= 2048
        {
            self.store
                .read(|c| self.append(c, "text_delta", Some(&self.pending)))?;
            self.pending.clear();
            self.last_write = Some(Instant::now());
        }
        Ok(())
    }

    pub fn finish(self, accepted: bool) -> Result<()> {
        self.store.tx(|c| {
            if accepted && !self.pending.is_empty() {
                self.append(c, "text_delta", Some(&self.pending))?;
            }
            self.append(
                c,
                if accepted {
                    "attempt_finished"
                } else {
                    "attempt_discarded"
                },
                None,
            )
        })
    }

    fn append(&self, c: &Connection, event: &str, text: Option<&str>) -> Result<()> {
        // Fence the write itself, including after cancellation or replacement by a new attempt.
        let written = c.execute(
            "INSERT INTO run_stream(run_id,model_call_id,attempt_id,event,text)
             SELECT ?1,?2,?3,?4,?5
             WHERE EXISTS(SELECT 1 FROM workq WHERE run_id=?1 AND token=?6
                          AND ready=1 AND expires>unixepoch())
             AND (SELECT attempt_id=?3 AND event IN ('attempt_started','text_delta')
                  FROM run_stream WHERE model_call_id=?2 ORDER BY seq DESC LIMIT 1)",
            params![
                self.run_id,
                self.model_call_id,
                self.attempt_id,
                event,
                text,
                self.token
            ],
        )?;
        if written == 0 {
            return Err(StoreError::StaleLease);
        }
        Ok(())
    }
}
