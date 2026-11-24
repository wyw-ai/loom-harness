use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct NoteStore {
    home: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NoteMeta {
    pub note_ref: String,
    pub description: String,
    pub created_at: String,
    pub line_count: usize,
}

#[derive(Debug, Serialize)]
pub struct NotePage {
    pub items: Vec<NoteMeta>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct NoteRead {
    pub note_ref: String,
    pub body: Option<String>,
    pub line_count: usize,
    pub size_bytes: u64,
    pub over_budget: bool,
}

#[derive(Serialize, Deserialize)]
struct Header {
    description: String,
    created_at: String,
    line_count: usize,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn component(s: &str) -> io::Result<&str> {
    // HOME IDs are opaque, single path components on both Windows and Unix.
    if s.is_empty()
        || s.len() > 160
        || !s
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return Err(invalid("invalid Note or Session identifier"));
    }
    let windows_device = s.to_ascii_uppercase();
    if matches!(windows_device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|p| {
            windows_device
                .strip_prefix(p)
                .is_some_and(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_digit())
        })
    {
        return Err(invalid("reserved Note or Session identifier"));
    }
    Ok(s)
}

fn check_type(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid("unexpected Note storage link"));
        }
    }
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(invalid("unexpected Note storage entry"));
    }
    Ok(())
}

fn header(reader: &mut BufReader<File>) -> io::Result<Header> {
    // Metadata-only searches must not read the entire log into memory.
    let mut lines = (&mut *reader).take(64 * 1024).lines();
    if lines.next().transpose()?.as_deref() != Some("---") {
        return Err(invalid("missing Note header"));
    }
    let mut yaml = String::new();
    for line in lines {
        let line = line?;
        if line == "---" {
            return serde_yaml::from_str(&yaml).map_err(|_| invalid("invalid Note header"));
        }
        yaml.push_str(&line);
        yaml.push('\n');
    }
    Err(invalid("unterminated Note header"))
}

impl NoteStore {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    fn directory(&self, session: &str, create: bool) -> io::Result<PathBuf> {
        let session = component(session)?;
        let mut dir = self.home.clone();
        if create {
            fs::create_dir_all(&dir)?;
        }
        for entry in ["artifacts", "notes", session] {
            dir.push(entry);
            if create {
                match fs::create_dir(&dir) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e),
                }
            }
            check_type(&dir, true)?;
        }
        Ok(dir)
    }

    #[cfg(test)]
    pub fn write(&self, session: &str, description: &str, body: &str) -> io::Result<NoteMeta> {
        self.write_once(session, &Uuid::new_v4().to_string(), description, body)
    }

    pub fn externalize(
        &self,
        session: &str,
        tool_call_id: &str,
        block_index: usize,
        description: &str,
        body: &str,
    ) -> io::Result<NoteMeta> {
        if tool_call_id.trim().is_empty() {
            return Err(invalid("tool call ID is required"));
        }
        let id = format!(
            "tool-{:x}-{block_index}",
            Sha256::digest(tool_call_id.as_bytes())
        );
        self.write_once(session, &id, description, body)
    }

    /// A durable ToolCall/content-part key can be supplied for automatic
    /// externalization. Publishing a fully written file never replaces the
    /// original on receipt replay or concurrent worker recovery.
    pub fn write_once(
        &self,
        session: &str,
        id: &str,
        description: &str,
        body: &str,
    ) -> io::Result<NoteMeta> {
        let dir = self.directory(session, true)?;
        let id = component(id)?;
        if description.trim().is_empty() || description.len() > 4096 {
            return Err(invalid("Note description is empty or too long"));
        }
        let target = dir.join(format!("{id}.md"));
        let now = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
        let meta = Header {
            description: description.to_owned(),
            created_at: now,
            line_count: body.lines().count(),
        };
        let tmp = dir.join(format!(".{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
        let result = (|| {
            // JSON strings are valid YAML scalars, including escaped newlines
            // and delimiter text from user-controlled descriptions.
            writeln!(
                file,
                "---\ndescription: {}\ncreated_at: {}\nline_count: {}\n---",
                serde_json::to_string(&meta.description)?,
                serde_json::to_string(&meta.created_at)?,
                meta.line_count
            )?;
            file.write_all(body.as_bytes())?;
            file.sync_all()?;
            drop(file);
            match fs::hard_link(&tmp, &target) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    check_type(&target, false)?;
                    let mut reader = BufReader::new(File::open(&target)?);
                    let saved = header(&mut reader)?;
                    let mut remaining = body.as_bytes();
                    if saved.description != description {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "Note origin has conflicting metadata",
                        ));
                    }
                    loop {
                        let chunk = reader.fill_buf()?;
                        if chunk.is_empty() && remaining.is_empty() {
                            break;
                        }
                        if chunk.is_empty() || !remaining.starts_with(chunk) {
                            return Err(io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                "Note origin has conflicting content",
                            ));
                        }
                        let len = chunk.len();
                        remaining = &remaining[len..];
                        reader.consume(len);
                    }
                }
                Err(e) => return Err(e),
            }
            #[cfg(unix)]
            File::open(&dir)?.sync_all()?;
            self.metadata(session, id, &target)
        })();
        let _ = fs::remove_file(tmp);
        result
    }

    fn metadata(&self, session: &str, id: &str, path: &Path) -> io::Result<NoteMeta> {
        check_type(path, false)?;
        let raw = header(&mut BufReader::new(File::open(path)?))?;
        Ok(NoteMeta {
            note_ref: format!("note:{session}:{id}"),
            description: raw.description,
            created_at: raw.created_at,
            line_count: raw.line_count,
        })
    }

    pub fn search(
        &self,
        session: &str,
        keyword: Option<&str>,
        cursor: Option<&str>,
        limit: usize,
    ) -> io::Result<NotePage> {
        if limit == 0 {
            return Err(invalid("Note page size must be positive"));
        }
        let dir = match self.directory(session, false) {
            Ok(dir) => dir,
            Err(e) if e.kind() == io::ErrorKind::NotFound && cursor.is_none() => {
                return Ok(NotePage {
                    items: vec![],
                    next_cursor: None,
                });
            }
            Err(e) => return Err(e),
        };
        let mut items = Vec::new();
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| invalid("invalid Note filename"))?;
            let meta = self.metadata(session, component(id)?, &path)?;
            if meta.description.contains(keyword.unwrap_or("")) {
                items.push(meta);
            }
        }
        items.sort_unstable_by(|a, b| {
            (&b.created_at, &b.note_ref).cmp(&(&a.created_at, &a.note_ref))
        });
        let start = match cursor {
            None => 0,
            Some(last) => {
                items
                    .iter()
                    .position(|n| n.note_ref == last)
                    .ok_or_else(|| invalid("invalid Note cursor"))?
                    + 1
            }
        };
        let has_more = items.len().saturating_sub(start) > limit;
        let items: Vec<_> = items.into_iter().skip(start).take(limit).collect();
        let next_cursor = if has_more {
            items.last().map(|n| n.note_ref.clone())
        } else {
            None
        };
        Ok(NotePage { items, next_cursor })
    }

    pub fn read(
        &self,
        session: &str,
        note_ref: &str,
        start: Option<usize>,
        end: Option<usize>,
        max_bytes: usize,
    ) -> io::Result<NoteRead> {
        let dir = self.directory(session, false)?;
        let prefix = format!("note:{session}:");
        let id = note_ref.strip_prefix(&prefix).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Note is outside this Session",
            )
        })?;
        let path = dir.join(format!("{}.md", component(id)?));
        check_type(&path, false)?;
        let mut reader = BufReader::new(File::open(&path)?);
        let meta = header(&mut reader)?;
        let size = reader.get_ref().metadata()?.len() - reader.stream_position()?;
        if start == Some(0) || end == Some(0) || start.zip(end).is_some_and(|(a, b)| a > b) {
            return Err(invalid("invalid inclusive Note line range"));
        }
        let full = start.is_none() && end.is_none();
        let result = |body: Option<String>| NoteRead {
            note_ref: note_ref.to_owned(),
            over_budget: body.is_none(),
            body,
            line_count: meta.line_count,
            size_bytes: size,
        };
        if full {
            if size > max_bytes as u64 {
                return Ok(result(None));
            }
            let mut body = String::new();
            reader.read_to_string(&mut body)?;
            return Ok(result(Some(body)));
        }
        let first = start.unwrap_or(1);
        let last = end.unwrap_or(meta.line_count);
        if first > meta.line_count || last > meta.line_count {
            return Err(invalid("Note range is outside the body"));
        }
        let mut body = Vec::new();
        let mut line = 1;
        // Scan bounded buffers so a single huge line cannot exceed the budget
        // merely by being traversed or selected.
        while line <= last {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                break;
            }
            let newline = buf.iter().position(|b| *b == b'\n');
            let n = newline.map_or(buf.len(), |i| i + 1);
            if line >= first {
                if n > max_bytes.saturating_sub(body.len()) {
                    return Ok(result(None));
                }
                body.extend_from_slice(&buf[..n]);
            }
            reader.consume(n);
            if newline.is_some() {
                line += 1;
            }
        }
        Ok(result(Some(
            String::from_utf8(body).map_err(|_| invalid("Note body is not UTF-8"))?,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_origin_survives_worker_change_without_rewriting_log() {
        let temp = tempfile::tempdir().unwrap();
        let store = NoteStore::new(temp.path());
        let log = (1..=300)
            .map(|i| {
                if i == 280 {
                    "E421\r\n".into()
                } else {
                    format!("line{i}\r\n")
                }
            })
            .collect::<String>();
        let original = store
            .externalize("s1", "call:42", 0, "发布验收", &log)
            .unwrap();
        let new_worker = NoteStore::new(temp.path());
        let replay = new_worker
            .externalize("s1", "call:42", 0, "发布验收", &log)
            .unwrap();
        assert_eq!(original.note_ref, replay.note_ref);
        assert_eq!(original.created_at, replay.created_at);
        let id = &original.note_ref;
        assert!(
            new_worker
                .externalize("s1", "call:42", 0, "发布验收", "different")
                .is_err()
        );
        assert_eq!(
            new_worker
                .read("s1", id, None, None, usize::MAX)
                .unwrap()
                .body
                .as_deref(),
            Some(log.as_str())
        );
        assert_eq!(
            new_worker
                .read("s1", id, Some(280), Some(280), 64)
                .unwrap()
                .body
                .as_deref(),
            Some("E421\r\n")
        );
        assert_eq!(
            new_worker
                .read("s1", id, Some(299), None, 64)
                .unwrap()
                .body
                .as_deref(),
            Some("line299\r\nline300\r\n")
        );
        assert_eq!(
            new_worker
                .read("s1", id, None, Some(2), 64)
                .unwrap()
                .body
                .as_deref(),
            Some("line1\r\nline2\r\n")
        );
        let too_large = new_worker.read("s1", id, None, None, 4).unwrap();
        assert!(too_large.over_budget && too_large.body.is_none());
        assert_eq!(too_large.line_count, 300);
        assert_eq!(too_large.size_bytes, log.len() as u64);
        assert_eq!(
            new_worker
                .search("s1", Some("E421"), None, 10)
                .unwrap()
                .items
                .len(),
            0
        );
        assert_eq!(
            new_worker
                .search("s1", Some("发布验收"), None, 10)
                .unwrap()
                .items
                .len(),
            1
        );
        new_worker.write("s2", "other", "other").unwrap();
        assert!(new_worker.read("s2", id, None, None, 1024).is_err());
        assert!(
            new_worker
                .read("s1", "note:s1:../s2/foo", None, None, 1024)
                .is_err()
        );
    }

    #[test]
    fn pagination_and_utf8_budget_are_literal_and_complete() {
        let temp = tempfile::tempdir().unwrap();
        let store = NoteStore::new(temp.path());
        let a = store.write("s", "One.[α]", "词").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = store.write("s", "One.[β]", "second").unwrap();
        let first = store.search("s", Some("One.["), None, 1).unwrap();
        assert_eq!(first.items[0].note_ref, b.note_ref);
        let last = store
            .search("s", Some("One.["), first.next_cursor.as_deref(), 1)
            .unwrap();
        assert_eq!(last.items[0].note_ref, a.note_ref);
        assert!(last.next_cursor.is_none());
        assert!(
            store
                .search("s", Some("one.["), None, 10)
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            store
                .read("s", &a.note_ref, Some(1), Some(1), 2)
                .unwrap()
                .over_budget
        );
    }
}
