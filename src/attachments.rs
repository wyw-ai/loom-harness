use crate::context;
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

fn location(home: &Path, owner: &str, id: &str) -> Result<PathBuf> {
    let uuid = Uuid::parse_str(id).context("invalid attachment id")?;
    Ok(home
        .join("artifacts")
        .join("images")
        .join(format!("{:x}", Sha256::digest(owner)))
        .join(uuid.to_string()))
}
pub fn put(home: &Path, owner: &str, body: &Value) -> Result<Value> {
    let mime = body["mime_type"].as_str().context("missing mime_type")?;
    let bytes = STANDARD.decode(body["data"].as_str().context("missing base64 data")?)?;
    context::image_part(mime, &bytes)?;
    let id = Uuid::new_v4().to_string();
    let path = location(home, owner, &id)?;
    fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("pending");
    let mut data = mime.as_bytes().to_vec();
    data.push(b'\n');
    data.extend_from_slice(&bytes);
    fs::write(&tmp, data)?;
    fs::rename(tmp, path)?;
    Ok(json!({"id":id,"mime_type":mime,"size_bytes":bytes.len()}))
}
pub fn read(home: &Path, owner: &str, id: &str) -> Result<Value> {
    let data = fs::read(location(home, owner, id)?).context("attachment not accessible")?;
    let newline = data
        .iter()
        .position(|b| *b == b'\n')
        .context("invalid attachment header")?;
    Ok(context::image_part(
        std::str::from_utf8(&data[..newline])?,
        &data[newline + 1..],
    )?)
}
pub fn remove(home: &Path, owner: &str, id: &str) -> Result<()> {
    fs::remove_file(location(home, owner, id)?).context("attachment not accessible")
}
pub fn resolve(home: &Path, owner: &str, messages: &mut [Value]) -> Result<()> {
    for message in messages {
        if let Some(parts) = message["content"].as_array_mut() {
            for part in parts {
                if part["type"] == "attachment" {
                    *part = read(
                        home,
                        owner,
                        part["attachment_id"]
                            .as_str()
                            .context("missing attachment_id")?,
                    )?;
                } else if part["type"] == "image_url" {
                    bail!("upload the image and reference its attachment_id")
                }
            }
        }
    }
    Ok(())
}
