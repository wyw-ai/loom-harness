use super::{Catalog, R};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub(super) fn id(s: &str) -> R<&str> {
    let first = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(first.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            first
                .strip_prefix(p)
                .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
        });
    if s.is_empty()
        || device
        || s.len() > 128
        || s == "."
        || s == ".."
        || s.starts_with('.')
        || s.ends_with('.')
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        return Err("invalid: resource identifier".into());
    }
    Ok(s)
}
pub(super) fn safe(root: &Path, relative: &Path) -> R<PathBuf> {
    let mut p = root.to_path_buf();
    for c in relative.components() {
        let Component::Normal(n) = c else {
            return Err("invalid: resource path".into());
        };
        p.push(n);
        if let Ok(m) = fs::symlink_metadata(&p)
            && linked(&m)
        {
            return Err("invalid: symlink in resource path".into());
        }
    }
    if p.exists() && !p.canonicalize().map_err(err)?.starts_with(root) {
        return Err("invalid: outside HOME".into());
    }
    Ok(p)
}
pub(super) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub(super) fn sha(v: &Value) -> String {
    format!("{:x}", Sha256::digest(v.to_string().as_bytes()))
}
fn linked(m: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    m.file_type().is_symlink()
}
pub(super) fn dirs(p: &Path) -> R<Vec<PathBuf>> {
    if !p.exists() {
        return Ok(vec![]);
    }
    let mut out = vec![];
    for entry in fs::read_dir(p).map_err(err)? {
        let entry = entry.map_err(err)?;
        if !linked(&fs::symlink_metadata(entry.path()).map_err(err)?) {
            out.push(entry.path())
        }
    }
    out.sort();
    Ok(out)
}
pub(super) fn write(p: &Path, data: &[u8]) -> R<()> {
    fs::create_dir_all(p.parent().ok_or("invalid path")?).map_err(err)?;
    let temp = p.with_extension(format!("loom-tmp-{}", uuid::Uuid::new_v4().simple()));
    fs::write(&temp, data).map_err(err)?;
    fs::rename(&temp, p).map_err(err)
}
impl Catalog {
    pub(super) fn db(&self) -> R<Connection> {
        let db = Connection::open(self.home.join("state/loom.db")).map_err(err)?;
        db.busy_timeout(Duration::from_secs(15)).map_err(err)?;
        Ok(db)
    }
    pub(super) fn initialize(&self) -> R<()> {
        let db = self.db()?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS catalog_current(kind TEXT NOT NULL,id TEXT NOT NULL,rev TEXT NOT NULL,PRIMARY KEY(kind,id));
        CREATE TABLE IF NOT EXISTS catalog_versions(kind TEXT NOT NULL,id TEXT NOT NULL,rev TEXT NOT NULL,document TEXT NOT NULL,PRIMARY KEY(kind,id,rev));
        CREATE TABLE IF NOT EXISTS catalog_diagnostics(kind TEXT,id TEXT,details TEXT,PRIMARY KEY(kind,id));
        CREATE TABLE IF NOT EXISTS catalog_refresh(id TEXT PRIMARY KEY,generation INTEGER NOT NULL);").map_err(err)
    }
    pub(super) fn get(&self, kind: &str, key: &str) -> R<Value> {
        self.db()?.query_row("SELECT v.document FROM catalog_current c JOIN catalog_versions v USING(kind,id,rev) WHERE c.kind=?1 AND c.id=?2",params![kind,key],|r|r.get::<_,String>(0)).optional().map_err(err)?
            .map(|s|serde_json::from_str(&s).map_err(err)).transpose()?.ok_or_else(||format!("not found: {kind} {key}"))
    }
    pub(super) fn version(&self, kind: &str, key: &str, revision: &str) -> R<Value> {
        let s = self
            .db()?
            .query_row(
                "SELECT document FROM catalog_versions WHERE kind=?1 AND id=?2 AND rev=?3",
                params![kind, key, revision],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(err)?
            .ok_or("not found: resource revision")?;
        serde_json::from_str(&s).map_err(err)
    }
    pub(super) fn all(&self, kind: &str) -> R<Vec<Value>> {
        let db = self.db()?;
        let mut stmt=db.prepare("SELECT v.document FROM catalog_current c JOIN catalog_versions v USING(kind,id,rev) WHERE c.kind=?1 ORDER BY c.id").map_err(err)?;
        let rows = stmt
            .query_map([kind], |r| r.get::<_, String>(0))
            .map_err(err)?;
        rows.map(|row| serde_json::from_str(&row.map_err(err)?).map_err(err))
            .collect()
    }
    // The write lock covers the expected revision check, file replacement and effective version.
    pub(super) fn save(
        &self,
        kind: &str,
        key: &str,
        mut value: Value,
        expect: Option<&str>,
        file: Option<&Path>,
    ) -> R<Value> {
        let mut db = self.db()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        if file.is_some() {
            let alias:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM catalog_current WHERE kind=?1 AND lower(id)=lower(?2) AND id<>?2)",params![kind,key],|r|r.get(0)).map_err(err)?;
            if alias {
                return Err("conflict: resource identifier differs only in case".into());
            }
        }
        let old = tx
            .query_row(
                "SELECT rev FROM catalog_current WHERE kind=?1 AND id=?2",
                params![kind, key],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(err)?;
        if let Some(e) = expect
            && old.as_deref().unwrap_or("") != e
        {
            return Err("conflict: stale resource revision".into());
        }
        let m = value
            .as_object_mut()
            .ok_or("invalid: expected resource object")?;
        m.remove("revision");
        m.remove("expected_revision");
        let rev = sha(&value);
        value["revision"] = json!(rev);
        if let Some(p) = file {
            write(p, serde_yaml::to_string(&value).map_err(err)?.as_bytes())?
        }
        tx.execute(
            "INSERT OR IGNORE INTO catalog_versions VALUES(?1,?2,?3,?4)",
            params![kind, key, rev, value.to_string()],
        )
        .map_err(err)?;
        tx.execute("INSERT INTO catalog_current VALUES(?1,?2,?3) ON CONFLICT(kind,id) DO UPDATE SET rev=excluded.rev",params![kind,key,rev]).map_err(err)?;
        tx.execute(
            "DELETE FROM catalog_diagnostics WHERE kind=?1 AND id=?2",
            params![kind, key],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(value)
    }
    pub(super) fn diagnose(&self, kind: &str, key: &str, error: &str, pending: &Value) -> R<()> {
        let val = json!({"kind":kind,"id":key,"error":error,"pending":pending,"effective_revision":self.get(kind,key).ok().map(|v|v["revision"].clone())});
        self.db()?
            .execute(
                "INSERT OR REPLACE INTO catalog_diagnostics VALUES(?1,?2,?3)",
                params![kind, key, val.to_string()],
            )
            .map_err(err)?;
        Ok(())
    }
    pub(super) fn diagnostics(&self) -> R<Value> {
        let db = self.db()?;
        let mut st = db
            .prepare("SELECT details FROM catalog_diagnostics ORDER BY kind,id")
            .map_err(err)?;
        let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
        let mut out = vec![];
        for r in rows {
            out.push(serde_json::from_str::<Value>(&r.map_err(err)?).map_err(err)?)
        }
        Ok(json!(out))
    }
}
