"""API acceptance for context, Goal, suggestions and Notes with a local provider.

Run after cargo build: python tests/acceptance_context.py -v
"""
import base64
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from threading import Event, Thread
import unittest
from urllib.error import HTTPError
from uuid import uuid4

from harness import Instance, ROOT


def answer(text):
    return {"choices": [{"finish_reason": "stop", "message": {"role": "assistant", "content": text}}]}


def tool(name, args):
    call = {"id": "call_" + uuid4().hex, "type": "function",
            "function": {"name": name, "arguments": json.dumps(args)}}
    return {"choices": [{"finish_reason": "tool_calls",
                         "message": {"role": "assistant", "content": None, "tool_calls": [call]}}]}


class Provider:
    def __init__(self):
        self.calls = []
        self.errors = []
        self.respond = lambda payload: answer("fixture reply")
        provider = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                if self.path != "/v1/chat/completions":
                    self.send_error(404)
                    return
                payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                provider.calls.append(payload)
                try:
                    result = provider.respond(payload)
                    status = result if isinstance(result, int) else 200
                    body = json.dumps({} if isinstance(result, int) else result).encode()
                except Exception as exc:
                    provider.errors.append(exc)
                    body, status = b"{}", 500
                try:
                    self.send_response(status)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
                    pass

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = Thread(target=self.http.serve_forever, daemon=True)
        self.thread.start()

    def stop(self):
        self.http.shutdown()
        self.http.server_close()
        self.thread.join()


class ContextAcceptance(unittest.TestCase):
    def setUp(self):
        self.provider = Provider()
        self.addCleanup(self.provider.stop)
        port = self.provider.http.server_address[1]
        routes = {name: {"base_url": f"http://127.0.0.1:{port}/v1", "model": name,
                         "images": name != "text"} for name in ("primary", "backup", "text")}
        self.app = Instance({"LOOM_MODEL_ROUTES": json.dumps(routes), "LOOM_DEFAULT_MODEL": "primary",
                             "LOOM_NOTE_THRESHOLD": "1000", "LOOM_NOTE_READ_BUDGET": "6000"}, start=False)
        self.addCleanup(self.app.__exit__, None, None, None)
        local = self.app.home / "tools" / "ci"
        local.mkdir(parents=True)
        (local / "tool.yaml").write_text(json.dumps({
            "id": "ci", "handler": "handler.lua", "exposure": "direct",
            "input_schema": {"type": "object"}, "allowed_agents": ["agent"],
        }), encoding="utf8")
        (local / "handler.lua").write_text(
            'return function(ctx,args) return {status="accepted",job_id="T42"} end', encoding="utf8")
        self.app.start()
        self.app.api("POST", "agents", {"id": "agent", "model": {"default": "primary", "fallbacks": ["backup"]},
                                          "tools": [{"toolset": "loom.daily"}, {"tool": "home.ci"}]})
        self.sid = self.session()

    def tearDown(self):
        self.assertEqual(self.provider.errors, [])

    def session(self, agent="agent"):
        return self.app.api("POST", "sessions", {"agent_id": agent})["id"]

    def run_wait(self, text, *, sid=None, status="completed", **fields):
        rid = self.app.send(sid or self.sid, text, **fields)["run_id"]
        result = self.app.wait(rid, (status, "failed", "cancelled"), timeout=10)
        self.assertEqual(result["status"], status, result.get("error") or result.get("stop_reason"))
        return result

    def calls(self, run, kind="model"):
        return self.app.api("GET", f"runs/{run['id']}/{kind}-calls")

    def trace(self, case, **data):
        if os.getenv("LOOM_TEST_TRACE"):
            print(json.dumps({"case": case, **data}), flush=True)

    def history(self, sid=None):
        return self.app.api("GET", f"sessions/{sid or self.sid}/context/items")

    def sample(self, run, version=None, token="user-token"):
        if version is None:
            version = self.app.api("GET", f"sessions/{self.sid}")["version"]
        return self.app.api("POST", f"sessions/{self.sid}/suggestions",
                            {"expected_version": version, "after_reply_id": run["id"]}, token=token)

    def note_call(self, name, **arguments):
        code_args = ",".join(f'[{json.dumps(k)}]={json.dumps(v,ensure_ascii=False)}' for k, v in arguments.items())
        sequence = iter([tool("code_exec", {"code": f'return tools.call("{name}", {{{code_args}}})'}),
                         answer("note operation completed")])
        self.provider.respond = lambda payload: next(sequence)
        r = self.run_wait(name)
        c = next(c for c in self.calls(r, "tool") if c["tool_id"] == name)
        self.assertEqual(c["status"], "completed", c.get("error"))
        return c["result"]

    def start_ci(self, **fields):
        self.provider.respond = lambda payload: tool("home_dci", {"job_name": "发布验收"})
        r = self.run_wait("start T42", status="waiting", **fields)
        c = next(c for c in self.calls(r, "tool") if c["tool_id"] == "home.ci")
        self.assertEqual(c["job_id"], "T42")
        return r, c

    def receipt(self, call, value, key=None):
        return self.app.api("POST", f"tool-calls/{call['id']}/results",
                            {"job_id": "T42", "status": "completed", "result": value},
                            token="source-token", key=key or uuid4().hex)

    def test_c01_one_event_two_deliveries(self):
        event = self.app.api("POST", "events", {"content": "build QA-715 failed"})
        address = f"events/{event['id']}/deliveries"
        archive = self.app.api("POST", address, {"target": {"session_id": self.sid}, "action": "attach"})
        investigate = self.app.api("POST", address, {"target": {"agent_id": "agent"}, "action": "start_run"})
        self.assertNotEqual(archive["id"], investigate["id"])
        self.assertEqual(len(self.app.api("GET", address)), 2)
        self.assertEqual(self.history()[0]["event_id"], event["id"])
        self.assertEqual(self.app.api("GET", f"sessions/{self.sid}/runs"), [])
        self.assertEqual(self.app.wait(investigate["run_id"])["status"], "completed")

    def test_c02_context_hook_and_sections_refreshed_without_new_session(self):
        def admin(method, path, value):
            return self.app.api(method, path, value, token="admin-token")
        select = """return function(loom)
          loom.register_context(function(ctx, input)
            local out = {}
            for _,m in ipairs(input.messages) do
              if m.content == 'C-marker' then out[#out+1]=m end
            end
            out[#out+1]={role='user',content='A-marker-edited'}
            out[#out+1]=input.messages[#input.messages]
            return out
          end)
        end"""
        admin("POST", "plugins/selector/versions", {"version": "1", "code": select})
        admin("POST", "plugins", {"id": "selector", "package": "selector", "version": "1",
                                  "enabled": True, "allowed_agents": ["section-agent"]})
        for version in ("1", "2"):
            source = f"return function(loom) loom.register_section({{id='guide',text='guide-v{version}'}}) end"
            admin("POST", "plugins/guide/versions", {"version": version, "code": source})
        admin("POST", "plugins", {"id": "guide", "package": "guide", "version": "1",
                                  "enabled": True, "allowed_agents": ["section-agent"]})
        self.app.api("POST", "agents", {"id": "section-agent", "context_plugin": "selector"})
        s = self.session("section-agent")
        for text in ("A-marker", "B-marker", "C-marker"):
            self.app.send(s, text, action="attach")
        first = self.run_wait("inspect", sid=s)
        initial = self.calls(first)[0]["messages"]
        self.assertIn("guide-v1", initial[0]["content"])
        self.assertEqual([m["content"] for m in initial[1:]], ["C-marker", "A-marker-edited", "inspect"])
        admin("PATCH", "plugins/guide", {"version": "2"})
        second = self.calls(self.run_wait("inspect again", sid=s))[0]["messages"]
        self.assertIn("guide-v2", second[0]["content"])
        self.assertNotIn("guide-v1", second[0]["content"])
        admin("PATCH", "plugins/guide", {"enabled": False})
        third = self.calls(self.run_wait("inspect last", sid=s))[0]["messages"]
        self.assertNotIn("guide-v", third[0]["content"])
        self.assertEqual(self.history(s)[0]["content"], "A-marker")
        self.assertEqual(self.calls(first)[0]["messages"], initial)

    def test_c03_c08_compress_evidence_and_fail_when_required_input_cannot_fit(self):
        self.app.api("POST", "agents", {"id": "small", "context_bytes": 900, "tools": []})
        sid = self.session("small")
        for evidence in ("initial", "obsolete-" * 200, "recent"):
            self.app.send(sid, evidence, action="attach")
        self.provider.respond = lambda p: answer("summary-marker" if not p.get("tools")
            and "Summarize" in p["messages"][0]["content"] else "analysis")
        run = self.run_wait("analyze only", sid=sid)
        call = self.calls(run)[-1]
        self.assertTrue(call["compressed_ranges"])
        self.assertIn("summary-marker", json.dumps(call["messages"], ensure_ascii=False))
        self.assertIn("analyze only", str(call["messages"]))
        self.assertIn("obsolete-", self.history(sid)[1]["content"])
        self.assertEqual(len(self.calls(run, "tool")), 0)
        goal = {"objective": "required-" * 300, "completion_condition": "report"}
        failed = self.run_wait("oversize", sid=self.session("small"), goal=goal, status="failed")
        self.assertIn("budget", failed["error"])

    def test_c04_branch_cut_and_late_report(self):
        for text in ("E1", "E2", "secret-after-cut"):
            self.app.send(self.sid, text, action="attach")
        parent, call = self.start_ci()
        child = self.app.api("POST", f"sessions/{self.sid}/branches", {"at": 2})
        self.assertEqual(child["parent_session_id"], self.sid)
        self.assertEqual([m["content"] for m in self.history(child["id"])], ["E1", "E2"])
        self.provider.respond = lambda payload: answer("report received")
        self.receipt(call, "parent report")
        self.assertEqual(self.app.wait(parent["id"])["status"], "completed")
        result = self.run_wait("child query", sid=child["id"])
        child_input = str(self.calls(result)[0]["messages"])
        self.assertNotIn("secret-after-cut", child_input)
        self.assertNotIn("parent report", child_input)
        self.assertEqual(len(self.calls(result, "tool")), 0)

    def test_c05_c06_goal_continuation_budget_and_wait_cancel(self):
        goal = {"objective": "migrate twenty", "completion_condition": "twenty complete; tests pass"}
        steps = iter([answer("fourteen complete"), tool("goal_complete", {"evidence": "twenty complete; tests pass"}),
                      answer("final report")])
        self.provider.respond = lambda p: next(steps)
        r = self.run_wait("migrate", goal=goal)
        calls = self.calls(r)
        self.assertEqual([c["reason"] for c in calls], ["input", "goal", "tool_result"])
        self.assertEqual([sum(m.get("source") == "goal" for m in c["messages"]) for c in calls], [0, 1, 0])
        self.assertEqual([m["content"] for m in self.history() if m["role"] == "user"], ["migrate"])
        self.assertTrue(all(goal["completion_condition"] in str(c["messages"]) for c in calls))
        self.provider.respond = lambda p: answer("still working")
        stopped = self.run_wait("continue", goal={**goal, "max_rounds": 2}, status="failed")
        self.assertIn("budget", stopped["stop_reason"])
        self.assertEqual(len(self.calls(stopped)), 2)
        waiting, call = self.start_ci(goal=goal)
        self.app.send(self.sid, "cancel", action="request_cancel", run_id=waiting["id"])
        self.receipt(call, "late report")
        self.assertEqual(self.app.wait(waiting["id"])["status"], "cancelled")
        self.assertEqual(len(self.calls(waiting)), 1)

    def test_c05_worker_crash_retries_same_compacted_continuation(self):
        self.app.stop()
        self.app.env["LOOM_WORK_LEASE_SECONDS"] = "2"
        self.app.start()
        self.app.api("PATCH", "agents/agent", {"context_bytes": 9000})
        for text in ("first", "completed steps " * 1800, "latest"):
            self.app.send(self.sid, text, action="attach")
        entered, release = Event(), Event()
        wire_attempts = []
        summaries = []
        def crash_window(payload):
            messages = payload["messages"]
            if "Summarize" in messages[0]["content"]:
                marker = "summary-" + uuid4().hex
                summaries.append(marker)
                return answer(marker)
            if messages[-1]["role"] == "tool":
                return answer("all twenty completed; full test report passed")
            if messages[-1]["content"].startswith("继续"):
                wire_attempts.append(messages)
                if not entered.is_set():
                    entered.set()
                    if not release.wait(6):
                        raise TimeoutError("continuation fixture not released")
                return tool("goal_complete", {"evidence": "verified 20 and passing report"})
            return answer("fourteen completed")
        self.provider.respond = crash_window
        goal = {"objective": "migrate twenty", "completion_condition": "all twenty completed and passing report"}
        run_id = self.app.send(self.sid, "migration request", goal=goal)["run_id"]
        try:
            self.assertTrue(entered.wait(5))
            before = self.calls({"id": run_id})
            pending = next(c for c in before if c["reason"] == "goal")
            self.assertEqual(pending["status"], "pending")
            self.assertTrue(pending["compressed_ranges"])
            self.app.stop()
        finally:
            release.set()
        self.app.start()
        complete_run = self.app.wait(run_id, timeout=15)
        self.assertEqual(complete_run["status"], "completed", complete_run.get("error"))
        replayed = [c for c in self.calls(complete_run) if c["reason"] == "goal"]
        self.assertEqual([c["id"] for c in replayed], [pending["id"]])
        self.assertEqual(len(wire_attempts), 2)
        self.assertEqual(wire_attempts[0], wire_attempts[1])
        self.assertEqual(len([marker for marker in summaries if marker in str(wire_attempts[0])]), 1)
        self.assertEqual(sum(m.get("source") == "goal" for m in replayed[0]["messages"]), 1)
        self.assertEqual(self.history()[-1]["run_id"], run_id)
        self.assertEqual(len(self.app.api("GET", f"sessions/{self.sid}/runs")), 1)
        self.trace("C05", run=run_id, continuation=pending["id"], actual_attempts=len(wire_attempts))

    def test_c07_repeated_receipt_resumes_goal_once(self):
        g = {"objective": "analyze T42", "completion_condition": "analyze failure"}
        self.app.api("PATCH", "agents/agent", {"context_bytes": 9000})
        for text in ("first", "closed migration steps " * 1500, "last"):
            self.app.send(self.sid, text, action="attach")
        def issue(payload):
            if "Summarize" in payload["messages"][0]["content"]:
                return answer("completed migration steps")
            return tool("home_dci", {"job_name": "发布验收"})
        self.provider.respond = issue
        r = self.run_wait("start T42", status="waiting", goal=g)
        call = next(c for c in self.calls(r, "tool") if c["tool_id"] == "home.ci")
        self.assertTrue(self.calls(r)[0]["compressed_ranges"])
        seq = iter([tool("goal_complete", {"evidence": "read FAIL-42"}), answer("FAIL-42")])
        self.provider.respond = lambda p: (answer("completed migration steps")
            if "Summarize" in p["messages"][0]["content"] else next(seq))
        self.receipt(call, "FAIL-42")
        self.receipt(call, "FAIL-42")
        self.assertEqual(self.app.wait(r["id"])["status"], "completed")
        records = self.calls(r)
        resumed = records[1]
        self.assertEqual(resumed["reason"], "tool_result")
        self.assertTrue(resumed["compressed_ranges"])
        self.assertEqual(sum(m.get("source") == "goal" for m in resumed["messages"]), 0)
        self.assertIn("FAIL-42", str(resumed["messages"]))
        self.assertIn(g["completion_condition"], str(resumed["messages"]))
        self.assertEqual(len(records), 3)
        self.assertEqual(len([c for c in self.calls(r, "tool") if c["tool_id"] == "home.ci"]), 1)
        self.trace("C07", run=r["id"], receipt_call=call["id"], resumed_model_call=resumed["id"])

    def test_c08_change_or_cancel_during_compaction(self):
        self.app.api("POST", "agents", {"id": "compacting", "context_bytes": 1200, "tools": []})
        for action in ("add_input", "request_cancel"):
            s = self.session("compacting")
            for piece in ("first", "old material " * 200, "newest"):
                self.app.send(s, piece, action="attach")
            entered, release = Event(), Event()
            def reply(payload):
                first = payload["messages"][0]["content"]
                if "Summarize" in first:
                    if not entered.is_set():
                        entered.set()
                        if not release.wait(5):
                            raise TimeoutError("summary was not released")
                    return answer("old suggestion to rerun CI")
                if payload["messages"][-1]["role"] == "tool":
                    return answer("analysis complete")
                return tool("goal_complete", {"evidence": "analyzed without rerun"})
            self.provider.respond = reply
            g = {"objective": "analyze", "completion_condition": "deliver analysis"}
            rid = self.app.send(s, "inspect the report", goal=g)["run_id"]
            try:
                self.assertTrue(entered.wait(5))
                self.app.send(s, "analysis only; no rerun", action=action, run_id=rid)
            finally:
                release.set()
            result = self.app.wait(rid)
            calls = self.calls(result)
            if action == "add_input":
                self.assertEqual(result["status"], "completed", result.get("error") or result.get("stop_reason"))
                self.assertTrue(calls)
                self.assertIn("analysis only; no rerun", str(calls[0]["messages"]))
            else:
                self.assertEqual(result["status"], "cancelled")
                self.assertEqual(calls, [])
            self.assertEqual(self.calls(result, "tool"), [])

    def test_c09_c10_c11_suggestions_scope_stale_response_and_manual_send(self):
        event = self.app.api("POST", "events", {"content": "private-sample-marker", "visibility": "agent"})
        self.app.api("POST", f"events/{event['id']}/deliveries", {"action": "attach", "target": {"session_id": self.sid}})
        r = self.run_wait("committed locally")
        history = self.history()
        self.provider.respond = lambda p: answer("push it")
        self.assertEqual(self.sample(r)["suggestion"], "push it")
        self.assertEqual(self.history(), history)
        self.assertNotIn("private-sample-marker", str(self.provider.calls[-1]))
        self.assertNotIn("tools", self.provider.calls[-1])
        self.provider.respond = lambda p: tool("push", {})
        self.assertIsNone(self.sample(r)["suggestion"])
        self.assertEqual(self.calls(r, "tool"), [])
        entered, release = Event(), Event()
        def delay(p):
            if "Suggest one" in p["messages"][0]["content"]:
                entered.set()
                if not release.wait(5):
                    raise TimeoutError("suggestion fixture not released")
                return answer("push it")
            return answer("show diff")
        self.provider.respond = delay
        with ThreadPoolExecutor(max_workers=1) as executor:
            outstanding = executor.submit(self.sample, r)
            try:
                self.assertTrue(entered.wait(5))
                self.app.send(self.sid, "first show the diff")
            finally:
                release.set()
            self.assertIsNone(outstanding.result()["suggestion"])
        sent = [m["content"] for m in self.history() if m["role"] == "user"]
        self.assertIn("first show the diff", sent)
        self.assertNotIn("push it", sent)

    def test_c11_suggestion_permission_revocation_quota_timeout(self):
        def settings(user):
            self.app.api("PATCH", "settings/composer-suggestions",
                {"principals": {"qa": {"reviewer": {"read_private": True}, "qa": user}}}, token="admin-token")
        settings({"enabled": True, "max_samples_per_minute": 30})
        event = self.app.api("POST", "events", {"content": "reviewer-only-build", "visibility": "agent"})
        self.app.api("POST", f"events/{event['id']}/deliveries", {"action": "attach", "target": {"session_id": self.sid}})
        r = self.run_wait("committed")
        self.provider.respond = lambda p: answer("reviewer-only-candidate" if "reviewer-only-build" in str(p) else "public candidate")
        self.assertEqual(self.sample(r, token="inspector-token")["suggestion"], "reviewer-only-candidate")
        self.assertEqual(self.sample(r)["suggestion"], "public candidate")
        entered, release = Event(), Event()
        def delayed(p):
            entered.set()
            release.wait(4)
            return answer("stale candidate")
        self.provider.respond = delayed
        with ThreadPoolExecutor(max_workers=1) as executor:
            outstanding = executor.submit(self.sample, r)
            try:
                self.assertTrue(entered.wait(4))
                settings({"enabled": False})
            finally:
                release.set()
            self.assertIsNone(outstanding.result()["suggestion"])
        settings({"enabled": True, "max_samples_per_minute": 0})
        count = len(self.provider.calls)
        self.assertIsNone(self.sample(r)["suggestion"])
        self.assertEqual(len(self.provider.calls), count)
        settings({"enabled": True, "max_samples_per_minute": 30, "timeout_ms": 100})
        entered.clear()
        release.clear()
        try:
            self.assertIsNone(self.sample(r)["suggestion"])
            self.assertTrue(entered.is_set())
        finally:
            release.set()
        self.assertEqual(self.app.api("GET", f"runs/{r['id']}")["output"], r["output"])
        self.assertEqual(self.calls(r, "tool"), [])

    def test_c12_note_metadata_search_and_line_reads(self):
        first = self.note_call("write_note", description="Release.[one]", body="E421")
        second = self.note_call("write_note", description="Release.[two]", body="line1\nline2")
        listed = self.note_call("search_notes", keyword="Release.[", limit=1)
        self.assertEqual(listed["items"][0]["note_ref"], second["note_ref"])
        self.assertNotIn("body", listed["items"][0])
        following = self.note_call("search_notes", keyword="Release.[", cursor=listed["next_cursor"], limit=1)
        self.assertEqual(following["items"][0]["note_ref"], first["note_ref"])
        self.assertEqual(self.note_call("search_notes", keyword="E421")["items"], [])
        self.assertEqual(self.note_call("read_note", note_ref=second["note_ref"], start_line=2)["body"], "line2")

    def test_c13_log_from_history_event_and_restarted_worker(self):
        run, call = self.start_ci()
        lines = ["E421" if i == 280 else f"log-{i:03d}" for i in range(1, 301)]
        log = "\n".join(lines)
        self.provider.respond = lambda p: answer("received")
        self.receipt(call, log, "same-report")
        self.receipt(call, log, "same-report")
        self.assertEqual(self.app.wait(run["id"])["status"], "completed")
        record = next(m for m in self.history() if m.get("note_ref"))
        ref = record["note_ref"]
        event = self.app.api("GET", "events/" + record["event_id"])
        self.assertIn(ref, json.dumps(event))
        self.app.stop()
        self.app.start()
        meta = self.note_call("search_notes", keyword="发布验收")["items"]
        self.assertEqual([m["note_ref"] for m in meta], [ref])
        self.assertEqual(self.note_call("read_note", note_ref=ref)["body"], log)
        self.assertEqual(self.note_call("read_note", note_ref=ref, start_line=280, end_line=280)["body"], "E421\n")
        self.assertEqual(self.note_call("read_note", note_ref=ref, end_line=2)["body"], "log-001\nlog-002\n")
        self.app.stop()
        self.app.env["LOOM_NOTE_READ_BUDGET"] = "32"
        self.app.start()
        large = self.note_call("read_note", note_ref=ref)
        self.assertTrue(large["over_budget"])
        self.assertEqual(large["line_count"], 300)
        self.assertEqual(self.note_call("search_notes")["items"][0]["note_ref"], ref)
        self.trace("C13", run=run["id"], call=call["id"], result_event=record["event_id"], note=ref)

    def test_c13_mixed_result_parts_keep_origin_type_order_and_bytes(self):
        r, c = self.start_ci()
        self.provider.respond = lambda p: answer("received mixed result")
        first, third = "alpha-" * 400, "omega-" * 300
        image = {"type": "image", "mimeType": "image/png", "data": "test-opaque-binary-block"}
        parts = {"content": [{"type": "text", "text": first}, image, {"type": "text", "text": third}]}
        self.receipt(c, parts, "mixed-original")
        self.receipt(c, parts, "mixed-original")
        self.assertEqual(self.app.wait(r["id"], timeout=12)["status"], "completed")
        result = self.app.api("GET", f"tool-calls/{c['id']}")["result"]["content"]
        self.assertEqual([b["type"] for b in result], ["text", "image", "text"])
        self.assertEqual(result[1], image)
        refs = [result[i]["note_ref"] for i in (0, 2)]
        self.assertNotEqual(refs[0], refs[1])
        stored = next(m for m in self.history() if m.get("tool_call_id") == c["id"] and m["status"] == "completed")
        self.assertEqual(stored["content"]["content"], result)
        self.assertIn(refs[0], json.dumps(self.app.api("GET", f"events/{stored['event_id']}")))
        for index, expected in [(0, first), (1, image), (2, third)]:
            found = self.app.api("GET", f"tool-calls/{c['id']}/results/{index}/content")
            self.assertEqual(found if index == 1 else found["body"], expected)
        with self.assertRaises(HTTPError) as denied:
            self.app.api("GET", f"tool-calls/{c['id']}/results/0/content", token="other-token")
        self.assertEqual(denied.exception.code, 404)
        denied.exception.close()

    def test_c14_c15_image_provider_fallback_schema_and_errors(self):
        image = (ROOT / "docs/acceptance/fixtures/report.png").read_bytes()
        aid = self.app.api("POST", "attachments", {
            "mime_type": "image/png", "data": base64.b64encode(image).decode()})["id"]
        content = [{"type": "text", "text": "before"}, {"type": "attachment", "attachment_id": aid},
                   {"type": "text", "text": "after"}]
        seen = []
        def with_fallback(payload):
            previous = any(m["role"] == "tool" for m in payload["messages"])
            if payload["model"] == "primary":
                if previous:
                    return 503
                return tool("code_exec", {"code": "return {'linux'}"})
            parts = next(m["content"] for m in payload["messages"] if isinstance(m["content"], list))
            seen.append((parts[0]["text"], base64.b64decode(parts[1]["image_url"]["url"].split(",")[1]), parts[2]["text"]))
            return answer('{"case":"checkout_retry","error":"E421"}')
        self.provider.respond = with_fallback
        schema = {"type": "object", "required": ["case", "error"],
                  "properties": {"case": {"type": "string"}, "error": {"type": "string"}}, "additionalProperties": False}
        r = self.run_wait(content, result_schema=schema)
        self.assertEqual(seen, [("before", image, "after")])
        self.assertEqual(r["output"], {"case": "checkout_retry", "error": "E421"})
        self.assertEqual([c["status"] for c in self.calls(r)], ["completed", "failed", "completed"])
        tools = self.calls(r, "tool")
        self.assertEqual(len(tools), 1)
        self.assertEqual(tools[0]["status"], "completed")
        self.assertIn("linux", str(tools[0]["result"]))
        self.app.api("POST", "agents", {"id": "textonly", "model": {"default": "text"}})
        failed = self.run_wait(content, sid=self.session("textonly"), status="failed")
        self.assertIn("compatible", failed["error"])
        with self.assertRaises(HTTPError) as denied:
            self.app.api("GET", f"attachments/{aid}/content", token="other-token")
        self.assertEqual(denied.exception.code, 404)
        denied.exception.close()
        self.app.api("DELETE", f"attachments/{aid}")
        unreadable = self.run_wait(content, sid=self.session(), status="failed")
        self.assertIn("attachment", unreadable["error"])
        self.provider.respond = lambda p: answer('{"case":"checkout_retry"}')
        wrong = self.run_wait("wrong schema", sid=self.session(), result_schema=schema, status="failed")
        self.assertIn("schema", wrong["error"])
        self.trace("C14", run=r["id"], attachment=aid, calls=[c["id"] for c in self.calls(r)])


if __name__ == "__main__":
    unittest.main()
