use super::*;

const RECENT_LIMIT: i64 = 6;

impl Store {
    pub fn console_overview(&self, owner: &str) -> Result<Value> {
        self.read(|c| {
            // Counts and rows must describe one snapshot, including when another
            // worker commits a Run while this process is reading the overview.
            let snapshot = c.unchecked_transaction()?;
            let session_count: i64 = snapshot.query_row(
                "SELECT count(*) FROM objects WHERE owner=? AND kind='session'",
                [owner], |row| row.get(0),
            )?;
            let run_counts: String = snapshot.query_row(
                "SELECT json_group_object(status, n) FROM (
                     SELECT json_extract(doc,'$.status') AS status, count(*) AS n
                     FROM objects WHERE owner=? AND kind='run' GROUP BY status
                 )", [owner], |row| row.get(0),
            )?;
            let run_counts: Value = serde_json::from_str(&run_counts)?;
            let recent_runs = fetch_many(&snapshot,
                "SELECT json_object('id',id,'agent_id',doc->>'$.agent_id',
                         'session_id',parent,'status',doc->>'$.status','created_at',doc->>'$.created_at')
                 FROM objects WHERE owner=?1 AND kind='run' ORDER BY seq DESC LIMIT ?2",
                params![owner, RECENT_LIMIT],
            )?;
            let mut recent_sessions = fetch_many(&snapshot,
                "SELECT json_object('id',id,'agent_id',doc->>'$.agent_id',
                         'title',doc->>'$.title','created_at',doc->>'$.created_at')
                 FROM objects WHERE owner=?1 AND kind='session' ORDER BY seq DESC LIMIT ?2",
                params![owner, RECENT_LIMIT],
            )?;
            add_session_previews(&snapshot, owner, &mut recent_sessions)?;
            snapshot.commit()?;
            Ok(json!({ "session_count": session_count, "run_counts": run_counts,
                "recent_sessions": recent_sessions, "recent_runs": recent_runs }))
        })
    }
}
