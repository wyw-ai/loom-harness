use super::{R, store::err};
use mlua::{Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, VmState};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};

fn vm() -> R<Lua> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )
    .map_err(err)?;
    // Base library loaders must also be removed; an authored tool receives no filesystem capability.
    for n in [
        "dofile",
        "loadfile",
        "load",
        "require",
        "print",
        "collectgarbage",
        "pcall",
        "xpcall",
    ] {
        lua.globals().raw_set(n, mlua::Value::Nil).map_err(err)?;
    }
    lua.set_memory_limit(16 * 1024 * 1024).map_err(err)?;
    let fuel = AtomicU32::new(0);
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, _| {
            if fuel.fetch_add(1, Ordering::Relaxed) >= 2000 {
                return Err(mlua::Error::runtime("Lua instruction budget exceeded"));
            }
            Ok(VmState::Continue)
        },
    )
    .map_err(err)?;
    Ok(lua)
}
pub(super) fn validate(code: &str) -> R<()> {
    let lua = vm()?;
    lua.load(code).eval::<Function>().map(|_| ()).map_err(err)
}
pub fn evaluate_dispatch_rule(source: &str, ctx: &Value) -> R<Value> {
    let lua = vm()?;
    let f = match lua.load(source).eval::<mlua::Value>().map_err(err)? {
        mlua::Value::Function(f) => f,
        _ => lua.globals().get::<Function>("decide").map_err(err)?,
    };
    let v = f
        .call::<mlua::Value>(lua.to_value(ctx).map_err(err)?)
        .map_err(err)?;
    lua.from_value(v).map_err(err)
}
pub(super) fn run(code: &str, ctx: &Value, args: &Value) -> R<Value> {
    let lua = vm()?;
    let f = lua.load(code).eval::<Function>().map_err(err)?;
    invoke(&lua, f, ctx, args)
}
fn invoke(lua: &Lua, f: Function, ctx: &Value, args: &Value) -> R<Value> {
    let context = lua.create_table().map_err(err)?;
    for (k, v) in ctx.as_object().into_iter().flatten() {
        if k != "note" {
            context
                .set(k.as_str(), lua.to_value(v).map_err(err)?)
                .map_err(err)?
        }
    }
    context.set("note", mlua::Value::Nil).map_err(err)?;
    if let Some(note) = ctx.get("note") {
        let r = note["note_ref"].as_str().unwrap_or("").to_owned();
        let b = note["body"].as_str().map(str::to_owned);
        context
            .set(
                "read_note",
                lua.create_function(move |_, s: String| {
                    if s != r {
                        return Err(mlua::Error::runtime("forbidden: note outside scope"));
                    }
                    b.clone().ok_or_else(|| {
                        mlua::Error::runtime("note exceeds input budget; read a smaller line range")
                    })
                })
                .map_err(err)?,
            )
            .map_err(err)?;
    }
    let out: mlua::Value = f
        .call((context, lua.to_value(args).map_err(err)?))
        .map_err(err)?;
    lua.from_value(out).map_err(err)
}
// Capture declarations from the package entry point. The caller decides instance identity and grants.
pub(crate) fn package(code: &str) -> R<(Lua, Table, Table)> {
    let lua = vm()?;
    let tools = lua.create_table().map_err(err)?;
    let sets = lua.create_table().map_err(err)?;
    let api = lua.create_table().map_err(err)?;
    crate::bus::register_lua_subscription(&lua, &api).map_err(err)?;
    for (name, registry) in [
        ("register_context", "loom.context"),
        ("register_trigger", "loom.trigger"),
        ("register_goal_strategy", "loom.goal_strategy"),
        ("register_suggestion_prompt", "loom.suggestion_prompt"),
        ("register_compaction_prompt", "loom.compaction_prompt"),
    ] {
        let register = lua
            .create_function(move |vm, f: Function| {
                if !vm.named_registry_value::<mlua::Value>(registry)?.is_nil() {
                    return Err(mlua::Error::runtime("duplicate hook"));
                }
                vm.set_named_registry_value(registry, f)
            })
            .map_err(err)?;
        api.set(name, register).map_err(err)?;
    }
    let routes = lua.create_table().map_err(err)?;
    let sections = lua.create_table().map_err(err)?;
    for (name, dst) in [
        ("register_route", routes.clone()),
        ("register_section", sections.clone()),
    ] {
        let f = lua
            .create_function(move |_, t: Table| dst.raw_push(t))
            .map_err(err)?;
        api.set(name, f.clone()).map_err(err)?;
        lua.globals().set(name, f).map_err(err)?;
    }
    lua.set_named_registry_value("loom.routes", routes)
        .map_err(err)?;
    lua.set_named_registry_value("loom.sections", sections)
        .map_err(err)?;
    for (name, dst) in [
        ("register_tool", tools.clone()),
        ("register_toolset", sets.clone()),
    ] {
        let f = lua
            .create_function(move |_, (t, h): (Table, Option<Function>)| {
                let id: String = t.get("id").or_else(|_| t.get("name"))?;
                if dst.contains_key(id.as_str())? {
                    return Err(mlua::Error::runtime("duplicate registration"));
                }
                if let Some(f) = h {
                    t.set("handler", f)?
                }
                dst.set(id, t)
            })
            .map_err(err)?;
        api.set(name, f.clone()).map_err(err)?;
        lua.globals().set(name, f).map_err(err)?;
    }
    lua.globals().set("loom", api.clone()).map_err(err)?;
    let ret: mlua::Value = lua.load(code).eval().map_err(err)?;
    if let mlua::Value::Function(init) = ret {
        init.call::<()>(api).map_err(err)?;
    }
    Ok((lua, tools, sets))
}
pub(super) fn extensions(code: &str) -> R<(Vec<Value>, Vec<Value>)> {
    let (lua, _, _) = package(code)?;
    let routes: Table = lua.named_registry_value("loom.routes").map_err(err)?;
    let sections: Table = lua.named_registry_value("loom.sections").map_err(err)?;
    let mut rr = vec![];
    for t in routes.sequence_values::<Table>() {
        let t = t.map_err(err)?;
        let method: String = t.get("method").map_err(err)?;
        let path: String = t.get("path").map_err(err)?;
        let _: Function = t.get("handler").map_err(err)?;
        rr.push(json!({"method":method,"path":path}));
    }
    let mut ss = vec![];
    for t in sections.sequence_values::<Table>() {
        let t = t.map_err(err)?;
        let id: String = t.get("id").map_err(err)?;
        let text: String = t.get("text").map_err(err)?;
        ss.push(json!({"id":id,"text":text}));
    }
    Ok((rr, ss))
}
pub(super) fn call_hook(code: &str, kind: &str, ctx: &Value, input: &Value) -> R<Value> {
    let (lua, _, _) = package(code)?;
    let hook: Function = lua
        .named_registry_value(&format!("loom.{kind}"))
        .map_err(err)?;
    invoke(&lua, hook, ctx, input)
}
pub(super) fn run_route(
    code: &str,
    method: &str,
    path: &str,
    ctx: &Value,
    args: &Value,
) -> R<Value> {
    let (lua, _, _) = package(code)?;
    let routes: Table = lua.named_registry_value("loom.routes").map_err(err)?;
    for t in routes.sequence_values::<Table>() {
        let t = t.map_err(err)?;
        if t.get::<String>("method").map_err(err)? == method
            && t.get::<String>("path").map_err(err)? == path
        {
            return invoke(&lua, t.get("handler").map_err(err)?, ctx, args);
        }
    }
    Err("not found: plugin route".into())
}
pub(super) fn declarations(code: &str) -> R<(Vec<Value>, Vec<Value>)> {
    let (lua, tools, sets) = package(code)?;
    let mut out = vec![];
    let mut sets_out = vec![];
    for entry in tools.pairs::<String, Table>() {
        let (id, t) = entry.map_err(err)?;
        let _: Function = t.get("handler").map_err(err)?;
        let mut d = json!({"id":id});
        for key in [
            "description",
            "input_schema",
            "output_schema",
            "exposure",
            "requires_approval",
            "webui",
        ] {
            let v: mlua::Value = t.get(key).map_err(err)?;
            if !v.is_nil() {
                d[key] = lua.from_value(v).map_err(err)?
            }
        }
        super::resources::normalize_tool_webui(&mut d)?;
        out.push(d);
    }
    for entry in sets.pairs::<String, Table>() {
        let (id, t) = entry.map_err(err)?;
        let members: Vec<String> = t.get("members").map_err(err)?;
        sets_out.push(json!({"id":id,"members":members}));
    }
    out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    sets_out.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Ok((out, sets_out))
}
pub(super) fn run_package(code: &str, name: &str, ctx: &Value, args: &Value) -> R<Value> {
    let (lua, tools, _) = package(code)?;
    let t: Table = tools.get(name).map_err(err)?;
    invoke(&lua, t.get("handler").map_err(err)?, ctx, args)
}
