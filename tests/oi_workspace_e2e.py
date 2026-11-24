"""Exercise two real OI services and Harness through their public interfaces.

Supply built OIS/OIC executables and the four OI App manifests. The local model
selects one published tool per turn. Screenshot data stays inside the test.
"""

import argparse
import base64
import hashlib
import json
import os
import secrets
import subprocess
import tempfile
import threading
import time
from contextlib import ExitStack
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen

from harness import Instance, free_port


class Model(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        tool, args = self.server.choice
        last = body["messages"][-1]
        if isinstance(last["content"], list):
            self.server.images = [
                base64.b64decode(
                    part["image_url"]["url"].split(",", 1)[1], validate=True
                )
                for part in last["content"]
                if part["type"] == "image_url"
            ]
        if last["role"] == "tool" or isinstance(last["content"], list):
            message = {"role": "assistant", "content": "Completed"}
        else:
            self.server.offered = [row["function"]["name"] for row in body["tools"]]
            message = {
                "role": "assistant",
                "content": None,
                "tool_calls": [
                    {
                        "id": "invoke",
                        "type": "function",
                        "function": {"name": tool, "arguments": json.dumps(args)},
                    }
                ],
            }
        value = json.dumps(
            {
                "choices": [
                    {
                        "message": message,
                        "finish_reason": "stop"
                        if "tool_calls" not in message
                        else "tool_calls",
                    }
                ]
            }
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(value)))
        self.end_headers()
        self.wfile.write(value)


def eventually(check, timeout=35):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            value = check()
            if value:
                return value
        except (OSError, URLError, subprocess.TimeoutExpired):
            pass
        time.sleep(0.2)
    raise TimeoutError(f"Timed out waiting for {check.__name__}")


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(data), encoding="utf-8")


def launch(stack, exe, conf, env, folder):
    log = stack.enter_context((folder / (exe.stem + ".log")).open("ab"))
    proc = subprocess.Popen(
        [str(exe), "--config", str(conf), "start", "--foreground"],
        stdout=log,
        stderr=log,
        env=env,
        cwd=folder,
    )

    def stop():
        if proc.poll() is None:
            proc.terminate()
            proc.wait(timeout=15)

    stack.callback(stop)


def run(cli, folder, stack):
    origin = f"http://127.0.0.1:{free_port()}"
    env = dict(os.environ)
    env.pop("DEEPSEEK_API_KEY", None)
    for key in (
        "CHANNEL_TOKEN",
        "ADMIN_TOKEN",
        "DIRECTORY_TOKEN",
        "CREATOR_TOKEN",
        "WORKBENCH_TOKEN",
    ):
        env[key] = secrets.token_hex(24)
    admin_file = folder / "oi-admin.secret"
    admin_file.write_text(env["ADMIN_TOKEN"], encoding="ascii")

    def member(app, action, name=None):
        return {
            "name": name or action,
            "source": {"channel": "host-a", "app": app, "action": action},
        }

    manage = [
        member("loom-oi", action)
        for action in (
            "list_hosts",
            "list_apps",
            "list_actions",
            "list_services",
            "get_service",
            "apply_service",
            "delete_service",
        )
    ]
    base_actions = [
        member("computer-use", "screenshot", "capture_a"),
        member("filesystem", "view_image", "view_on_a"),
    ]
    skill_actions = [member("skills", "list_skills"), member("skills", "read_skill")]

    def service(actions, cred):
        return {
            "access": {"credential_env": cred},
            "actions": actions,
            "publish": {"mcp": True},
        }

    services = {
        "oi-catalog": service([member("loom-oi", "list_services")], "DIRECTORY_TOKEN"),
        "oi-admin": service(manage, "CREATOR_TOKEN"),
        "workbench": service(base_actions, "WORKBENCH_TOKEN"),
    }
    write(
        folder / "ois.yaml",
        {
            "version": 1,
            "state_dir": "./ois",
            "admin": {"token_env": "ADMIN_TOKEN"},
            "server": {
                "listen": origin.split("//")[1],
                "public_url": origin,
                "data_file": "./ois/config.json",
            },
            "channels": {
                "host-a": {
                    "expected_host_id": "host-a",
                    "credential_env": "CHANNEL_TOKEN",
                }
            },
            "services": services,
        },
    )

    def principal(svc, token):
        return {"service": svc, "identity": f"consumer:{svc}:{token}"}

    catalog = {
        **principal("oi-catalog", "DIRECTORY_TOKEN"),
        "visible_services": list(services),
    }
    creator = {
        **principal("oi-admin", "CREATOR_TOKEN"),
        "hosts": ["host-a"],
        "sources": [m["source"] for m in base_actions + skill_actions],
        "visible_services": list(services),
        "managed_services": ["workbench"],
    }
    project, home, exchange = (
        folder / name for name in ("project", "home", "exchange")
    )
    exchange.mkdir()
    for root in (project, home):
        for product in ("agents", "codex"):
            pkg = root / ("." + product) / "skills" / "sample"
            pkg.mkdir(parents=True)
            (pkg / "SKILL.md").write_text(
                f"---\nname: sample\ndescription: {root.name}-{product}\n---\nRead the companion.\n",
                encoding="utf-8",
            )
            (pkg / "details.txt").write_text(
                f"E2E-{root.name}-{product}", encoding="utf-8"
            )
    write(
        folder / "oic.yaml",
        {
            "version": 1,
            "state_dir": "./oic",
            "host": {"id": "host-a", "name": "Host A"},
            "channel": {"id": "host-a"},
            "ois": {"url": origin, "credential_env": "CHANNEL_TOKEN"},
            "app_config": {
                "loom-oi": {
                    "base_url": origin,
                    "admin_token_file": str(admin_file),
                    "callers": [catalog, creator],
                },
                "skills": {
                    "workspace": str(project),
                    "home": str(home),
                    "allowed_callers": [principal("workbench", "WORKBENCH_TOKEN")],
                },
                "filesystem": {"root": str(exchange), "read_only": True},
                "computer-use": {"output_root": str(exchange)},
            },
        },
    )

    def oi_get(path):
        req = Request(
            origin + "/api/v1/" + path,
            headers={"Authorization": "Bearer " + env["ADMIN_TOKEN"]},
        )
        with urlopen(req, timeout=3) as response:
            return json.load(response)

    launch(stack, cli.ois, folder / "ois.yaml", env, folder)
    eventually(lambda: oi_get("services"))
    launch(stack, cli.oic, folder / "oic.yaml", env, folder)
    cmd = [str(cli.oic), "--config", str(folder / "oic.yaml")]
    eventually(
        lambda: (
            subprocess.run(
                cmd + ["status"], env=env, capture_output=True, timeout=4, check=False
            ).returncode
            == 0
        )
    )
    for manifest in (cli.management, cli.skills, cli.capture, cli.filesystem):
        result = subprocess.run(
            cmd + ["load", str(manifest)],
            env=env,
            capture_output=True,
            timeout=35,
            check=False,
        )
        if result.returncode:
            raise RuntimeError(result.stderr.decode(errors="replace"))
    eventually(lambda: "list_skills" in json.dumps(oi_get("actions")))

    model = ThreadingHTTPServer(("127.0.0.1", 0), Model)
    stack.callback(model.server_close)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    stack.callback(model.shutdown)
    routes = {
        "test": {
            "model": "fixture",
            "base_url": f"http://127.0.0.1:{model.server_port}",
            "images": True,
        }
    }
    if cli.live_text:
        routes["live"] = {
            "model": "deepseek-flash",
            "base_url": "https://api.deepseek.com",
            "api_key_env": "DEEPSEEK_API_KEY",
            "images": False,
        }
    henv = {"LOOM_MODEL_ROUTES": json.dumps(routes), "LOOM_DEFAULT_MODEL": "test"}
    for ref, token in [
        ("DIR", "DIRECTORY_TOKEN"),
        ("MGR", "CREATOR_TOKEN"),
        ("WORK", "WORKBENCH_TOKEN"),
    ]:
        henv["LOOM_MCP_CREDENTIAL_" + ref] = env[token]
    app = stack.enter_context(Instance(env=henv))

    def browser(phase, **trace):
        subprocess.run(
            ["node", str(Path(__file__).with_name("oi_workspace_browser.cjs"))],
            env={
                **os.environ,
                "LOOM_BROWSER_PHASE": phase,
                "LOOM_BROWSER_BASE": app.base,
                **trace,
            },
            check=True,
            timeout=150,
        )

    def admin(method, path, data=None):
        return app.api(method, path, data, token="admin-token")

    admin(
        "POST",
        "plugins",
        {
            "id": "office",
            "package": "internal-tools",
            "version": "1.0.0",
            "enabled": True,
        },
    )
    admin(
        "PATCH",
        "settings/office",
        {
            "base_url": origin,
            "catalog_service": "oi-catalog",
            "catalog_credential_ref": "dir",
            "consumer_credential_ref": "work",
            "refresh_interval_secs": 6,
            "service_credentials": {"oi-admin": "mgr", "oi-catalog": "dir"},
        },
    )
    admin("POST", "plugins/office/refresh", {})

    def agent(name, svc):
        group = "office." + svc
        admin(
            "POST",
            "agents",
            {
                "id": name,
                "tools": []
                if cli.browser and name == "reader"
                else [{"toolset": group}],
                "grants": {"toolsets": [group]},
                "model": "test",
            },
        )
        return app.api("POST", "sessions", {"agent_id": name})["id"]

    author, reader = agent("author", "oi-admin"), agent("reader", "workbench")
    trace = {}

    def invoke(sid, name, args):
        model.choice = (name, args)
        rid = app.send(sid, "Call " + name)["run_id"]
        r = app.wait(rid, timeout=30)
        assert r["status"] == "completed", (
            r["status"],
            r.get("error"),
            r.get("stop_reason"),
        )
        calls = app.api("GET", "runs/" + rid + "/tool-calls")
        c = next((c for c in calls if c["tool_id"].endswith("." + name)), None)
        assert c and c["status"] == "completed", (
            name,
            model.offered,
            [(i["tool_id"], i["status"], i.get("error")) for i in calls],
        )
        assert name in model.offered
        assert c["oi_call_id"].startswith("call_") and c["binding_revision"], name
        assert c["binding"]["service"] in ("oi-admin", "workbench"), c["binding"]
        trace.update(run=rid, call=c["id"], oi=c["oi_call_id"])
        return c["result"]

    def data(result):
        if "structuredContent" in result:
            return result["structuredContent"]
        if isinstance(result.get("content"), list):
            return json.loads(result["content"][0]["text"])
        return result

    assert "host-a" in json.dumps(invoke(author, "list_hosts", {}))
    assert "skills" in json.dumps(invoke(author, "list_apps", {"host_id": "host-a"}))
    assert "read_skill" in json.dumps(
        invoke(author, "list_actions", {"host_id": "host-a", "app": "skills"})
    )
    old = data(invoke(author, "get_service", {"id": "workbench"}))
    assert old["id"] == "workbench" and {a["name"] for a in old["actions"]} == {
        "capture_a",
        "view_on_a",
    }
    updated = {k: old[k] for k in ("actions", "access", "publish")}
    updated["actions"] = old["actions"] + skill_actions
    invoke(
        author,
        "apply_service",
        {
            "id": "workbench",
            "service": updated,
        },
    )
    # The user shouldn't need an explicit refresh when OI changes the Service.
    eventually(
        lambda: any(
            t["id"] == "office.workbench.list_skills"
            for t in admin("GET", "catalog/tools")
        ),
        timeout=50,
    )
    if cli.browser:
        assert not app.api("GET", "agents/reader/tools")
        browser("bind")
    original = data(invoke(reader, "list_skills", {}))["items"]
    rows = data(invoke(reader, "list_skills", {"product": "codex"}))["items"]
    assert len(original) == 4 and len(rows) == 2
    ref = next(r["ref"] for r in rows if r["description"] == "project-codex")
    assert "companion" in json.dumps(invoke(reader, "read_skill", {"ref": ref}))
    assert "E2E-project-codex" in json.dumps(
        invoke(reader, "read_skill", {"ref": ref, "resource": "details.txt"})
    )
    if cli.live_text:
        skill_tools = [
            "office.workbench." + name for name in ("list_skills", "read_skill")
        ]
        admin(
            "POST",
            "agents",
            {
                "id": "live-reader",
                "model": "live",
                "tools": [{"toolset": "office.workbench"}],
                "grants": {"tools": skill_tools},
            },
        )
        sid = app.api("POST", "sessions", {"agent_id": "live-reader"})["id"]
        message = (
            "请调用 list_skills 查询 Codex 的 Skill。选描述为 project-codex 的那一项，"
            "使用它的 ref 调用 read_skill 读取包内的 details.txt，最后只回复文件中的标签。"
        )
        rid = app.send(sid, message)["run_id"]
        completed = app.wait(rid, timeout=150)
        assert (
            completed["status"] == "completed"
            and "E2E-project-codex" in completed["output"]
        ), (completed["status"], completed.get("error"), completed.get("output"))
        calls = app.api("GET", "runs/" + rid + "/tool-calls")
        assert {c["tool_id"] for c in calls if c.get("oi_call_id")} == set(skill_tools)
        recorded = app.api("GET", "runs/" + rid + "/model-calls")
        assert all(c["model"] == "live" for c in recorded)
        assert all(
            not any(
                k["function"]["name"] in {"capture_a", "view_on_a"} for k in c["tools"]
            )
            for c in recorded
        )
    capture = data(invoke(reader, "capture_a", {}))
    model.images = []
    image = invoke(reader, "view_on_a", {"path": capture["path"]})
    part = next(p for p in image["content"] if p["type"] == "image")
    assert model.images == [base64.b64decode(part["data"], validate=True)]
    capture_path = exchange / capture["path"]
    original_bytes = capture_path.read_bytes()
    assert original_bytes.startswith(b"\x89PNG\r\n\x1a\n")
    header = {"image/png": b"\x89PNG\r\n\x1a\n", "image/jpeg": b"\xff\xd8\xff"}
    assert model.images[0].startswith(header[part["mimeType"]])
    if not image["structuredContent"].get("reencoded", True):
        assert model.images[0] == original_bytes
    image_trace = trace.copy()
    if cli.browser:
        browser(
            "trace",
            LOOM_BROWSER_TRACE_RUN=trace["run"],
            LOOM_BROWSER_TRACE_CALL=trace["call"],
            LOOM_BROWSER_OI_CALL=trace["oi"],
        )
    invoke(author, "delete_service", {"id": "workbench"})
    admin("POST", "plugins/office/refresh", {})
    assert not app.api("GET", "agents/reader/tools")
    assert (
        app.api("GET", "tool-calls/" + image_trace["call"])["oi_call_id"]
        == image_trace["oi"]
    )
    print(
        json.dumps(
            {
                "service": "workbench",
                "skill_roots": len(original),
                "codex_roots": len(rows),
                "capture_size": (exchange / capture["path"]).stat().st_size,
                "original_sha256": hashlib.sha256(
                    (exchange / capture["path"]).read_bytes()
                ).hexdigest(),
                "withdrawn": True,
                "model_received_image": True,
                "browser": cli.browser,
                "live_skill_read": cli.live_text,
            }
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--live-text",
        action="store_true",
        help="also use DeepSeek to read a synthetic Skill in a separate text-only session",
    )
    parser.add_argument(
        "--browser",
        action="store_true",
        help="exercise WebUI binding and trace through Playwright",
    )
    for name in ("ois", "oic", "management", "skills", "capture", "filesystem"):
        parser.add_argument(
            "--" + name, required=True, type=lambda p: Path(p).resolve(strict=True)
        )
    args = parser.parse_args()
    if args.live_text and not os.environ.get("DEEPSEEK_API_KEY"):
        parser.error("--live-text requires DEEPSEEK_API_KEY")
    with (
        tempfile.TemporaryDirectory(prefix="loom-workspace-e2e-") as temp,
        ExitStack() as stack,
    ):
        run(args, Path(temp), stack)
