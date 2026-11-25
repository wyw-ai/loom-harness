mod api;
mod attachments;
mod bus;
mod catalog;
mod config;
mod context;
mod dispatch;
mod engine;
mod store;

use anyhow::Result;
use config::{Config, EventBusConfig};
use std::{sync::Arc, time::Duration};

#[derive(Clone)]
pub struct App {
    cfg: Arc<Config>,
    db: store::Store,
    catalog: catalog::Catalog,
    http: reqwest::Client,
    notes: Arc<context::NoteStore>,
    ready: Arc<tokio::sync::Notify>,
    suggestion_slots: Arc<tokio::sync::Semaphore>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Arc::new(Config::load()?);
    let state = cfg.home.join("state");
    std::fs::create_dir_all(&state)?;
    let app = App {
        db: store::Store::open(state.join("loom.db"))?,
        catalog: catalog::Catalog::open(&cfg.home).map_err(anyhow::Error::msg)?,
        notes: Arc::new(context::NoteStore::new(cfg.home.clone())),
        cfg: cfg.clone(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(90))
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        ready: Default::default(),
        suggestion_slots: Arc::new(tokio::sync::Semaphore::new(4)),
    };
    let instance = uuid::Uuid::new_v4().simple().to_string();
    let bus: Arc<dyn bus::EventBus> = match &cfg.event_bus {
        EventBusConfig::Local => Arc::new(bus::LocalBus::new(app.db.clone())),
        EventBusConfig::Redis { url, stream } => {
            let stream = match stream {
                Some(name) => name.clone(),
                None => format!("loom:{}:events", app.db.instance_id()?),
            };
            Arc::new(bus::RedisBus::new(url, &stream)?)
        }
    };
    println!("loom-harness event bus: {}", cfg.event_bus.name());
    let _bus = bus::spawn_bus(
        app.db.clone(),
        bus.clone(),
        instance.clone(),
        vec![bus::Subscription::runtime()],
    );
    let _plugins =
        bus::spawn_plugin_bus(app.db.clone(), bus, instance.clone(), app.catalog.clone());
    if cfg.role != "api" {
        tokio::spawn(dispatch::pump(app.clone()));
        for i in 0..cfg.workers {
            tokio::spawn(engine::worker(app.clone(), format!("{instance}-{i}")));
        }
    }
    if cfg.role != "worker" {
        tokio::spawn(app.catalog.clone().sync_oi_instances());
        let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
        println!("loom-harness listening on {}", listener.local_addr()?);
        axum::serve(listener, api::router(app))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await?;
    } else {
        tokio::signal::ctrl_c().await?;
    }
    Ok(())
}
