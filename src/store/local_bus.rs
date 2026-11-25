use super::{Result, Store};
use rusqlite::{Connection, Params, params};
use uuid::Uuid;

fn entries(c: &Connection, sql: &str, args: impl Params) -> Result<Vec<(i64, String)>> {
    Ok(c.prepare_cached(sql)?
        .query_map(args, |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

impl Store {
    // The cursor advances on claim. Only unacknowledged entries need individual
    // rows, so polling starts at the group's cursor even with a long history.
    pub(crate) fn claim_local_events(
        &self,
        group: &str,
        limit: usize,
        lease_ms: i64,
    ) -> Result<Vec<(i64, String, String)>> {
        // Idle groups only take a read lock. Repeat the selection inside the
        // write transaction since another process may claim the work first.
        let available = self.read(|c| {
            let mut check = c.prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM outbox WHERE seq >
                    COALESCE((SELECT last_claimed FROM local_bus_groups WHERE consumer=?1),0))
                     OR EXISTS(SELECT 1 FROM local_bus_pending WHERE consumer=?1 AND expires_ms<=?2)",
            )?;
            Ok(check.query_row(params![group, chrono::Utc::now().timestamp_millis()], |r| r.get::<_, bool>(0))?)
        })?;
        if !available {
            return Ok(vec![]);
        }

        self.tx(|c| {
            let now = chrono::Utc::now().timestamp_millis();
            let expired = entries(
                c,
                "SELECT p.seq,o.event_id FROM local_bus_pending p JOIN outbox o ON o.seq=p.seq
                 WHERE p.consumer=?1 AND p.expires_ms<=?2 ORDER BY p.expires_ms,p.seq LIMIT ?3",
                params![group, now, limit],
            )?;
            // Reserve capacity for new work even if a full batch keeps failing.
            let fresh = entries(
                c,
                "SELECT seq,event_id FROM outbox WHERE seq >
                 COALESCE((SELECT last_claimed FROM local_bus_groups WHERE consumer=?1),0)
                 ORDER BY seq LIMIT ?2",
                params![group, limit],
            )?;
            if let Some((new_last, _)) = fresh.last() {
                c.prepare_cached(
                    "INSERT INTO local_bus_groups(consumer,last_claimed) VALUES(?1,?2)
                     ON CONFLICT(consumer) DO UPDATE SET last_claimed=excluded.last_claimed",
                )?.execute(params![group, new_last])?;
            }

            let lease = Uuid::new_v4().simple().to_string();
            let expiry = now.saturating_add(lease_ms);
            let mut pending = c.prepare_cached(
                "INSERT INTO local_bus_pending(consumer,seq,lease,expires_ms) VALUES(?1,?2,?3,?4)
                 ON CONFLICT(consumer,seq) DO UPDATE SET lease=excluded.lease,expires_ms=excluded.expires_ms",
            )?;
            let mut claims = Vec::with_capacity(expired.len() + fresh.len());
            for (seq, event) in expired.into_iter().chain(fresh) {
                pending.execute(params![group, seq, lease, expiry])?;
                claims.push((seq, lease.clone(), event));
            }
            Ok(claims)
        })
    }

    pub(crate) fn ack_local_event(&self, group: &str, seq: i64, lease: &str) -> Result<()> {
        self.tx(|c| {
            c.prepare_cached(
                "DELETE FROM local_bus_pending WHERE consumer=?1 AND seq=?2 AND lease=?3",
            )?
            .execute(params![group, seq, lease])?;
            Ok(())
        })
    }
}
