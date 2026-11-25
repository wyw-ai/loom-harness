use crate::context::ModelRoute;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{collections::BTreeMap, env, path::PathBuf};

#[derive(Clone, Deserialize)]
pub struct Identity {
    pub owner: String,
    pub subject: Option<String>,
    pub source_id: Option<String>,
    #[serde(default)]
    pub admin: bool,
    #[serde(default)]
    pub source: bool,
    #[serde(default)]
    pub approve: bool,
}

#[derive(Clone, Deserialize)]
pub struct Provider {
    pub base_url: String,
    pub model: String,
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub images: bool,
}

impl Provider {
    pub fn route(&self) -> ModelRoute {
        ModelRoute {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key_env: self.api_key_env.clone(),
        }
    }
}

pub struct Config {
    pub home: PathBuf,
    pub bind: String,
    pub event_bus: EventBusConfig,
    pub role: String,
    pub workers: usize,
    pub lease_seconds: i64,
    pub tokens: BTreeMap<String, Identity>,
    pub models: BTreeMap<String, Provider>,
    pub default_model: Option<String>,
    pub max_rounds: usize,
    pub note_threshold: usize,
    pub note_budget: usize,
    pub notification_targets: Vec<String>,
}

pub enum EventBusConfig {
    Local,
    Redis { url: String, stream: Option<String> },
}

impl EventBusConfig {
    fn select(backend: Option<&str>, url: Option<&str>, stream: Option<&str>) -> Result<Self> {
        let url = url.map(str::trim).filter(|value| !value.is_empty());
        let stream = stream.filter(|value| !value.trim().is_empty());
        match backend.map(str::trim).filter(|value| !value.is_empty()) {
            Some("local") => Ok(Self::Local),
            None if url.is_none() && stream.is_none() => Ok(Self::Local),
            Some("redis") | None => Ok(Self::Redis {
                url: url.unwrap_or("redis://127.0.0.1:6379/").to_owned(),
                stream: stream.map(str::to_owned),
            }),
            Some(_) => bail!("LOOM_EVENT_BUS must be local or redis"),
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Redis { .. } => "redis",
        }
    }
}

fn value(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}
fn number(key: &str, default: usize) -> Result<usize> {
    let n = value(key, &default.to_string())
        .parse::<usize>()
        .with_context(|| format!("invalid {key}"))?;
    if n == 0 {
        bail!("{key} must be positive")
    }
    Ok(n)
}

impl Config {
    pub fn load() -> Result<Self> {
        let mut home = PathBuf::from(value("LOOM_HOME", ".loom"));
        let mut role = value("LOOM_ROLE", "all");
        let mut bind = value("LOOM_BIND", "127.0.0.1:7331");
        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            let val = args.next().context("expected an option value")?;
            match arg.as_str() {
                "--home" => home = val.into(),
                "--role" => role = val,
                "--bind" => bind = val,
                _ => bail!("unknown option {arg}"),
            }
        }
        if !matches!(role.as_str(), "all" | "api" | "worker") {
            bail!("role must be all, api or worker")
        }
        let tokens: BTreeMap<String, Identity> =
            serde_json::from_str(&value("LOOM_AUTH_JSON", "{}"))
                .context("invalid LOOM_AUTH_JSON")?;
        if role != "worker" && tokens.is_empty() {
            bail!("configure at least one API bearer in LOOM_AUTH_JSON")
        }
        if tokens
            .iter()
            .any(|(key, v)| key.is_empty() || v.owner.is_empty())
        {
            bail!("empty API bearer or owner")
        }
        let mut models: BTreeMap<String, Provider> =
            serde_json::from_str(&value("LOOM_MODEL_ROUTES", "{}"))
                .context("invalid LOOM_MODEL_ROUTES")?;
        if models.is_empty() && env::var_os("DEEPSEEK_API_KEY").is_some() {
            models.insert(
                "deepseek-flash".into(),
                Provider {
                    base_url: "https://api.deepseek.com".into(),
                    model: "deepseek-flash".into(),
                    api_key_env: Some("DEEPSEEK_API_KEY".into()),
                    images: false,
                },
            );
        }
        let default_model = env::var("LOOM_DEFAULT_MODEL").ok().or_else(|| {
            if models.len() == 1 {
                models.keys().next().cloned()
            } else {
                None
            }
        });
        if default_model
            .as_ref()
            .is_some_and(|m| !models.contains_key(m))
        {
            bail!("configured default model is not available")
        }
        Ok(Self {
            home,
            bind,
            role,
            tokens,
            models,
            default_model,
            event_bus: EventBusConfig::select(
                env::var("LOOM_EVENT_BUS").ok().as_deref(),
                env::var("LOOM_REDIS_URL").ok().as_deref(),
                env::var("LOOM_REDIS_STREAM").ok().as_deref(),
            )?,
            workers: number("LOOM_WORKERS", 1)?,
            lease_seconds: i64::try_from(number("LOOM_WORK_LEASE_SECONDS", 45)?)?.clamp(2, 3600),
            max_rounds: number("LOOM_MAX_ROUNDS", 24)?,
            note_threshold: number("LOOM_NOTE_THRESHOLD", 8192)?,
            note_budget: number("LOOM_NOTE_READ_BUDGET", 65536)?,
            notification_targets: serde_json::from_str(&value("LOOM_NOTIFICATION_TARGETS", "[]"))?,
        })
    }
}

#[cfg(test)]
mod bus_selection_tests {
    use super::EventBusConfig;

    #[test]
    fn local_is_default_and_explicit_selection_wins_over_legacy_settings() {
        for (backend, url, stream, expected) in [
            (None, None, None, "local"),
            (None, Some(""), Some(" "), "local"),
            (None, Some("redis://host/"), None, "redis"),
            (None, None, Some("legacy-stream"), "redis"),
            (Some("local"), Some("unreachable"), Some("old"), "local"),
            (Some("redis"), None, None, "redis"),
        ] {
            assert_eq!(
                EventBusConfig::select(backend, url, stream).unwrap().name(),
                expected
            );
        }
        assert!(EventBusConfig::select(Some("typo"), None, None).is_err());
    }
}
