use super::*;
use crate::catalog::Catalog;
use mlua::{Function, Lua, LuaSerdeExt, Table};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use tokio::task::AbortHandle;

const HANDLERS: &str = "loom.event.subscriptions";

pub(crate) fn register_lua_subscription(lua: &Lua, api: &Table) -> mlua::Result<()> {
    let handlers = lua.create_table()?;
    lua.set_named_registry_value(HANDLERS, handlers.clone())?;
    let subscribe =
        lua.create_function(move |_, (_, spec, callback): (Table, Table, Function)| {
            let id: String = spec.get("id")?;
            if id.trim().is_empty() || handlers.contains_key(&id[..])? {
                return Err(mlua::Error::runtime("empty or duplicate subscription id"));
            }
            let types: Vec<String> = spec.get("types")?;
            if types.is_empty() || types.iter().any(|ty| ty.is_empty()) {
                return Err(mlua::Error::runtime(
                    "subscription requires named event types",
                ));
            }
            let _: Option<String> = spec.get("source")?;
            spec.set("handler", callback)?;
            handlers.set(id, spec)
        })?;
    api.set("subscribe", subscribe)?;
    lua.globals().set("plugin", api.clone())
}

fn load_handlers(code: &str) -> Result<(Lua, Table)> {
    let (lua, _, _) = crate::catalog::lua::package(code).map_err(anyhow::Error::msg)?;
    let handlers = lua.named_registry_value(HANDLERS).map_err(lua_error)?;
    Ok((lua, handlers))
}

fn lua_error(err: mlua::Error) -> anyhow::Error {
    anyhow::Error::msg(err.to_string())
}

fn identity(instance: &str, subscription: &str) -> String {
    json!([instance, subscription]).to_string()
}

pub(crate) fn subscription_group(owner: &str, instance: &str, subscription: &str) -> String {
    Subscription::scoped(&identity(instance, subscription), owner, vec![]).group
}

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .with_context(|| format!("missing subscription instance {key}"))
}

fn registrations(catalog: &Catalog, plugin: &Value) -> Result<Vec<Subscription>> {
    let owner = field(plugin, "owner")?;
    let instance = field(plugin, "id")?;
    let (_lua, handlers) = load_handlers(field(plugin, "code")?)?;
    let mut out = Vec::new();
    for pair in handlers.pairs::<String, Table>() {
        let (id, definition) = pair.map_err(lua_error)?;
        let types: Vec<String> = definition.get("types").map_err(lua_error)?;
        let sub = Subscription::scoped(&identity(instance, &id), owner, types);
        let (catalog, instance, owner) = (catalog.clone(), instance.to_owned(), owner.to_owned());
        out.push(sub.with_commit(move |store, group, event| {
            let eid = event_key(event)?;
            if store.subscription_result(group, eid)?.is_some() {
                return Ok(());
            }
            // An event may sit in a consumer group's pending list while an instance
            // is disabled, reconfigured, or moved out of this authorization scope.
            let current = catalog
                .subscription_plugins()
                .map_err(anyhow::Error::msg)?
                .into_iter()
                .find(|p| p["id"] == instance && p["owner"] == owner);
            let Some(current) = current else {
                return Ok(());
            };
            let (lua, handlers) = load_handlers(field(&current, "code")?)?;
            let Some(def): Option<Table> = handlers.get(id.as_str()).map_err(lua_error)? else {
                return Ok(());
            };
            let source: Option<String> = def.get("source").map_err(lua_error)?;
            let types: Vec<String> = def.get("types").map_err(lua_error)?;
            if source.as_deref().is_some_and(|s| event["source"] != s)
                || !types.iter().any(|ty| event["type"].as_str() == Some(ty))
            {
                return Ok(());
            }
            // A handler computes a value in the same restricted VM used for Lua tools.
            // Re-execution after a crash is safe: its only externally visible result
            // is the following idempotent database commit.
            let context = json!({"plugin_instance": instance, "owner": owner,
                "subscription_id":id, "settings":current["settings"]});
            let callback: Function = def.get("handler").map_err(lua_error)?;
            let result: mlua::Value = callback
                .call((
                    lua.to_value(&context).map_err(lua_error)?,
                    lua.to_value(event).map_err(lua_error)?,
                ))
                .map_err(lua_error)?;
            let value: Value = lua.from_value(result).map_err(lua_error)?;
            let record = json!({"event_id":eid,"plugin_instance":instance,"subscription_id":id,
                "version":current["version"],"result":value});
            store.consume_event_result(group, eid, &record)?;
            Ok(())
        }));
    }
    Ok(out)
}

/// Reconcile plugin instances installed through the normal catalog. New revisions
/// reuse subscription group identities; the commit path checks live authorization.
pub(crate) fn spawn_plugin_bus(
    store: Store,
    bus: Arc<dyn EventBus>,
    worker: String,
    catalog: Catalog,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut tasks: JoinSet<()> = JoinSet::new();
        let mut active: HashMap<String, (Value, Vec<AbortHandle>)> = HashMap::new();
        loop {
            while tasks.try_join_next().is_some() {}
            match catalog.subscription_plugins() {
                Err(err) => eprintln!("cannot list event subscriptions: {err}"),
                Ok(plugins) => {
                    let mut enabled = HashSet::new();
                    for plugin in plugins {
                        let Some(id) = plugin["id"].as_str() else {
                            continue;
                        };
                        enabled.insert(id.to_owned());
                        if active
                            .get(id)
                            .is_some_and(|(rev, _)| rev == &plugin["revision"])
                        {
                            continue;
                        }
                        let subscriptions = match registrations(&catalog, &plugin) {
                            Ok(subscriptions) => subscriptions,
                            Err(err) => {
                                eprintln!("invalid event subscriptions for {id}: {err}");
                                continue;
                            }
                        };
                        if let Some((_, old)) = active.remove(id) {
                            for task in old {
                                task.abort();
                            }
                        }
                        let handles = subscriptions
                            .into_iter()
                            .map(|sub| {
                                let (bus, store, worker) =
                                    (bus.clone(), store.clone(), worker.clone());
                                tasks.spawn(async move {
                                    let mut cursor = String::from("0-0");
                                    let mut failures = 0;
                                    loop {
                                        let result = consume_batch(
                                            &store,
                                            &*bus,
                                            &sub,
                                            &worker,
                                            &mut cursor,
                                        )
                                        .await;
                                        pace(sub.group(), result, &mut failures).await;
                                    }
                                })
                            })
                            .collect();
                        active.insert(id.to_owned(), (plugin["revision"].clone(), handles));
                    }
                    active.retain(|id, (_, handles)| {
                        if enabled.contains(id) {
                            return true;
                        }
                        for h in handles {
                            h.abort();
                        }
                        false
                    });
                }
            }
            sleep(Duration::from_millis(400)).await;
        }
    })
}
