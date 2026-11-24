"""Exercise ordinary user flows via a live Loom HTTP server."""

import json
import os
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from harness import Instance, ROOT


class ReportEndpoint(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        method = req["method"]
        if method == "notifications/initialized":
            self.send_response(202)
            self.end_headers()
            return
        if method == "initialize":
            result = {
                "protocolVersion": req["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "release-ci", "version": "1"},
            }
        elif method == "tools/list":
            result = {
                "tools": [
                    {
                        "name": "get_report",
                        "description": "Get the CI report for a build by job ID.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"job": {"type": "string"}},
                            "required": ["job"],
                        },
                    }
                ]
            }
        elif method == "tools/call":
            job = req["params"]["arguments"].get("job")
            self.server.jobs.append(job)
            found = req["params"]["name"] == "get_report" and job == "T42"
            result = {
                "isError": not found,
                "content": [
                    {
                        "type": "text",
                        "text": self.server.report if found else "Unknown CI job",
                    }
                ],
            }
        else:
            self.send_error(400)
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(
            json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}).encode()
        )


class Conversation(unittest.TestCase):
    @unittest.skipUnless(
        os.getenv("DEEPSEEK_API_KEY"), "live DeepSeek credential required"
    )
    def test_a01_live_model_and_reconnect(self):
        with Instance() as app:
            templates = app.api("GET", "/api/agent-templates")
            template = next(t for t in templates if t["id"] == "standard")
            model = app.api("GET", "/api/models")[0]["id"]
            agent = app.api(
                "POST",
                "/api/agents",
                {
                    "id": "assistant",
                    "model": {"default": model},
                    "template": {k: template[k] for k in ("id", "source", "version")},
                },
            )
            sid = app.api("POST", "/api/sessions", {"agent_id": agent["id"]})["id"]
            delivery = app.send(sid, "本次登记号是 QA-715。请只回复登记号。")
            run = app.wait(delivery["run_id"])
            self.assertEqual(run["status"], "completed", run.get("error"))
            self.assertIn("QA-715", run["output"])
            calls = app.api("GET", f"/api/runs/{run['id']}/model-calls")
            self.assertEqual(len(calls), 1)
            self.assertEqual(app.api("GET", f"/api/runs/{run['id']}/tool-calls"), [])
            self.assertEqual(calls[-1]["provider_model"], "deepseek-flash")
            app.stop()
            app.start()
            self.assertIn(sid, [s["id"] for s in app.api("GET", "sessions")])
            self.assertEqual(
                [r["id"] for r in app.api("GET", f"/api/sessions/{sid}/runs")],
                [run["id"]],
            )
            history = app.api("GET", f"/api/sessions/{sid}/context/items")
            self.assertEqual(history[-1]["content"], run["output"])
            self.assertEqual(
                app.api("GET", f"/api/runs/{run['id']}/model-calls"), calls
            )
            with self.assertRaises(HTTPError) as denied:
                app.api("GET", f"/api/runs/{run['id']}", token="other-token")
            self.assertEqual(denied.exception.code, 404)
            denied.exception.close()
            print(
                json.dumps(
                    {
                        "case": "A01",
                        "run": run["id"],
                        "model": model,
                        "reply": run["output"],
                        "model_calls": len(calls),
                        "tool_calls": len(
                            app.api("GET", f"/api/runs/{run['id']}/tool-calls")
                        ),
                    }
                )
            )

    @unittest.skipUnless(
        os.getenv("DEEPSEEK_API_KEY"), "live DeepSeek credential required"
    )
    def test_a02_live_model_uses_mcp_and_recalls_original_note(self):
        peer = ThreadingHTTPServer(("127.0.0.1", 0), ReportEndpoint)
        self.addCleanup(peer.server_close)
        self.addCleanup(peer.shutdown)
        threading.Thread(target=peer.serve_forever, daemon=True).start()
        peer.jobs = []
        peer.report = (ROOT / "docs/acceptance/fixtures/t42.txt").read_text()
        with Instance(env={"LOOM_NOTE_THRESHOLD": "900"}) as app:

            def admin(path, body):
                return app.api("POST", "/api/" + path, body, token="admin-token")

            admin(
                "plugins/mcp/versions",
                {"version": "1", "code": "return function(loom) end"},
            )
            admin(
                "plugins",
                {"id": "remote", "package": "mcp", "version": "1", "enabled": True},
            )
            admin(
                "mcp/connections",
                {
                    "id": "ci",
                    "instance": "remote",
                    "url": f"http://127.0.0.1:{peer.server_port}/ci",
                },
            )
            admin("mcp/connections/remote/ci/refresh", {})
            tid = "remote.ci.get_report"
            admin(
                "agents",
                {
                    "id": "reporter",
                    "model": "deepseek-flash",
                    "tools": [{"toolset": "loom.daily"}, {"tool": tid}],
                    "grants": {"tools": [tid]},
                    "system_prompt": "使用真实工具查询报告。工具产生 Note 时可用 read_note 读取。用 Lua 在本地处理返回正文，只返回该问题需要的行。沿用已读证据。",
                },
            )
            sid = app.api("POST", "/api/sessions", {"agent_id": "reporter"})["id"]
            first = app.wait(
                app.send(sid, "查询 T42 的失败报告，只告诉我错误码。")["run_id"]
            )
            self.assertEqual(first["status"], "completed", first.get("error"))
            self.assertIn("E421", first["output"])
            calls = app.api("GET", f"/api/runs/{first['id']}/tool-calls")
            report = next(c for c in calls if c["tool_id"] == tid)
            ref = report["result"]["note_ref"]
            app.stop()
            app.start()
            second = app.wait(
                app.send(sid, "刚才那个错误码在第几行？沿用原报告，不要再查询 CI。")[
                    "run_id"
                ]
            )
            self.assertEqual(second["status"], "completed", second.get("error"))
            self.assertIn("280", second["output"])
            self.assertNotEqual(first["id"], second["id"])
            self.assertEqual(peer.jobs, ["T42"])
            history = app.api("GET", f"/api/sessions/{sid}/context/items")
            self.assertIn(ref, json.dumps(history))
            events = app.api("GET", "/api/events")
            event = next(
                e
                for e in events
                if e.get("tool_call_id") == report["id"]
                and e["type"] == "tool.result_received"
            )
            original = app.api("GET", f"events/{event['id']}")
            entry = next(
                m
                for m in history
                if m.get("tool_call_id") == report["id"] and m.get("note_ref")
            )
            self.assertEqual(original["result"]["note_ref"], entry["note_ref"])
            self.assertEqual(entry["note_ref"], ref)
            saved = app.api("GET", f"tool-calls/{report['id']}/results/0/content")
            self.assertEqual(saved["body"], peer.report)
            self.assertIn("E421", saved["body"].splitlines()[279])
            self.assertEqual(
                app.api("GET", f"runs/{first['id']}")["status"], "completed"
            )
            followup = app.api("GET", f"runs/{second['id']}/model-calls")
            self.assertIn(ref, json.dumps(followup[0]["messages"]))
            print(
                json.dumps(
                    {
                        "case": "A02",
                        "first": first["id"],
                        "followup": second["id"],
                        "ci_calls": 1,
                        "line": 280,
                        "note_ref": ref,
                    }
                )
            )


class ControlledConversation(unittest.TestCase):
    def setUp(self):
        from acceptance_context import Provider

        self.provider = Provider()
        self.addCleanup(self.provider.stop)
        url = f"http://127.0.0.1:{self.provider.http.server_port}/v1"
        self.app = Instance(
            env={
                "LOOM_MODEL_ROUTES": json.dumps(
                    {"test": {"base_url": url, "model": "qa"}}
                ),
                "LOOM_MAX_ROUNDS": "3",
            },
            start=False,
        )
        self.addCleanup(self.app.__exit__, None, None, None)
        ci = self.app.home / "tools" / "ci"
        ci.mkdir(parents=True)
        (ci / "tool.yaml").write_text(
            json.dumps(
                {
                    "id": "ci",
                    "handler": "handler.lua",
                    "allowed_agents": ["a"],
                    "input_schema": {
                        "type": "object",
                        "required": ["job"],
                        "properties": {"job": {"type": "string"}},
                    },
                }
            )
        )
        (ci / "handler.lua").write_text(
            'return function(ctx,args) return {status="accepted",job_id=args.job} end'
        )
        self.app.start()
        self.app.api(
            "POST",
            "agents",
            {"id": "a", "tools": [{"toolset": "loom.daily"}, {"tool": "home.ci"}]},
        )
        self.sid = self.app.api("POST", "sessions", {"agent_id": "a"})["id"]

    def calls(self, r, kind="tool"):
        return self.app.api("GET", f"runs/{r}/{kind}-calls")

    def send(self, msg, **kwargs):
        return self.app.send(self.sid, msg, **kwargs)["run_id"]

    def finish_ci(self, c, body):
        return self.app.api(
            "POST",
            f"tool-calls/{c['id']}/results",
            body,
            token="source-token",
            key="final-" + c["id"],
        )

    def test_a03_two_independent_runs_steer_and_cancel(self):
        from acceptance_context import answer, tool

        def respond(payload):
            last = payload["messages"][-1]
            if any(m["role"] == "tool" for m in payload["messages"]):
                self.assertIn("只分析", str(payload["messages"]))
                return answer("Windows failure W7; analysis only")
            job = "windows" if "Windows" in last["content"] else "linux"
            return tool(
                "code_exec", {"code": f"return tools.call('home.ci', {{job='{job}'}})"}
            )

        self.provider.respond = respond
        rw = self.send("查 Windows")
        self.app.wait(rw, ["waiting"])
        rl = self.send("查 Linux")
        self.app.wait(rl, ["waiting"])
        cw = next(c for c in self.calls(rw) if c["tool_id"] == "home.ci")
        cl = next(c for c in self.calls(rl) if c["tool_id"] == "home.ci")
        self.app.send(self.sid, "只分析，先别重跑", action="add_input", run_id=rw)
        alien = self.app.api(
            "POST", "events", {"content": "cancel"}, token="other-token"
        )
        with self.assertRaises(HTTPError) as denied:
            self.app.api(
                "POST",
                f"events/{alien['id']}/deliveries",
                {
                    "target": {"session_id": self.sid},
                    "run_id": rl,
                    "action": "request_cancel",
                },
                token="other-token",
            )
        denied.exception.close()
        self.assertEqual(self.app.api("GET", f"runs/{rl}")["status"], "waiting")
        self.app.send(self.sid, "停止 Linux", action="request_cancel", run_id=rl)
        self.finish_ci(cl, {"result": "late-linux"})
        self.finish_ci(cw, {"result": "W7"})
        self.assertIn("W7", self.app.wait(rw)["output"])
        self.assertEqual(self.app.wait(rl)["status"], "cancelled")
        self.assertEqual(len(self.app.api("GET", f"sessions/{self.sid}/runs")), 2)
        self.assertEqual([c["tool_id"] for c in self.calls(rw)].count("home.ci"), 1)

    def test_a04_deployment_default_and_missing_model(self):
        from acceptance_context import answer

        self.provider.respond = lambda _: answer("READY")
        self.assertEqual(self.app.wait(self.send("Reply READY"))["output"], "READY")
        self.app.stop()
        self.app.env.pop("DEEPSEEK_API_KEY", None)
        self.app.env.pop("LOOM_DEFAULT_MODEL", None)
        self.app.env["LOOM_MODEL_ROUTES"] = "{}"
        self.app.start()
        failed = self.app.wait(self.send("Reply READY"))
        self.assertEqual(failed["status"], "failed")
        self.assertIn("model", failed["error"])

    def test_a05_reply_recovers_only_the_selected_question(self):
        from acceptance_context import answer, tool

        def ask_then_query(p):
            prior = [m for m in p["messages"] if m["role"] == "tool"]
            if len(prior) >= 2:
                return answer("T42 result: ready")
            if prior:
                return tool(
                    "code_exec", {"code": "return tools.call('home.ci',{job='T42'})"}
                )
            return tool("ask_uuser", {"question": "Which build?"})

        self.provider.respond = ask_then_query
        a = self.send("First report")
        self.app.wait(a, ["waiting"])
        b = self.send("Second report")
        self.app.wait(b, ["waiting"])
        question = self.calls(a)[0]
        self.assertEqual(question["question"], "Which build?")
        self.app.stop()
        self.app.start()
        event = self.app.api("POST", "events", {"content": "T42"})
        update = {
            "target": {"session_id": self.sid},
            "action": "add_input",
            "run_id": a,
            "question_id": question["id"],
        }
        path = f"events/{event['id']}/deliveries"
        first = self.app.api("POST", path, update, key="one-answer")
        self.assertEqual(
            self.app.api("POST", path, update, key="one-answer")["id"], first["id"]
        )
        self.app.wait(a, ["waiting"])
        ci = next(c for c in self.calls(a) if c["tool_id"] == "home.ci")
        self.assertEqual(ci["job_id"], "T42")
        self.finish_ci(ci, {"result": "ready"})
        self.assertEqual(self.app.wait(a)["output"], "T42 result: ready")
        self.assertEqual(self.app.api("GET", f"runs/{b}")["status"], "waiting")
        self.assertEqual(len(self.calls(a, "model")), 3)
        self.assertEqual([c["tool_id"] for c in self.calls(a)].count("home.ci"), 1)

    def test_a06_repeated_read_only_call_hits_total_budget(self):
        from acceptance_context import tool

        self.provider.respond = lambda _: tool(
            "code_exec", {"code": "return tools.call('home.ci', {job='read-only'})"}
        )
        r = self.send("Repeat inspection")
        processed = set()
        for index in range(3):
            import time

            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                calls = [
                    c
                    for c in self.calls(r)
                    if c["tool_id"] == "home.ci"
                    and c["status"] == "accepted"
                    and c["id"] not in processed
                ]
                if calls:
                    break
                time.sleep(0.05)
            self.assertTrue(calls)
            call = calls[0]
            processed.add(call["id"])
            if index == 1:
                self.app.stop()
                self.app.start()
            for _ in range(2):
                self.finish_ci(call, {"result": "report received"})
        result = self.app.wait(r)
        self.assertEqual(result["status"], "failed")
        self.assertIn("budget", result["stop_reason"])
        self.assertIn("report received", str(result["partial_result"]))
        self.assertEqual(len(self.calls(r, "model")), 3)
        self.app.stop()
        self.app.start()
        self.assertEqual(len(self.calls(r, "model")), 3)

    def test_a07_parallel_reports_success_and_partial_failure(self):
        from acceptance_context import answer, tool

        for failure in (False, True):
            self.sid = self.app.api("POST", "sessions", {"agent_id": "a"})["id"]

            def respond(payload):
                msg = payload["messages"][-1]
                if msg["role"] == "tool":
                    return answer(msg["content"])
                return tool(
                    "code_exec",
                    {
                        "code": "return tools.parallel({{id='home.ci',args={job='X'}},{id='home.ci',args={job='Y'}}})"
                    },
                )

            self.provider.respond = respond
            r = self.send("Get both reports")
            self.app.wait(r, ["waiting"])
            childs = {
                c["arguments"]["job"]: c
                for c in self.calls(r)
                if c["tool_id"] == "home.ci"
            }
            self.assertEqual(set(childs), {"X", "Y"})
            self.assertEqual(childs["Y"]["parent_id"], childs["X"]["parent_id"])
            self.finish_ci(childs["Y"], {"result": "Y:ok"})
            self.assertEqual(self.app.api("GET", f"runs/{r}")["status"], "waiting")
            self.finish_ci(
                childs["X"],
                {"status": "failed", "error": "X:error"}
                if failure
                else {"result": "X:ok"},
            )
            output = self.app.wait(r)["output"]
            self.assertIn("Y:ok", output)
            self.assertIn("X:error" if failure else "X:ok", output)


if __name__ == "__main__":
    unittest.main()
