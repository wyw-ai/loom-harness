"""Run the catalog acceptance paths against a built loom-harness and local HTTP peers.

    cargo build
    python tests/acceptance_catalog.py

No API keys are needed. The two bearer credentials are random, process-local and
never printed. The model fixture selects the exact tool request under test.
"""

from collections import Counter, deque
import contextlib
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import secrets
import shutil
import socket
import subprocess
import tempfile
import threading
import time
import unittest
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

from harness import service_environment


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def write(home, path, data):
    p = Path(home, path)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(data if isinstance(data, str) else json.dumps(data), encoding="utf-8")


class Peers(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, code, body):
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(body).encode())

    def do_POST(self):
        data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path.endswith("chat/completions"):
            self.server.inputs.append(data)
            if self.server.sample_gate is not None and any(
                "Suggest one short next message" in str(m.get("content"))
                for m in data["messages"]
            ):
                entered, release = self.server.sample_gate
                entered.set()
                release.wait(10)
            instruction = self.server.steps.popleft() if self.server.steps else None
            if not instruction and any(
                m.get("role") == "tool" and "FINAL_REPORT" in str(m.get("content"))
                for m in data["messages"]
            ):
                if any(
                    t["function"]["name"] == "goal_complete"
                    for t in data.get("tools", [])
                ):
                    instruction = (
                        "goal_complete",
                        {"evidence": "FINAL_REPORT received"},
                    )
            report_seen = any(
                m.get("role") == "tool" and "FINAL_REPORT E421" in str(m.get("content"))
                for m in data["messages"]
            )
            msg = {
                "role": "assistant",
                "content": "CI failed with E421." if report_seen else "finished",
            }
            if instruction:
                if callable(instruction):
                    instruction = instruction(data)
                name, arguments = (
                    instruction
                    if isinstance(instruction, tuple)
                    else ("code_exec", {"code": instruction})
                )
                msg = {
                    "role": "assistant",
                    "content": None,
                    "tool_calls": [
                        {
                            "id": secrets.token_hex(8),
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": json.dumps(arguments),
                            },
                        }
                    ],
                }
            return self.reply(
                200,
                {
                    "choices": [
                        {
                            "message": msg,
                            "finish_reason": "tool_calls" if instruction else "stop",
                        }
                    ]
                },
            )
        method = data["method"]
        expected_credential = self.server.expected_credentials.get(self.path)
        if (
            expected_credential
            and self.headers.get("Authorization") != "Bearer " + expected_credential
        ):
            return self.reply(401, {})
        if self.path in self.server.offline_paths:
            return self.reply(503, {})
        if method == "notifications/initialized":
            return self.reply(202, {})
        if method == "initialize":
            value = {
                "protocolVersion": data["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "catalog-fixture", "version": "1"},
            }
        elif method == "tools/list":
            barrier = self.server.list_gate
            if barrier and self.path == barrier[0]:
                self.server.list_gate = None
                barrier[1].set()
                barrier[2].wait(8)
            tools = self.server.tools.get(self.path, ["search", "get_report"])
            if self.server.page_failure:
                if "cursor" in data["params"]:
                    return self.reply(502, {})
                value = {
                    "tools": [{"name": "partial", "inputSchema": {"type": "object"}}],
                    "nextCursor": "p2",
                }
            else:
                value = {
                    "tools": [
                        {
                            "name": name,
                            "description": name,
                            "inputSchema": self.server.signature.get(
                                name, {"type": "object"}
                            ),
                        }
                        for name in tools
                    ]
                }
        elif method == "tools/call":
            name = data["params"]["name"]
            self.server.counts[self.path, name] += 1
            if self.path.startswith("/recovery/"):
                job = "T42-" + self.path.rsplit("/", 1)[-1]
                if name == "submit":
                    key = self.headers["Idempotency-Key"]
                    self.server.jobs.setdefault((self.path, key), job)
                    self.server.submissions.setdefault(job, []).append(
                        (key, data["params"]["arguments"], data["params"]["_meta"])
                    )
                    if len(
                        self.server.submissions[job]
                    ) == 1 and not self.path.endswith("/saved"):
                        self.server.release.wait(20)
                        self.connection.close()
                        return
                else:
                    self.server.lookups[job] = data["params"]["arguments"]
                value = {
                    "content": [],
                    "structuredContent": {"status": "accepted", "job_id": job},
                }
                return self.reply(
                    200, {"jsonrpc": "2.0", "id": data["id"], "result": value}
                )
            result = self.server.results.get(name, name)
            value = {"content": [{"type": "text", "text": result}]}
        else:
            return self.reply(400, {})
        return self.reply(200, {"jsonrpc": "2.0", "id": data["id"], "result": value})


def create_peer(handler=Peers):
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
    server.steps, server.inputs, server.counts = deque(), [], Counter()
    server.tools, server.results, server.signature = {}, {}, {}
    server.page_failure = False
    server.offline_paths = set()
    server.expected_credentials = {}
    server.list_gate = server.sample_gate = None
    return server


class CatalogAcceptance(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.addClassCleanup(cls.tmp.cleanup)
        cls.home = Path(cls.tmp.name)
        cls.peer = create_peer()
        threading.Thread(target=cls.peer.serve_forever, daemon=True).start()
        cls.addClassCleanup(cls.peer.server_close)
        cls.addClassCleanup(cls.peer.shutdown)
        cls.peer_url = "http://127.0.0.1:%d" % cls.peer.server_port
        cls.tokens = {
            name: secrets.token_urlsafe(32)
            for name in ("admin", "user", "other", "ci", "reviewer")
        }
        cls.address = "127.0.0.1:%s" % port()
        cls.env = {
            **service_environment(),
            "LOOM_HOME": str(cls.home),
            "LOOM_BIND": cls.address,
            "LOOM_ROLE": "all",
            "LOOM_WORKERS": "6",
            "LOOM_NOTE_THRESHOLD": "900",
            "LOOM_AUTH_JSON": json.dumps(
                {
                    v: {
                        "owner": "user" if k in ("ci", "reviewer") else k,
                        "admin": k == "admin",
                        "source": k == "ci",
                        "approve": k == "reviewer",
                    }
                    for k, v in cls.tokens.items()
                }
            ),
            "LOOM_MODEL_ROUTES": json.dumps(
                {"test": {"base_url": cls.peer_url, "model": "fixture"}}
            ),
        }
        cls.bin = Path(
            os.environ.get(
                "LOOM_TEST_BINARY",
                Path(__file__).resolve().parents[1]
                / "target/debug"
                / ("loom-harness.exe" if os.name == "nt" else "loom-harness"),
            )
        )
        if not cls.bin.exists():
            raise RuntimeError(
                "run cargo build before this suite, or set LOOM_TEST_BINARY"
            )
        cls.bin = Path(shutil.copy2(cls.bin, cls.home / cls.bin.name))
        if os.getenv("LOOM_TEST_TRACE"):
            print(
                "binary_sha256=" + hashlib.sha256(cls.bin.read_bytes()).hexdigest(),
                flush=True,
            )
        write(
            cls.home,
            "tools/release_tag/tool.yaml",
            {"id": "release_tag", "handler": "handler.lua"},
        )
        write(
            cls.home,
            "tools/release_tag/handler.lua",
            "return function(ctx,args) return true end",
        )
        base = "packages/release-source/1/"
        write(
            cls.home,
            base + "plugin.yaml",
            {"id": "release-source", "version": "1", "entrypoint": "plugin.lua"},
        )
        write(
            cls.home,
            base + "plugin.lua",
            "return function(l) l.register_tool({id='inspect',handler=function() return true end});l.register_toolset({id='checks',members={'inspect'}}) end",
        )
        write(
            cls.home,
            "plugins/release-source/instance.yaml",
            {
                "id": "release-source",
                "package": "release-source",
                "version": "1",
                "enabled": True,
            },
        )
        write(
            cls.home,
            "plugins/release-source/connections/ci.yaml",
            {"id": "ci", "url": cls.peer_url + "/release"},
        )
        write(
            cls.home,
            "agents/release/agent.yaml",
            {
                "id": "release",
                "name": "Release",
                "description": "old",
                "owner": "user",
                "system_prompt": "Use release evidence",
                "model": {"default": "test", "fallbacks": ["backup"]},
                "tools": [
                    {"tool": "home.release_tag"},
                    {"toolset": "release-source.checks"},
                    {"tool": "release-source.ci.search"},
                ],
                "grants": {
                    "tools": [
                        "home.release_tag",
                        "release-source.inspect",
                        "release-source.ci.search",
                    ]
                },
            },
        )
        cls.start_service()
        cls.addClassCleanup(cls.stop_service)

    @classmethod
    def start_service(cls):
        cls.proc = subprocess.Popen(
            [str(cls.bin)],
            env=cls.env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        for _ in range(200):
            if cls.proc.poll() is not None:
                raise RuntimeError("loom-harness exited before HTTP readiness")
            try:
                with urlopen("http://" + cls.address + "/health", timeout=0.5):
                    return
            except (URLError, TimeoutError):
                time.sleep(0.035)
        raise TimeoutError("service readiness")

    @classmethod
    def stop_service(cls):
        cls.proc.terminate()
        cls.proc.wait(timeout=10)

    def api(self, method, path, data=None, actor="user", expected=200, key=None):
        req = Request(
            "http://" + self.address + "/api/" + path,
            data=None if data is None else json.dumps(data).encode(),
            headers={
                "Authorization": "Bearer " + self.tokens[actor],
                "Content-Type": "application/json",
            },
            method=method,
        )
        if key is not None:
            req.add_header("Idempotency-Key", key)
        try:
            response = urlopen(req, timeout=15)
        except HTTPError as e:
            response = e
        with contextlib.closing(response):
            body = json.load(response)
            if os.getenv("LOOM_TEST_EVIDENCE"):
                from harness import record_http

                record_http(actor, method, path, response.code, data, body)
            self.assertEqual(response.code, expected, (method, path, body))
            return body

    def admin(self, method, path, body=None, expected=200):
        return self.api(method, path, body, "admin", expected)

    def agent(self, name, tools=None, grants=None, **extra):
        body = {"id": name, "model": "test", "owner": "user", **extra}
        if tools is not None:
            body["tools"] = tools
        if grants is not None:
            body["grants"] = grants
        return self.admin("POST", "agents", body)

    def session(self, agent):
        return self.api("POST", "sessions", {"agent_id": agent})["id"]

    def event(self, content, actor="user"):
        return self.api(
            "POST",
            "events",
            {"type": "user.message", "source": "user", "content": content},
            actor,
        )["id"]

    def run_tools(self, session, steps):
        self.assertFalse(self.peer.steps)
        self.peer.steps.extend(steps)
        eid = self.event("execute the specified catalog test")
        delivery = self.api(
            "POST",
            f"events/{eid}/deliveries",
            {"target": {"session_id": session}, "action": "start_run"},
            expected=202,
        )
        rid = delivery["run_id"]
        for _ in range(240):
            run = self.api("GET", f"runs/{rid}")
            if run["status"] in (
                "completed",
                "delivered",
                "succeeded",
                "failed",
                "cancelled",
            ):
                break
            time.sleep(0.05)
        self.assertNotIn(
            run["status"], ("failed", "cancelled", "queued", "running", "waiting"), run
        )
        self.assertFalse(self.peer.steps)
        calls = self.api("GET", f"runs/{rid}/tool-calls")
        return run, calls

    def test_01_home_templates_and_versions(self):
        ids = {x["id"] for x in self.api("GET", "agent-templates")}
        self.assertTrue({"standard", "creator"} <= ids)
        agent = self.api("GET", "agents/release")
        self.admin("POST", "mcp/connections/release-source/ci/refresh", {})
        self.assertEqual(
            [t["id"] for t in self.api("GET", "agents/release/tools")],
            ["home.release_tag", "release-source.ci.search", "release-source.inspect"],
        )
        self.assertIn({"toolset": "release-source.checks"}, agent["tools"])
        self.assertEqual(agent["model"], {"default": "test", "fallbacks": ["backup"]})
        self.assertEqual(agent["system_prompt"], "Use release evidence")
        self.assertEqual(self.api("GET", "agents/release"), agent)
        changed = self.api(
            "PATCH",
            "agents/release",
            {"expected_revision": agent["revision"], "description": "new"},
        )
        new = self.api(
            "PATCH",
            "agents/release",
            {"expected_revision": changed["revision"], "model": {"default": "backup"}},
        )
        self.api(
            "PATCH",
            "agents/release",
            {"expected_revision": changed["revision"], "tools": []},
            expected=409,
        )
        self.assertEqual(
            self.api("GET", "agents/release")["model"]["default"], "backup"
        )
        self.assertEqual(self.api("GET", "agents/release")["tools"], agent["tools"])
        self.admin("POST", "agents", {"id": "release"}, expected=409)
        self.assertIn(
            "new", (self.home / "agents/release/agent.yaml").read_text(encoding="utf-8")
        )
        self.api("GET", "agents/release", actor="other", expected=404)
        self.assertEqual(
            len([x for x in self.api("GET", "agents") if x["id"] == "release"]), 1
        )
        for ver, aid in [("1", "fromone"), ("2", "fromtwo")]:
            self.admin(
                "POST",
                "plugins/templates/versions",
                {
                    "version": ver,
                    "code": "return function(loom) end",
                    "templates": [
                        {
                            "id": "third",
                            "spec": {
                                "system_prompt": ver,
                                "tools": [{"tool": "plugins.install"}],
                            },
                        }
                    ],
                },
            )
            if ver == "1":
                self.admin(
                    "POST",
                    "plugins",
                    {
                        "id": "templates",
                        "package": "templates",
                        "version": ver,
                        "enabled": True,
                    },
                )
            else:
                self.admin("PATCH", "plugins/templates", {"version": ver})
            self.api(
                "POST",
                "agents",
                {
                    "id": aid,
                    "template": {"id": "third", "source": "templates", "version": ver},
                },
            )
            if ver == "1":
                self.api("PATCH", "agents/" + aid, {"description": "independent edit"})
        self.assertEqual(self.api("GET", "agents/fromone")["system_prompt"], "1")
        self.assertEqual(
            self.api("GET", "agents/fromone")["description"], "independent edit"
        )
        self.assertEqual(self.api("GET", "agents/fromtwo")["system_prompt"], "2")
        self.assertTrue(
            (self.home / "agents/fromone/agent.yaml").is_file()
            and (self.home / "agents/fromtwo/agent.yaml").is_file()
        )
        self.assertNotIn(
            "plugins.install",
            [t["id"] for t in self.api("GET", "agents/fromtwo/tools")],
        )
        self.stop_service()
        self.start_service()
        self.assertEqual(self.api("GET", "agents/release")["revision"], new["revision"])

    def test_02_instance_scopes_and_extensions(self):
        code = """return function(loom)
        loom.register_tool({id='echo', handler=function(ctx,args) return ctx.agent_config.n end})
        loom.register_route({method='GET',path='/mode',handler=function(ctx,args) return ctx.settings.mode end})
        loom.register_section({id='hint',text='CATALOG_SECTION_MARKER'})
        end"""
        schema = {
            "type": "object",
            "properties": {"n": {"type": "integer", "minimum": 1}},
            "required": ["n"],
        }
        self.admin(
            "POST",
            "plugins/echo/versions",
            {"version": "1", "manifest": {"agent_config_schema": schema}, "code": code},
        )
        for p in ["p", "q"]:
            self.admin(
                "POST",
                "plugins",
                {
                    "id": p,
                    "package": "echo",
                    "version": "1",
                    "enabled": True,
                    "allowed_owners": ["user"],
                },
            )
            self.admin("PATCH", "settings/" + p, {"mode": p})
            self.assertEqual(self.api("GET", "extensions/" + p + "/mode"), p)
        for a, config in [
            ("echo-a", {"p": {"n": 11}, "q": {"n": 22}}),
            ("echo-b", {"p": {"n": 33}}),
        ]:
            names = [i + ".echo" for i in config]
            self.agent(
                a,
                [{"tool": n} for n in names],
                {"tools": names, "plugins": list(config)},
                extensions=config,
            )
            _, calls = self.run_tools(
                self.session(a), ["return tools.call('%s', {})" % n for n in names]
            )
            actual = {
                c["tool_id"]: c["result"] for c in calls if c.get("tool_id") in names
            }
            self.assertEqual(actual, {i + ".echo": v["n"] for i, v in config.items()})
        self.api(
            "POST",
            "agents",
            {"id": "invalidext", "extensions": {"q": {"n": 0}}},
            expected=400,
        )
        self.api("GET", "agents/invalidext", expected=404)
        self.assertNotIn("invalidext", [x["id"] for x in self.api("GET", "agents")])
        self.assertEqual(self.admin("GET", "settings/p")["mode"], "p")
        paths = self.api("GET", "openapi.json")["paths"]
        self.assertIn("/api/extensions/p/mode", paths)
        self.assertNotIn(
            "/api/extensions/p/mode",
            self.api("GET", "openapi.json", actor="other")["paths"],
        )
        self.assertEqual(self.api("GET", "docs/agent-authoring@1")["version"], "1")

    def test_03_mixed_sources_discovery_and_stale_binding(self):
        definition = {
            "id": "check",
            "handler": "handler.lua",
            "input_schema": {
                "type": "object",
                "properties": {"tag": {"type": "string"}},
                "required": ["tag"],
            },
        }
        write(self.home, "tools/check/tool.yaml", definition)
        write(
            self.home,
            "tools/check/handler.lua",
            "return function(ctx,args) return args.tag:match('^v%d+%.%d+%.%d+$') ~= nil end",
        )
        self.admin(
            "POST",
            "plugins/mcp/versions",
            {"version": "1", "code": "return function(loom) end"},
        )
        for inst in ("remote", "otherremote"):
            self.admin(
                "POST",
                "plugins",
                {"id": inst, "package": "mcp", "version": "1", "enabled": True},
            )
            self.admin(
                "POST",
                "mcp/connections",
                {"id": "ci", "instance": inst, "url": self.peer_url + "/" + inst},
            )
            self.admin("POST", "mcp/connections/" + inst + "/ci/refresh", {})
        write(
            self.home,
            "plugins/remote/toolsets/mixed.yaml",
            {
                "id": "mixed",
                "members": [
                    "home.check",
                    "remote.ci.get_report",
                    "otherremote.ci.search",
                ],
            },
        )
        write(
            self.home,
            "plugins/remote/toolsets/same-name.yaml",
            {
                "id": "same-name",
                "members": ["remote.ci.search", "otherremote.ci.search"],
            },
        )
        self.admin("POST", "catalog/apply", {})
        self.agent(
            "only-one-ci",
            [{"toolset": "remote.same-name"}],
            {"tools": ["remote.ci.search"]},
        )
        self.assertEqual(
            [t["id"] for t in self.api("GET", "agents/only-one-ci/tools")],
            ["remote.ci.search"],
        )
        search_sid = self.session("only-one-ci")
        self.run_tools(search_sid, ["return tools.call('remote.ci.search',{})"])
        self.assertEqual(self.peer.counts["/remote", "search"], 1)
        self.assertEqual(self.peer.counts["/otherremote", "search"], 0)

        def revoke_search(_):
            self.admin("PATCH", "agents/only-one-ci", {"grants": {"tools": []}})
            return "return tools.call('remote.ci.search',{})"

        _, rejected = self.run_tools(search_sid, [revoke_search])
        self.assertTrue(
            any(
                c["tool_id"] == "remote.ci.search" and c["status"] == "failed"
                for c in rejected
            )
        )
        self.assertEqual(self.peer.counts["/remote", "search"], 1)
        self.assertEqual(self.peer.counts["/otherremote", "search"], 0)
        bindings = [{"toolset": "remote.mixed"}, {"tool": "home.check"}]
        self.agent("mixed", bindings, {"tools": ["home.check", "remote.ci.get_report"]})
        self.assertEqual(
            [t["id"] for t in self.api("GET", "agents/mixed/tools")],
            ["home.check", "remote.ci.get_report"],
        )
        s = self.session("mixed")
        _, calls = self.run_tools(
            s,
            [
                "local a=tools.call('home.check',{tag='v1.2.3-rc.1'});local b=tools.call('remote.ci.get_report',{});return {a=a,b=b}"
            ],
        )
        outer = next(c for c in calls if c["tool_id"] == "code_exec")
        children = [c for c in calls if c.get("parent_id") == outer["id"]]
        self.assertEqual(len(children), 2)
        self.assertEqual(
            next(c["result"] for c in children if c["tool_id"] == "home.check"), False
        )
        self.assertEqual(self.peer.counts["/remote", "get_report"], 1)
        self.assertEqual(self.peer.counts["/otherremote", "search"], 0)
        _, wrong = self.run_tools(s, [("remote_dci_dget_ureport", {})])
        self.assertTrue(any(c["status"] == "failed" for c in wrong))
        self.assertEqual(self.peer.counts["/remote", "get_report"], 1)
        definition["description"] = "Accept formal tags and release candidates"
        write(self.home, "tools/check/tool.yaml", definition)
        write(
            self.home,
            "tools/check/handler.lua",
            "return function(ctx,args) return args.tag:match('^v%d+%.%d+%.%d+%-rc%.%d+$') ~= nil or args.tag:match('^v%d+%.%d+%.%d+$') ~= nil end",
        )
        self.admin("POST", "catalog/apply", {})
        rev1 = next(
            t["revision"]
            for t in self.api("GET", "agents/mixed/tools")
            if t["id"] == "home.check"
        )
        write(
            self.home,
            "tools/check/handler.lua",
            "return function(ctx,args) return args.tag=='v1.2.3-rc.1' or args.tag=='v1.2.3' end",
        )
        self.admin("POST", "catalog/apply", {})
        rev2 = next(
            t["revision"]
            for t in self.api("GET", "agents/mixed/tools")
            if t["id"] == "home.check"
        )
        self.assertNotEqual(rev1, rev2)
        self.assertIn(
            "match",
            self.admin("GET", "catalog/tools/home.check/versions/" + rev1)["code"],
        )
        write(self.home, "tools/check/handler.lua", "invalid lua!")
        self.assertTrue(
            any(
                x["id"] == "home.check" for x in self.admin("POST", "catalog/apply", {})
            )
        )
        _, calls = self.run_tools(
            s, ["return tools.call('home.check',{tag='v1.2.3-rc.1'})"]
        )
        self.assertIs(
            next(c["result"] for c in calls if c["tool_id"] == "home.check"), True
        )
        baseline = self.admin("GET", "mcp/connections/remote/ci/discovery")
        self.peer.page_failure = True
        try:
            self.admin("POST", "mcp/connections/remote/ci/refresh", {}, expected=400)
        finally:
            self.peer.page_failure = False
        self.assertEqual(
            self.admin("GET", "mcp/connections/remote/ci/discovery"), baseline
        )
        self.peer.offline_paths.add("/remote")
        try:
            self.admin("POST", "mcp/connections/remote/ci/refresh", {}, expected=400)
            self.assertEqual(
                [d["id"] for d in self.api("GET", "agents/mixed/tools")], ["home.check"]
            )
            _, local = self.run_tools(
                s, ["return tools.call('home.check',{tag='v1.2.3'})"]
            )
            self.assertTrue(
                any(c["tool_id"] == "home.check" and c["result"] is True for c in local)
            )
        finally:
            self.peer.offline_paths.remove("/remote")
        self.admin("POST", "mcp/connections/remote/ci/refresh", {})

        def revoke_after_model_input(_request):
            self.admin("PATCH", "agents/mixed", {"grants": {"tools": ["home.check"]}})
            return "return tools.call('remote.ci.get_report',{})"

        _, calls = self.run_tools(s, [revoke_after_model_input])
        self.assertTrue(any(c["status"] == "failed" for c in calls))
        self.assertEqual(self.peer.counts["/remote", "get_report"], 1)
        self.peer.tools["/remote"] = ["search"]
        self.admin("POST", "mcp/connections/remote/ci/refresh", {})
        self.assertEqual(
            [t["id"] for t in self.api("GET", "agents/mixed/tools")], ["home.check"]
        )

    def test_04_standard_session_inbox_and_revoked_authority(self):
        self.agent("daily")
        self.api("POST", "agents", {"id": "isolated"}, actor="other")
        sid = self.session("daily")
        e1, e2, e3 = (
            self.event("SESSION_ONLY"),
            self.event("DIRECT_ONLY"),
            self.event("OTHER_OWNER", "other"),
        )
        for e, target, actor in [
            (e1, {"session_id": sid}, "user"),
            (e2, {"agent_id": "daily"}, "user"),
            (e3, {"agent_id": "isolated"}, "other"),
        ]:
            self.api(
                "POST",
                f"events/{e}/deliveries",
                {"target": target, "action": "attach"},
                actor,
                202,
            )
        before = [self.api("GET", f"events/{e}/deliveries") for e in (e1, e2)]
        _, calls = self.run_tools(
            sid,
            [
                "return tools.call('agents.get',{})",
                "return tools.call('sessions.history',{})",
                "return tools.call('events.search',{scope='current_agent_inbox'})",
                "return tools.call('write_note',{description='example',body='actual note'})",
                "return tools.call('plugins.install',{id='attacker'})",
            ],
        )

        def get(name):
            return next(c for c in calls if c["tool_id"] == name)

        self.assertEqual(get("agents.get")["result"]["id"], "daily")
        self.assertIn("SESSION_ONLY", json.dumps(get("sessions.history")["result"]))
        self.assertNotIn("DIRECT_ONLY", json.dumps(get("sessions.history")["result"]))
        self.assertEqual([e["id"] for e in get("events.search")["result"]], [e2])
        self.assertNotIn("OTHER_OWNER", json.dumps(calls))
        self.assertEqual(get("plugins.install")["status"], "failed")
        note_ref = get("write_note")["result"]["note_ref"]
        _, calls = self.run_tools(
            sid,
            [
                "return tools.call('search_notes',{keyword='example'})",
                "return tools.call('read_note',{note_ref='%s'})" % note_ref,
            ],
        )
        self.assertTrue(
            any(
                c.get("result", {}).get("body") == "actual note"
                for c in calls
                if isinstance(c.get("result"), dict)
            )
        )
        self.assertEqual(
            before, [self.api("GET", f"events/{e}/deliveries") for e in (e1, e2)]
        )
        self.assertEqual(len(self.api("GET", "sessions/" + sid + "/runs")), 2)
        self.api("GET", "events/" + e3, expected=404)
        self.api(
            "PATCH",
            "agents/daily",
            {"grants": {"operations": ["plugins.install"]}},
            expected=403,
        )

    def test_05_creator_authors_and_uses_a_real_plugin(self):
        self.admin(
            "POST",
            "plugins/cilog/versions",
            {"version": "1", "code": "return function(loom) end"},
        )
        self.admin(
            "POST",
            "plugins",
            {"id": "cilog", "package": "cilog", "version": "1", "enabled": True},
        )
        self.admin(
            "POST",
            "mcp/connections",
            {"id": "build", "instance": "cilog", "url": self.peer_url + "/log"},
        )
        self.peer.tools["/log"] = ["build_report"]
        self.peer.results["build_report"] = "\n".join(
            "E421 missing dependency" if i == 280 else "ordinary line %d" % i
            for i in range(1, 301)
        )
        self.admin("POST", "mcp/connections/cilog/build/refresh", {})
        base = [
            {"toolset": x}
            for x in ["loom.daily", "plugins.authoring", "loom.self_management"]
        ]
        self.agent(
            "author",
            base + [{"tool": "cilog.build.build_report"}],
            {
                "plugins": ["extract"],
                "operations": [
                    "agents.update",
                    "plugins.install",
                    "plugins.set_enabled",
                ],
                "tools": ["cilog.build.build_report"],
            },
            template="creator",
        )
        sid = self.session("author")
        _, original = self.run_tools(
            sid, ["return tools.call('cilog.build.build_report',{})"]
        )
        ref = next(
            c["result"]["note_ref"]
            for c in original
            if c["tool_id"] == "cilog.build.build_report"
        )
        package = """return function(loom) loom.register_tool({id='code',input_schema={type='object',properties={note_ref={type='string'}}},handler=function(ctx,args) return ctx.read_note(args.note_ref):match('E%d+') end}) end"""
        bind = "return tools.call('agents.update',{id='author',patch={tools={{toolset='loom.daily'},{toolset='plugins.authoring'},{toolset='loom.self_management'},{tool='extract.code'}}}})"
        _, calls = self.run_tools(
            sid,
            [
                "return tools.call('harness.docs',{id='agent-authoring'})",
                "return tools.call('search_notes',{keyword='build_report'})",
                "return tools.call('plugins.install',{id='extract',version='1',manifest={},code=[[%s]]})"
                % package,
                "return tools.call('plugins.set_enabled',{id='extract',version='1',enabled=true})",
                bind,
                "return tools.call('extract.code',{note_ref='%s'})" % ref,
            ],
        )
        executed = next(c for c in calls if c["tool_id"] == "extract.code")
        if os.getenv("LOOM_TEST_TRACE"):
            print(
                json.dumps(
                    {
                        "creator_tool_call": executed["id"],
                        "note_ref": ref,
                        "result": executed["result"],
                    }
                ),
                flush=True,
            )
        self.assertEqual(executed["result"], "E421")
        self.assertEqual(self.peer.counts["/log", "build_report"], 1)
        snapshots = [
            m
            for m in self.peer.inputs
            if "extract.code" in json.dumps(m.get("tools", []))
        ]
        self.assertTrue(snapshots)
        _, denied = self.run_tools(
            sid,
            [
                "return tools.call('agents.update',{id='stranger',patch={description='bad'}})",
                "return tools.call('plugins.set_enabled',{id='cilog',enabled=false})",
                "return tools.call('plugins.set_enabled',{id='extract',enabled=false})",
            ],
        )
        self.assertEqual(
            sum(c["status"] == "failed" for c in denied if c["tool_id"] != "code_exec"),
            2,
        )
        self.assertTrue(self.admin("GET", "plugins/cilog")["enabled"])
        self.assertNotIn(
            "extract.code", [d["id"] for d in self.api("GET", "agents/author/tools")]
        )
        self.assertEqual(
            self.api("GET", "tool-calls/" + executed["id"])["result"], "E421"
        )
        self.admin("PATCH", "plugins/extract", {"enabled": True})

        def disable_after_model_input(_):
            self.admin("PATCH", "plugins/extract", {"enabled": False})
            return "return tools.call('extract.code',{note_ref='%s'})" % ref

        _, late = self.run_tools(sid, [disable_after_model_input])
        self.assertTrue(
            any(
                c["tool_id"] == "extract.code" and c["status"] == "failed" for c in late
            )
        )

    def test_06_t12_lost_first_receipt_after_worker_replacement(self):
        self.peer.release = threading.Event()
        self.peer.submissions, self.peer.lookups, self.peer.jobs = {}, {}, {}
        self.admin(
            "POST",
            "plugins/recovery/versions",
            {"version": "1", "code": "return function(loom) end"},
        )
        self.admin(
            "POST",
            "plugins",
            {"id": "recovery", "package": "recovery", "version": "1", "enabled": True},
        )
        initial_settings = self.admin(
            "PATCH", "settings/recovery", {"label": "initial"}
        )
        modes = {
            "saved": {},
            "query": {"lookup_tool": "lookup", "lookup_key_arg": "key"},
            "repeat": {"idempotent_resubmit": True, "ttl_ms": 300000},
            "unknown": {},
            "ordinary": {},
            "expired": {"idempotent_resubmit": True, "ttl_ms": 1},
        }
        runs = {}
        for name, policy in modes.items():
            path = "/recovery/" + name
            self.peer.tools[path] = ["submit", "lookup"]
            qualified = "recovery." + name + ".submit"
            self.admin(
                "POST",
                "mcp/connections",
                {
                    "id": name,
                    "instance": "recovery",
                    "url": self.peer_url + path,
                    "imports": [
                        {"name": "submit", "effectful": True, "recovery": policy}
                    ],
                },
            )
            self.admin("POST", "mcp/connections/recovery/" + name + "/refresh", {})
            agent = "recover-" + name
            self.agent(agent, [{"tool": qualified}], {"tools": [qualified]})
            sid = self.session(agent)
            self.peer.steps.append("return tools.call('%s',{build='B42'})" % qualified)
            eid = self.event("wait for the final report " + name)
            delivery = {"target": {"session_id": sid}, "action": "start_run"}
            if name == "unknown":
                delivery["goal"] = {"objective": "analyze the final report"}
            runs[name] = self.api(
                "POST", f"events/{eid}/deliveries", delivery, expected=202
            )["run_id"]
            deadline = time.monotonic() + 6
            while (
                "T42-" + name not in self.peer.submissions
                and time.monotonic() < deadline
            ):
                time.sleep(0.04)
            self.assertIn("T42-" + name, self.peer.submissions)
            if name == "saved":
                deadline = time.monotonic() + 6
                while (
                    self.api("GET", "runs/" + runs[name])["status"] != "waiting"
                    and time.monotonic() < deadline
                ):
                    time.sleep(0.04)
                old = self.api("GET", "runs/" + runs[name])["agent_revision"]
                self.assertNotEqual(
                    self.admin(
                        "PATCH",
                        "agents/" + agent,
                        {"description": "changed after dispatch"},
                    )["revision"],
                    old,
                )
                self.admin(
                    "PATCH", "settings/recovery", {"label": "changed after dispatch"}
                )
        self.stop_service()
        self.peer.release.set()
        self.start_service()
        # A real process claims the three original leases after expiry. Do not alter the DB clock.
        deadline = time.monotonic() + 70
        while time.monotonic() < deadline:
            if all(
                self.api("GET", f"runs/{r}")["status"] == "waiting"
                for r in runs.values()
            ):
                break
            time.sleep(0.2)
        for name, rid in runs.items():
            job = "T42-" + name
            self.assertEqual(self.api("GET", f"runs/{rid}")["status"], "waiting")
            calls = self.api("GET", f"runs/{rid}/tool-calls")
            self.assertEqual(len(calls), 2)
            child = next(c for c in calls if c["tool_id"] != "code_exec")
            self.assertTrue(child["parent_id"])
            if name == "saved":
                stored = self.api("GET", "runs/" + rid)
                self.assertNotEqual(
                    stored["agent_revision"],
                    self.api("GET", "agents/recover-saved")["revision"],
                )
                old_dir = self.api("GET", f"runs/{rid}/model-calls")[0][
                    "tool_directory"
                ]
                historical = next(t for t in old_dir if t["id"] == child["tool_id"])
                self.assertEqual(
                    historical["settings_revision"], initial_settings["revision"]
                )
                self.assertEqual(historical["instance"], "recovery")
                self.assertEqual(historical["version"], "1")
                self.assertEqual(
                    self.admin(
                        "GET",
                        "settings/recovery/versions/" + historical["settings_revision"],
                    )["label"],
                    "initial",
                )
                self.assertIn(historical, child["directory"])
            accepted = self.peer.submissions[job]
            self.assertEqual(len(accepted), 2 if name == "repeat" else 1)
            k = accepted[0][0]
            self.assertEqual(k, child["external_dispatch"]["idempotency_key"])
            self.assertTrue(
                all(
                    x[0] == k
                    and x[1] == {"build": "B42"}
                    and x[2]["loom.tool_call_id"] == child["id"]
                    for x in accepted
                )
            )
            if name == "query":
                self.assertEqual(self.peer.lookups[job], {"key": k})
            else:
                self.assertNotIn(job, self.peer.lookups)
            receipt = "tool-calls/" + child["id"] + "/results"
            if name in ("unknown", "ordinary", "expired"):
                self.assertEqual(child["status"], "unknown")
                self.api(
                    "POST",
                    receipt,
                    {"status": "accepted", "job_id": job},
                    "ci",
                    key=job + "-late",
                )
                self.assertEqual(self.api("GET", f"runs/{rid}")["status"], "waiting")
            else:
                self.assertEqual(child["job_id"], job)
            result = {
                "status": "completed",
                "job_id": job,
                "result": "FINAL_REPORT E421 " + job,
            }
            self.api("POST", receipt, result, "ci", key=job + "-final-1")
            self.api("POST", receipt, result, "ci", key=job + "-final-2")
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline and any(
            self.api("GET", f"runs/{r}")["status"] != "completed" for r in runs.values()
        ):
            time.sleep(0.07)
        for name, rid in runs.items():
            run = self.api("GET", f"runs/{rid}")
            self.assertEqual(
                run["status"],
                "completed",
                (
                    name,
                    run.get("waiting_for"),
                    run.get("round"),
                    [
                        (c["tool_id"], c["status"])
                        for c in self.api("GET", f"runs/{rid}/tool-calls")
                    ],
                ),
            )
            self.assertIn("E421", str(run["output"]))
            events = self.api("GET", "events?type=run.wait_satisfied&run_id=" + rid)
            self.assertEqual(len(events), 1)
            self.assertEqual(
                len(self.peer.submissions["T42-" + name]), 2 if name == "repeat" else 1
            )
            self.assertEqual(
                sum(path == "/recovery/" + name for path, _ in self.peer.jobs), 1
            )
            calls = self.api("GET", f"runs/{rid}/model-calls")
            analyses = [
                c
                for c in calls
                if any(
                    m.get("role") == "tool" and "FINAL_REPORT" in str(m.get("content"))
                    for m in c["messages"]
                )
                and not any(
                    m.get("role") == "tool"
                    and "Completion recorded:" in str(m.get("content"))
                    for m in c["messages"]
                )
            ]
            self.assertEqual(len(analyses), 1)
            if os.getenv("LOOM_TEST_TRACE"):
                print(
                    json.dumps(
                        {
                            "branch": name,
                            "run_id": rid,
                            "jobs": 1,
                            "submit_requests": len(
                                self.peer.submissions["T42-" + name]
                            ),
                            "report_analyses": len(analyses),
                            "wakes": len(events),
                            "output": run["output"],
                        }
                    ),
                    flush=True,
                )

    def test_07_new_connection_revision_beats_late_refresh(self):
        self.admin(
            "POST",
            "plugins/latency/versions",
            {"version": "1", "code": "return function(loom) end"},
        )
        self.admin(
            "POST",
            "plugins",
            {"id": "late", "package": "latency", "version": "1", "enabled": True},
        )
        connection = "mcp/connections/late/ci"
        self.admin(
            "POST",
            "mcp/connections",
            {"id": "ci", "instance": "late", "url": self.peer_url + "/old"},
        )
        self.peer.tools["/old"] = ["old_name"]
        self.peer.tools["/new"] = ["new_name"]
        self.admin("POST", connection + "/refresh", {})
        entered, release = threading.Event(), threading.Event()
        self.peer.list_gate = ("/old", entered, release)
        errors = []

        def finish_old():
            try:
                self.admin("POST", connection + "/refresh", {}, expected=409)
            except Exception as e:
                errors.append(e)

        thread = threading.Thread(target=finish_old)
        thread.start()
        try:
            self.assertTrue(entered.wait(4))
            self.admin("PATCH", connection, {"url": self.peer_url + "/new"})
            self.admin("POST", connection + "/refresh", {})
        finally:
            release.set()
            thread.join(5)
        self.assertFalse(errors, errors)
        self.assertEqual(
            self.admin("GET", connection + "/discovery")["members"],
            ["late.ci.new_name"],
        )
        self.agent(
            "lateagent", [{"tool": "late.ci.new_name"}], {"tools": ["late.ci.new_name"]}
        )

        def change_signature(_):
            self.peer.signature["new_name"] = {
                "type": "object",
                "properties": {"revision": {"type": "integer"}},
            }
            self.admin("POST", connection + "/refresh", {})
            return "return tools.call('late.ci.new_name',{})"

        _, calls = self.run_tools(self.session("lateagent"), [change_signature])
        self.assertTrue(
            any(
                c["status"] == "failed" and c["tool_id"] == "late.ci.new_name"
                for c in calls
            )
        )
        self.assertEqual(self.peer.counts["/new", "new_name"], 0)
        self.assertIn(
            "revision",
            self.api("GET", "agents/lateagent/tools")[0]["input_schema"]["properties"],
        )
        self.admin(
            "PATCH",
            connection,
            {"imports": [{"name": "new_name", "requires_approval": True}]},
        )
        self.admin("POST", connection + "/refresh", {})
        self.peer.steps.append("return tools.call('late.ci.new_name',{})")
        sid = self.session("lateagent")
        eid = self.event("hold original call at the approval boundary")
        rid = self.api(
            "POST",
            f"events/{eid}/deliveries",
            {"target": {"session_id": sid}, "action": "start_run"},
            expected=202,
        )["run_id"]
        for _ in range(120):
            if self.api("GET", "runs/" + rid)["status"] == "waiting":
                break
            time.sleep(0.04)
        old_call = next(
            c
            for c in self.api("GET", f"runs/{rid}/tool-calls")
            if c["tool_id"] == "late.ci.new_name"
        )
        self.assertEqual(old_call["status"], "awaiting_approval")
        old_model = self.api("GET", f"runs/{rid}/model-calls")[0]
        self.peer.signature["new_name"] = {"type": "object", "required": ["new_build"]}
        self.admin("POST", connection + "/refresh", {})
        self.stop_service()
        self.start_service()
        self.api(
            "POST",
            "tool-calls/" + old_call["id"] + "/approval",
            {"allow": True, "arguments": old_call["arguments"]},
            actor="reviewer",
            key="approve-original",
        )
        for _ in range(120):
            if self.api("GET", "runs/" + rid)["status"] == "completed":
                break
            time.sleep(0.04)
        self.assertEqual(
            self.api("GET", "tool-calls/" + old_call["id"])["status"], "failed"
        )
        self.assertEqual(
            self.api("GET", "model-calls/" + old_model["id"])["tool_directory"],
            old_model["tool_directory"],
        )
        self.assertEqual(self.peer.counts["/new", "new_name"], 0)

    def test_07b_connections_keep_their_addresses_and_identity(self):
        self.admin(
            "POST",
            "plugins/twoservers/versions",
            {"version": "1", "code": "return function(loom) end"},
        )
        self.admin(
            "POST",
            "plugins",
            {"id": "two", "package": "twoservers", "version": "1", "enabled": True},
        )
        original = self.admin("GET", "plugins/twoservers/versions/1")
        for name in ["ci", "docs"]:
            secret = secrets.token_urlsafe(32)
            self.env["LOOM_MCP_CREDENTIAL_" + name.upper() + "KEY"] = secret
            self.peer.expected_credentials["/two-" + name] = secret
            self.admin(
                "POST",
                "mcp/connections",
                {
                    "id": name,
                    "instance": "two",
                    "url": self.peer_url + "/two-" + name,
                    "credential_ref": name + "key",
                },
            )
        self.stop_service()
        self.start_service()
        self.peer.expected_credentials["/two-ci-new"] = self.peer.expected_credentials[
            "/two-ci"
        ]
        for name in ["ci", "docs"]:
            self.admin("POST", "mcp/connections/two/" + name + "/refresh", {})
        self.admin(
            "PATCH", "mcp/connections/two/ci", {"url": self.peer_url + "/two-ci-new"}
        )
        self.admin("POST", "mcp/connections/two/ci/refresh", {})
        names = ["two.ci.search", "two.docs.search"]
        self.agent("two-agent", [{"tool": x} for x in names], {"tools": names})
        self.run_tools(
            self.session("two-agent"), ["return tools.call('%s',{})" % x for x in names]
        )
        self.assertEqual(self.peer.counts["/two-ci", "search"], 0)
        self.assertEqual(self.peer.counts["/two-ci-new", "search"], 1)
        self.assertEqual(self.peer.counts["/two-docs", "search"], 1)
        self.assertEqual(original, self.admin("GET", "plugins/twoservers/versions/1"))
        self.assertEqual(
            self.admin("GET", "mcp/connections/two/docs")["credential_ref"], "docskey"
        )

    def test_08_independent_trigger_context_and_prompt_section(self):
        source = """return function(loom)
            loom.register_trigger(function(ctx,input) return ctx.settings.open == true end)
            loom.register_context(function(ctx,input) return {input.messages[#input.messages]} end)
            loom.register_section({id='explain',text='ACTIVE_CATALOG_SECTION'})
            end"""
        self.admin("POST", "plugins/hooks/versions", {"version": "1", "code": source})
        self.admin(
            "POST",
            "plugins",
            {"id": "hooks", "package": "hooks", "version": "1", "enabled": True},
        )
        self.admin("PATCH", "settings/hooks", {"open": False})
        self.agent("hooks-only", [], {"plugins": ["hooks"]}, trigger_plugin="hooks")
        sid = self.session("hooks-only")
        prior = self.event("HISTORIC_HOOK_MARKER")
        self.api(
            "POST",
            f"events/{prior}/deliveries",
            {"target": {"session_id": sid}, "action": "attach"},
            expected=202,
        )
        eid = self.event("ask whether input is ready")
        r = self.api(
            "POST",
            f"events/{eid}/deliveries",
            {"target": {"session_id": sid}, "action": "start_run"},
            expected=202,
        )["run_id"]
        for _ in range(100):
            if self.api("GET", "runs/" + r)["status"] in (
                "waiting",
                "waiting_for_input",
            ):
                break
            time.sleep(0.05)
        self.assertEqual(self.api("GET", f"runs/{r}/model-calls"), [])
        self.admin("PATCH", "settings/hooks", {"open": True})
        eid = self.event("NOW_READY")
        self.api(
            "POST",
            f"events/{eid}/deliveries",
            {"target": {"session_id": sid}, "action": "add_input", "run_id": r},
            expected=202,
        )
        for _ in range(100):
            if self.api("GET", "runs/" + r)["status"] == "completed":
                break
            time.sleep(0.05)
        self.assertEqual(self.api("GET", "runs/" + r)["status"], "completed")
        actual = json.dumps(self.api("GET", f"runs/{r}/model-calls")[0]["messages"])
        for marker in ("HISTORIC_HOOK_MARKER", "ACTIVE_CATALOG_SECTION", "NOW_READY"):
            self.assertIn(marker, actual)
        self.agent(
            "context-only",
            [],
            {"plugins": ["hooks"]},
            context_plugin="hooks",
            system_prompt="HOST_CONSTRAINT",
        )
        sid = self.session("context-only")
        eid = self.event("OLD_OMITTED_BY_CONTEXT")
        self.api(
            "POST",
            f"events/{eid}/deliveries",
            {"target": {"session_id": sid}, "action": "attach"},
            expected=202,
        )
        r, _ = self.run_tools(sid, [])
        actual = json.dumps(
            self.api("GET", f"runs/{r['id']}/model-calls")[0]["messages"]
        )
        self.assertNotIn("OLD_OMITTED_BY_CONTEXT", actual)
        for marker in (
            "HOST_CONSTRAINT",
            "ACTIVE_CATALOG_SECTION",
            "execute the specified catalog test",
        ):
            self.assertIn(marker, actual)

    def test_09_direct_member_has_one_entry_and_can_be_retained(self):
        self.admin(
            "POST",
            "plugins/direct/versions",
            {
                "version": "1",
                "code": """return function(loom)
          loom.register_tool({id='record',exposure='direct',handler=function(ctx,args) return 'dispatched' end})
          loom.register_toolset({id='one',members={'record'}})
          loom.register_toolset({id='two',members={'record'}})
          end""",
            },
        )
        self.admin(
            "POST",
            "plugins",
            {"id": "direct", "package": "direct", "version": "1", "enabled": True},
        )
        binds = [
            {"tool": "direct.record"},
            {"toolset": "direct.one"},
            {"toolset": "direct.two"},
        ]
        self.agent("direct-agent", binds, {"tools": ["direct.record"]})
        directory = self.api("GET", "agents/direct-agent/tools")
        self.assertEqual(len(directory), 1)
        self.assertTrue(directory[0]["direct"])
        self.api("PATCH", "agents/direct-agent", {"tools": binds[1:]})
        remaining = self.api("GET", "agents/direct-agent/tools")
        self.assertEqual(
            [(d["id"], d["revision"]) for d in remaining],
            [(directory[0]["id"], directory[0]["revision"])],
        )
        sid = self.session("direct-agent")
        _, calls = self.run_tools(sid, [("direct_drecord", {})])
        self.assertTrue(
            any(
                c.get("result") == "dispatched" and c["entry"] == "direct"
                for c in calls
            )
        )
        _, calls = self.run_tools(sid, ["return tools.call('direct.record',{})"])
        self.assertFalse(any(c.get("result") == "dispatched" for c in calls))

    def test_10_bundled_policies_can_be_disabled(self):
        self.agent("officialtest")
        sid = self.session("officialtest")
        for instance in ["goal", "composer-suggestions"]:
            self.assertTrue(self.admin("GET", "plugins/" + instance)["enabled"])
            pkg = self.admin("GET", f"plugins/{instance}/versions/1.0.0")
            self.assertIn("register_", pkg["code"])
            self.assertTrue(
                (self.home / "packages" / instance / "1.0.0/plugin.lua").exists()
            )
        result, _ = self.run_tools(sid, [])
        query = {
            "after_reply_id": result["id"],
            "expected_version": self.api("GET", "sessions/" + sid)["version"],
        }
        entered, release = threading.Event(), threading.Event()
        self.peer.sample_gate = (entered, release)
        outcome = []

        def sample():
            try:
                outcome.append(
                    self.api("POST", "sessions/" + sid + "/suggestions", query)
                )
            except Exception as e:
                outcome.append(e)

        t = threading.Thread(target=sample)
        t.start()
        try:
            self.assertTrue(entered.wait(4))
            self.admin("PATCH", "plugins/composer-suggestions", {"enabled": False})
        finally:
            release.set()
            t.join(8)
            self.peer.sample_gate = None
        self.assertEqual(len(outcome), 1)
        self.assertIsInstance(outcome[0], dict, outcome)
        self.assertIsNone(outcome[0]["suggestion"])
        self.admin("PATCH", "plugins/goal", {"enabled": False})
        before = len(self.peer.inputs)
        eid = self.event("goal suppressed by deployment policy")
        # A disabled optional feature may explicitly refuse the opt-in at admission.
        try:
            d = self.api(
                "POST",
                f"events/{eid}/deliveries",
                {
                    "target": {"session_id": sid},
                    "action": "start_run",
                    "goal": {"objective": "keep making progress"},
                },
                expected=202,
            )
        except AssertionError as e:
            self.assertRegex(str(e), r"(400|403|409|422) != 202")
        else:
            for _ in range(100):
                run = self.api("GET", "runs/" + d["run_id"])
                if run["status"] in ("completed", "failed", "cancelled"):
                    break
                time.sleep(0.04)
            self.assertIn(run["status"], ("completed", "failed", "cancelled"))
            self.assertFalse(
                any(
                    c.get("reason") == "goal"
                    for c in self.api("GET", f"runs/{d['run_id']}/model-calls")
                )
            )
        self.assertLessEqual(len(self.peer.inputs) - before, 1)
        self.stop_service()
        self.start_service()
        for instance in ["goal", "composer-suggestions"]:
            self.assertFalse(self.admin("GET", "plugins/" + instance)["enabled"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
