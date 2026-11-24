"""Exercise dispatch and recovery through a real Loom endpoint and observable HTTP recipients."""

from __future__ import annotations

import json
import math
import os
import socket
import subprocess
import threading
import time
import unittest
import uuid
from collections import Counter
from contextlib import closing, contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError

from harness import ROOT, Instance


def eventually(read, test, timeout=12):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = read()
        if test(last):
            return last
        time.sleep(0.05)
    raise AssertionError(f"expected condition not reached, last observation: {last!r}")


def trace(case, **fields):
    print(json.dumps({"case": case, **fields}, ensure_ascii=True), flush=True)


class Peers:
    def __init__(self):
        self.models = []
        self.notifications = {}
        self.requests = Counter()
        self.queries = Counter()
        self.drop = set()
        self.hold_ack = False
        self.receiver_saved = threading.Event()
        self.release_ack = threading.Event()
        self.entered = threading.Event()
        self.release = threading.Event()
        self.lock = threading.Lock()
        self.submissions = []
        self.ci_started = threading.Event()
        self.ci_release = threading.Event()
        self.hold_submission = False
        self.model_delay = 0
        peers = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def reply(self, value, status=200):
                buf = json.dumps(value).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(buf)))
                self.end_headers()
                self.wfile.write(buf)

            def do_POST(self):
                data = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if self.path.endswith("/chat/completions"):
                    with peers.lock:
                        peers.models.append(data)
                    if "HOLD-MODEL" in json.dumps(data["messages"]):
                        peers.entered.set()
                        peers.release.wait(8)
                    if peers.model_delay:
                        time.sleep(peers.model_delay)
                    steps = data["messages"]
                    if any(
                        m.get("role") == "user" and "SUBMIT-CI" in str(m.get("content"))
                        for m in steps
                    ) and not any(m.get("role") == "tool" for m in steps):
                        parallel = any(
                            "TWO-REPORTS" in str(m.get("content")) for m in steps
                        )
                        code = "return tools.call('testing.ci.submit', {})"
                        if parallel:
                            code = "return tools.parallel({{id='testing.ci.submit',args={}},{id='testing.ci.config',args={}}})"
                        message = {
                            "role": "assistant",
                            "content": None,
                            "tool_calls": [
                                {
                                    "type": "function",
                                    "id": uuid.uuid4().hex,
                                    "function": {
                                        "name": "code_exec",
                                        "arguments": json.dumps({"code": code}),
                                    },
                                }
                            ],
                        }
                        reason = "tool_calls"
                    else:
                        message = {"role": "assistant", "content": "CI verified E421"}
                        if any("R11-OUTCOME" in str(m.get("content")) for m in steps):
                            failed = any(
                                m.get("role") == "tool"
                                and "E421" in str(m.get("content"))
                                for m in steps
                            )
                            message["content"] = json.dumps(
                                {
                                    "status": "failed" if failed else "passed",
                                    "error": "E421" if failed else "",
                                }
                            )
                        reason = "stop"
                    self.reply(
                        {
                            "id": "fixture",
                            "model": "fixture",
                            "choices": [
                                {
                                    "index": 0,
                                    "finish_reason": reason,
                                    "message": message,
                                }
                            ],
                        }
                    )
                elif self.path == "/mcp":
                    method = data["method"]
                    if method == "initialize":
                        out = {
                            "protocolVersion": data["params"]["protocolVersion"],
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "test-ci", "version": "1"},
                        }
                    elif method == "notifications/initialized":
                        self.reply({}, 202)
                        return
                    elif method == "tools/list":
                        out = {
                            "tools": [
                                {"name": n, "inputSchema": {"type": "object"}}
                                for n in ["submit", "config"]
                            ]
                        }
                    elif method == "tools/call":
                        if data["params"]["name"] == "config":
                            out = {
                                "content": [
                                    {"type": "text", "text": "configuration:release"}
                                ]
                            }
                        else:
                            with peers.lock:
                                peers.submissions.append(data["params"])
                                job = f"T{41 + len(peers.submissions)}"
                            peers.ci_started.set()
                            if peers.hold_submission:
                                peers.ci_release.wait(5)
                            out = {
                                "structuredContent": {
                                    "status": "accepted",
                                    "job_id": job,
                                },
                                "content": [
                                    {"type": "text", "text": "accepted " + job}
                                ],
                            }
                    else:
                        self.reply({}, 400)
                        return
                    self.reply({"jsonrpc": "2.0", "id": data["id"], "result": out})
                elif self.path.startswith("/notify/"):
                    key = self.headers["Idempotency-Key"]
                    if data["delivery_id"] != key:
                        self.reply({"error": "wrong delivery identity"}, 400)
                        return
                    with peers.lock:
                        peers.requests[self.path] += 1
                        peers.notifications.setdefault(
                            (self.path, key), data["content"]
                        )
                        lose_confirmation = self.path in peers.drop
                        peers.drop.discard(self.path)
                    if lose_confirmation:
                        if peers.hold_ack:
                            peers.receiver_saved.set()
                            peers.release_ack.wait(8)
                        self.connection.shutdown(socket.SHUT_RDWR)
                        self.connection.close()
                    else:
                        self.reply({"confirmed": True})
                else:
                    self.reply({}, 404)

            def do_GET(self):
                from urllib.parse import parse_qs, urlsplit

                path = urlsplit(self.path)
                if path.path.startswith("/ci/jobs/"):
                    job = path.path.rsplit("/", 1)[-1]
                    with peers.lock:
                        peers.queries[path.path] += 1
                        valid_job = job in [
                            f"T{i + 42}" for i in range(len(peers.submissions))
                        ]
                    self.reply(
                        {
                            "job_id": job,
                            "status": "failed",
                            "error": "E421",
                            "result": {"status": "failed", "error": "E421"},
                        },
                        200 if valid_job else 404,
                    )
                    return
                key = parse_qs(path.query).get("delivery_id", [""])[0]
                with peers.lock:
                    peers.queries[path.path] += 1
                    present = (path.path, key) in peers.notifications
                self.reply({"confirmed": present}, 200 if present else 404)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.release.set()
        self.ci_release.set()
        self.release_ack.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()


@contextmanager
def deployment(peers, **options):
    config = {
        "LOOM_MODEL_ROUTES": json.dumps(
            {"fixture": {"base_url": peers.url + "/v1", "model": "fixture"}}
        ),
        "LOOM_DEFAULT_MODEL": "fixture",
        "LOOM_NOTIFICATION_TARGETS": json.dumps(
            [
                peers.url + "/notify/" + cap
                for cap in ("query_by_key", "idempotent", "unknown")
            ]
        ),
    }
    config.update({k: str(v) for k, v in options.items()})
    with Instance(env=config) as app:
        ident = "review-" + uuid.uuid4().hex[:10]
        agent = app.api(
            "POST", "agents", {"id": ident, "template": "standard", "model": "fixture"}
        )
        sid = app.api("POST", "sessions", {"agent_id": agent["id"]})["id"]
        yield app, agent["id"], sid


@contextmanager
def worker_processes(app, count):
    exe = os.getenv("LOOM_TEST_BIN") or ROOT / "target" / "debug" / (
        "loom-harness.exe" if os.name == "nt" else "loom-harness"
    )
    children = []
    try:
        for i in range(count):
            with (app.home / f"worker-{i}.log").open("ab") as log:
                children.append(
                    subprocess.Popen(
                        [str(exe), "--role", "worker"],
                        env={**app.env, "LOOM_WORKERS": "1"},
                        stdout=log,
                        stderr=log,
                        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
                    )
                )
        yield children
        assert all(child.poll() is None for child in children), "worker exited early"
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
        for child in children:
            child.wait(timeout=10)


def ci_agent(app, peers, approval=False):
    def admin(method, path, body):
        return app.api(method, path, body, token="admin-token")

    admin(
        "POST",
        "plugins/mcp/versions",
        {"version": "1", "code": "return function(loom) end"},
    )
    admin(
        "POST",
        "plugins",
        {"id": "testing", "package": "mcp", "version": "1", "enabled": True},
    )
    names = ["testing.ci.submit", "testing.ci.config"]
    admin(
        "POST",
        "mcp/connections",
        {
            "id": "ci",
            "instance": "testing",
            "url": peers.url + "/mcp",
            "imports": [
                {"name": "submit", "requires_approval": approval},
                {"name": "config"},
            ],
        },
    )
    admin("POST", "mcp/connections/testing/ci/refresh", {})
    actor = admin(
        "POST",
        "agents",
        {
            "id": "ci-reader",
            "owner": "qa",
            "model": "fixture",
            "tools": [{"tool": name} for name in names],
            "grants": {"tools": names, "plugins": ["testing"]},
        },
    )
    return actor["id"]


def start_ci(app, actor, text="SUBMIT-CI"):
    sid = app.api("POST", "sessions", {"agent_id": actor})["id"]
    rid = app.send(sid, text)["run_id"]
    return sid, rid


def job_call(app, rid):
    return eventually(
        lambda: app.api("GET", f"runs/{rid}/tool-calls"),
        lambda rows: any(row.get("tool_id") == "testing.ci.submit" for row in rows),
    )


def report(app, cid, key="report"):
    return app.api(
        "POST",
        f"tool-calls/{cid}/results",
        {
            "status": "failed",
            "error": "E421",
            "result": {"status": "failed", "error": "E421"},
        },
        token="source-token",
        key=key,
    )


class DispatchAcceptance(unittest.TestCase):
    def test_r05_report_precedes_wait_and_repeated_callback_after_restart(self):
        for early in (True, False):
            with (
                self.subTest(early=early),
                Peers() as peers,
                deployment(peers) as (app, _, _),
            ):
                actor = ci_agent(app, peers)
                peers.hold_submission = early
                _, run = start_ci(app, actor, "SUBMIT-CI TWO-REPORTS")
                self.assertTrue(peers.ci_started.wait(8))
                cid = next(
                    c["id"]
                    for c in job_call(app, run)
                    if c["tool_id"] == "testing.ci.submit"
                )
                if not early:
                    app.wait(run, ("waiting",))
                    app.stop()
                    app.start()
                    # The external CI status adapter and the direct callback report the same
                    # job through the authorized source endpoint, with distinct receipt keys.
                    from concurrent.futures import ThreadPoolExecutor
                    from urllib.request import urlopen

                    gate = threading.Barrier(2)

                    def poll_adapter(gate, cid):
                        with urlopen(peers.url + "/ci/jobs/T42", timeout=3) as response:
                            authoritative = json.load(response)
                        gate.wait(3)
                        return app.api(
                            "POST",
                            f"tool-calls/{cid}/results",
                            authoritative,
                            token="source-token",
                            key="ci-status-query",
                        )

                    with ThreadPoolExecutor(max_workers=1) as pool:
                        lookup = pool.submit(poll_adapter, gate, cid)
                        gate.wait(3)
                        report(app, cid)
                        self.assertEqual(lookup.result()["id"], cid)
                    self.assertEqual(peers.queries["/ci/jobs/T42"], 1)
                else:
                    report(app, cid)
                report(app, cid, "repeated-report")
                peers.ci_release.set()
                self.assertEqual(app.wait(run)["status"], "completed")
                calls = app.api("GET", f"runs/{run}/tool-calls")
                self.assertEqual(
                    len(
                        [
                            c
                            for c in calls
                            if c["tool_id"] == "code_exec"
                            and c["status"] == "completed"
                        ]
                    ),
                    1,
                )
                self.assertEqual(len(peers.submissions), 1)
                self.assertEqual(len(peers.models), 2)
                self.assertIn("E421", json.dumps(peers.models[1]["messages"]))
                self.assertIn(
                    "configuration:release", json.dumps(peers.models[1]["messages"])
                )
                trace("R05", early=early, run=run, tool_call=cid, ci_jobs=1, analyses=1)

    def test_r08_measured_execution_pool(self):
        from concurrent.futures import ThreadPoolExecutor

        samples = []
        n = 8
        for count in (1, 2, 4):
            with (
                Peers() as peers,
                deployment(peers, LOOM_ROLE="api") as (app, _, _),
                worker_processes(app, count),
            ):
                actor = ci_agent(app, peers)
                peers.model_delay = 0.04
                jobs = [start_ci(app, actor)[1] for _ in range(n)]
                for job in jobs:
                    app.wait(job, ("waiting",))
                calls = [
                    next(
                        c["id"]
                        for c in job_call(app, job)
                        if c["tool_id"] == "testing.ci.submit"
                    )
                    for job in jobs
                ]
                t0 = time.monotonic()
                with ThreadPoolExecutor(max_workers=n) as pool:
                    list(pool.map(lambda c: report(app, c), calls))
                    list(pool.map(lambda c: report(app, c, "repeat"), calls))
                    completed = list(pool.map(app.wait, jobs))
                elapsed = time.monotonic() - t0
                self.assertTrue(all(r["status"] == "completed" for r in completed))
                steps = [
                    row
                    for rid in jobs
                    for row in app.api("GET", f"runs/{rid}/steps")
                    if row.get("ready_receipt_ms") is not None
                    and row.get("committed_ms") is not None
                ]
                self.assertEqual(len({(s["run_id"], s["step"]) for s in steps}), n)
                self.assertEqual(len(steps), n)
                self.assertEqual(len(peers.submissions), n)
                self.assertEqual(len(peers.models), n * 2)
                workers = {s["worker"] for s in steps}
                if count > 1:
                    self.assertGreaterEqual(len(workers), 2)
                    self.assertTrue(
                        any(
                            a["worker"] != b["worker"]
                            and a["started_ms"] < b["committed_ms"]
                            and b["started_ms"] < a["committed_ms"]
                            for a in steps
                            for b in steps
                        )
                    )
                delays = sorted(s["started_ms"] - s["ready_receipt_ms"] for s in steps)
                self.assertGreaterEqual(min(delays), 0)
                # All three deployments get the same burst and the same measurement window.
                window_ms = 500
                start_ms = min(s["ready_receipt_ms"] for s in steps)
                in_window = sum(
                    start_ms <= s["committed_ms"] < start_ms + window_ms for s in steps
                )
                samples.append(
                    {
                        "workers": count,
                        "workload": n,
                        "independent_steps": len(steps),
                        "seconds": round(elapsed, 3),
                        "steps_per_second": round(len(steps) / elapsed, 2),
                        "p95_ms": delays[math.ceil(len(delays) * 0.95) - 1],
                        "window_ms": window_ms,
                        "window_commits": in_window,
                        "window_steps_per_second": in_window * 1000 / window_ms,
                    }
                )
        print("R08 real API samples", json.dumps(samples), flush=True)

    def test_r08_waiting_run_releases_the_default_worker(self):
        with Peers() as peers, deployment(peers) as (app, _, sid):
            _, a = start_ci(app, ci_agent(app, peers))
            app.wait(a, ("waiting",))
            b = app.send(sid, "answer independent question")["run_id"]
            self.assertEqual(app.wait(b)["status"], "completed")
            self.assertEqual(app.api("GET", "runs/" + a)["status"], "waiting")
            cid = next(
                c["id"] for c in job_call(app, a) if c["tool_id"] == "testing.ci.submit"
            )
            report(app, cid)
            self.assertEqual(app.wait(a)["status"], "completed")
            self.assertEqual(len(peers.models), 3)
            self.assertEqual(len(peers.submissions), 1)
            trace("R08", waiting_run=a, independent_run=b)

    def test_r09_cancel_ci_and_keep_its_late_result(self):
        with Peers() as peers, deployment(peers) as (app, _, wrong_session):
            actor = ci_agent(app, peers)
            sid, rid = start_ci(app, actor)
            app.wait(rid, ("waiting",))
            cid = next(
                c["id"]
                for c in job_call(app, rid)
                if c["tool_id"] == "testing.ci.submit"
            )
            with self.assertRaises(HTTPError) as wrong:
                app.send(
                    wrong_session,
                    "cancel another session",
                    action="request_cancel",
                    run_id=rid,
                )
            self.assertEqual(wrong.exception.code, 404)
            wrong.exception.close()
            self.assertEqual(app.api("GET", "runs/" + rid)["status"], "waiting")
            app.send(sid, "cancel original", action="request_cancel", run_id=rid)
            report(app, cid)
            report(app, cid, "duplicate")
            self.assertEqual(app.wait(rid)["status"], "cancelled")
            self.assertEqual(app.api("GET", "tool-calls/" + cid)["error"], "E421")
            self.assertEqual(len(peers.models), 1)
            self.assertEqual(len(peers.submissions), 1)

            trace("R09", cancelled=rid, call=cid, late_error="E421")
            sid, rid = start_ci(app, actor)
            app.wait(rid, ("waiting",))
            cid = next(
                c["id"]
                for c in job_call(app, rid)
                if c["tool_id"] == "testing.ci.submit"
            )
            app.send(
                sid,
                "Only analyze this failure. Do not rerun.",
                action="add_input",
                run_id=rid,
            )
            self.assertEqual(app.api("GET", "runs/" + rid)["status"], "waiting")
            report(app, cid)
            self.assertEqual(app.wait(rid)["status"], "completed")
            inputs = json.dumps(peers.models[-1]["messages"])
            self.assertIn("Only analyze this failure", inputs)
            self.assertIn("E421", inputs)
            self.assertEqual(len(peers.submissions), 2)
            self.assertEqual(len(peers.models), 3)
            trace("R09", supplemented=rid, call=cid, error="E421")

    def test_r10_pending_approval_across_restart(self):
        with Peers() as peers, deployment(peers) as (app, _, _):
            actor = ci_agent(app, peers, approval=True)
            for choice in ("approve", "deny", "cancel"):
                with self.subTest(choice=choice):
                    before = len(peers.submissions)
                    sid, rid = start_ci(app, actor)
                    app.wait(rid, ("waiting",))
                    first = next(
                        c
                        for c in job_call(app, rid)
                        if c["tool_id"] == "testing.ci.submit"
                    )
                    self.assertEqual(first["status"], "awaiting_approval")
                    self.assertEqual(len(peers.submissions), before)
                    cid = first["id"]
                    app.stop()
                    app.start()
                    path = f"tool-calls/{cid}/approval"
                    decision = {
                        "allow": choice != "deny",
                        "arguments": first["arguments"],
                    }
                    with self.assertRaises(HTTPError) as changed:
                        app.api(
                            "POST",
                            path,
                            {**decision, "arguments": {"other": True}},
                            token="approve-token",
                            key="altered",
                        )
                    self.assertEqual(changed.exception.code, 409)
                    changed.exception.close()
                    self.assertEqual(len(peers.submissions), before)
                    with self.assertRaises(HTTPError) as wrong:
                        app.api("POST", path, decision, key="forbidden")
                    self.assertEqual(wrong.exception.code, 403)
                    wrong.exception.close()
                    if choice == "cancel":
                        app.send(
                            sid, "cancel approval", action="request_cancel", run_id=rid
                        )
                        with self.assertRaises(HTTPError) as expired:
                            app.api(
                                "POST",
                                path,
                                decision,
                                token="approve-token",
                                key="late",
                            )
                        self.assertEqual(expired.exception.code, 409)
                        expired.exception.close()
                    else:
                        for _ in range(2):
                            app.api(
                                "POST",
                                path,
                                decision,
                                token="approve-token",
                                key="approval",
                            )
                        if choice == "approve":
                            eventually(
                                lambda: len(peers.submissions),
                                lambda n, expected=before + 1: n == expected,
                            )
                            report(app, cid)
                        self.assertEqual(app.wait(rid)["status"], "completed")
                        if choice == "deny":
                            denied = app.api("GET", "tool-calls/" + cid)
                            self.assertEqual(denied["status"], "failed")
                            self.assertEqual(denied["error"], "approval denied")
                            self.assertIn(
                                "approval denied",
                                json.dumps(peers.models[-1]["messages"]),
                            )
                    self.assertEqual(
                        len(peers.submissions), before + int(choice == "approve")
                    )
                    trace(
                        "R10",
                        decision=choice,
                        run=rid,
                        tool_call=cid,
                        dispatches=len(peers.submissions) - before,
                    )

    def test_r01_direct_idempotency_and_scope(self):
        with Peers() as peers, deployment(peers) as (app, agent, sid):
            other = app.api("POST", "sessions", {}, token="other-token")["id"]
            eid = app.api("POST", "events", {"content": "check report"})["id"]
            resource = f"events/{eid}/deliveries"
            archive = {"target": {"session_id": sid}, "action": "attach"}
            start = {"target": {"agent_id": agent}, "action": "start_run"}
            first = app.api("POST", resource, archive, key="archive")
            job = app.api("POST", resource, start, key="job")
            self.assertEqual(
                app.api("POST", resource, archive, key="archive")["id"], first["id"]
            )
            self.assertEqual(
                app.api("POST", resource, start, key="job")["id"], job["id"]
            )
            self.assertEqual(app.wait(job["run_id"])["status"], "completed")
            self.assertEqual(len(app.api("GET", resource)), 2)
            self.assertEqual(app.api("GET", f"sessions/{sid}/runs"), [])
            self.assertEqual(len(app.api("GET", "runs")), 1)
            with self.assertRaises(HTTPError) as err:
                app.api(
                    "POST",
                    resource,
                    {"target": {"session_id": other}, "action": "attach"},
                )
            self.assertIn(err.exception.code, (403, 404))
            err.exception.close()
            self.assertEqual(
                app.api("GET", f"sessions/{other}/context/items", token="other-token"),
                [],
            )
            trace(
                "R01",
                event=eid,
                archive=first["id"],
                delivery=job["id"],
                run=job["run_id"],
            )
            immediate = {
                "trigger": {"type": "direct", "event_id": eid},
                "rule": {
                    "type": "fixed",
                    "action": "attach",
                    "target": {"session_id": sid},
                },
            }
            direct = app.api("POST", "dispatches", immediate, key="direct-arrangement")
            duplicate = app.api(
                "POST", "dispatches", immediate, key="direct-arrangement"
            )
            self.assertEqual(direct["id"], duplicate["id"])
            self.assertEqual(
                len(app.api("GET", f"dispatches/{direct['id']}/deliveries")), 1
            )

    def test_r02_timed_occurrence_survives_restart(self):
        import sqlite3
        from datetime import datetime
        from pathlib import Path
        from tempfile import TemporaryDirectory

        def shanghai(local):
            return int(datetime.fromisoformat(local + "+08:00").timestamp())

        with (
            TemporaryDirectory() as clock_dir,
            Peers() as peers,
            deployment(peers, LOOM_DISPATCH_CLOCK_FILE=Path(clock_dir) / "clock") as (
                app,
                agent,
                archive,
            ),
        ):
            path = Path(clock_dir) / "clock"

            def advance(local):
                replacement = path.with_suffix(".new")
                replacement.write_text(str(shanghai(local)), encoding="ascii")
                replacement.replace(path)

            advance("2026-11-30T08:59:00")
            dispatch = app.api(
                "POST",
                "dispatches",
                {
                    "trigger": {
                        "type": "schedule",
                        "cron": "0 9 * * 1-5",
                        "timezone": "Asia/Shanghai",
                    },
                    "rule": {
                        "type": "fixed",
                        "action": "start_run",
                        "target": {"agent_id": agent},
                        "message": "timer",
                    },
                },
            )
            did = dispatch["id"]
            self.assertEqual(dispatch["next_due"], shanghai("2026-11-30T09:00:00"))
            self.assertEqual(app.api("GET", f"dispatches/{did}/deliveries"), [])
            advance("2026-11-30T09:00:00")
            read = lambda: app.api("GET", f"dispatches/{did}/deliveries")
            first = eventually(read, lambda rows: len(rows) == 1)[0]
            self.assertEqual(app.wait(first["run_id"])["status"], "completed")
            app.stop()
            # Recreate a crash after the atomic evaluation, before advancing its separate cursor.
            with closing(sqlite3.connect(app.home / "state" / "loom.db")) as fault:
                fault.execute(
                    "UPDATE objects SET doc=json_set(doc,'$.next_due',?) WHERE kind='dispatch' AND id=?",
                    (dispatch["next_due"], did),
                )
                fault.commit()
            app.start()
            eventually(
                lambda: app.api("GET", f"dispatches/{did}"),
                lambda d: d["next_due"] > dispatch["next_due"],
            )
            self.assertEqual([x["id"] for x in read()], [first["id"]])
            first_eval = app.api("GET", f"dispatches/{did}/evaluations")
            self.assertEqual(len(first_eval), 1)
            self.assertEqual(first_eval[0]["revision"], dispatch["revision"])
            self.assertEqual(len(app.api("GET", "runs")), 1)
            advance("2026-12-01T08:59:00")
            app.api("PATCH", f"dispatches/{did}", {"enabled": False})
            # A separate one-shot archive crossing the same boundary proves the scheduler saw Tuesday 09:00.
            control = app.api(
                "POST",
                "dispatches",
                {
                    "trigger": {
                        "type": "schedule",
                        "at": shanghai("2026-12-01T09:00:00"),
                    },
                    "rule": {
                        "type": "fixed",
                        "action": "attach",
                        "target": {"session_id": archive},
                    },
                },
            )
            advance("2026-12-01T09:00:00")
            eventually(
                lambda: app.api("GET", f"dispatches/{control['id']}/deliveries"), bool
            )
            self.assertEqual([x["id"] for x in read()], [first["id"]])
            advance("2026-12-02T08:59:00")
            resumed = app.api("PATCH", f"dispatches/{did}", {"enabled": True})
            self.assertEqual(resumed["next_due"], shanghai("2026-12-02T09:00:00"))
            self.assertEqual(len(read()), 1)
            advance("2026-12-02T09:00:00")
            second = eventually(read, lambda ds: len(ds) == 2)[1]
            self.assertEqual(app.wait(second["run_id"])["status"], "completed")
            history = app.api("GET", f"dispatches/{did}/evaluations")
            self.assertEqual(
                [e["revision"] for e in history],
                [dispatch["revision"], resumed["revision"]],
            )
            self.assertNotEqual(history[0]["trigger_key"], history[1]["trigger_key"])
            self.assertEqual(
                [int(e["trigger_key"].rsplit(":", 1)[1]) for e in history],
                [shanghai("2026-11-30T09:00:00"), shanghai("2026-12-02T09:00:00")],
            )
            trace(
                "R02",
                dispatch=did,
                triggers=[e["trigger_key"] for e in history],
                run=first["run_id"],
            )

    def test_r03_scoped_lua_rule_and_preview(self):
        with Peers() as peers, deployment(peers) as (app, agent, sid):
            code = """function decide(ctx)
              if ctx.event.data.status=='broken' then error('invalid report') end
              if ctx.event.data.status~='failed' then return {} end
              return {{target={session_id=ctx.params.sid},action='attach'},
                      {target={agent_id=ctx.params.agent},action='start_run'}}
            end"""
            d = app.api(
                "POST",
                "dispatches",
                {
                    "trigger": {"type": "source", "source": "ci"},
                    "rule": {
                        "type": "lua",
                        "code": code,
                        "params": {"sid": sid, "agent": agent},
                    },
                },
            )
            route = "dispatches/" + d["id"]
            sample = {
                "type": "job.finished",
                "source": "ci",
                "data": {"status": "failed"},
            }
            preview = app.api("POST", route + "/previews", {"event": sample})
            self.assertEqual(len(preview["intents"]), 2)
            self.assertEqual(app.api("GET", route + "/deliveries"), [])
            for status in ("passed", "broken", "failed", "failed"):
                app.api(
                    "POST",
                    "events",
                    {**sample, "data": {"status": status}},
                    token="source-token",
                    key=status,
                )
            app.api(
                "POST",
                "events",
                {**sample, "source": "unrelated"},
                token="source-token",
            )
            evaluations = eventually(
                lambda: app.api("GET", route + "/evaluations"), lambda v: len(v) == 3
            )
            self.assertCountEqual(
                [e["status"] for e in evaluations], ["decided", "failed", "skipped"]
            )
            self.assertEqual(len(app.api("GET", route + "/deliveries")), 2)
            self.assertEqual(len(app.api("GET", "runs")), 1)
            other = app.api("POST", "sessions", {}, token="other-token")["id"]
            updated = app.api(
                "PATCH",
                route,
                {
                    "rule": {
                        "type": "fixed",
                        "action": "attach",
                        "target": {"session_id": other},
                    }
                },
            )
            trigger = app.api(
                "POST", "events", sample, token="source-token", key="new-rule"
            )
            history = eventually(
                lambda: app.api("GET", route + "/evaluations"), lambda es: len(es) == 4
            )
            denied = next(e for e in history if e["source_event_id"] == trigger["id"])
            self.assertEqual(denied["status"], "failed")
            self.assertTrue(denied["error"])
            self.assertEqual(denied["revision"], updated["revision"])
            self.assertTrue(
                all(e["revision"] == d["revision"] for e in history if e != denied)
            )
            self.assertEqual(
                app.api("GET", f"sessions/{other}/context/items", token="other-token"),
                [],
            )
            self.assertEqual(len(app.api("GET", route + "/deliveries")), 2)
            trace(
                "R03", dispatch=d["id"], denied=denied["id"], source_event=trigger["id"]
            )

    def test_r09_accepted_cancellation_fences_late_model_return(self):
        with Peers() as peers, deployment(peers) as (app, _, sid):
            rid = app.send(sid, "HOLD-MODEL")["run_id"]
            self.assertTrue(peers.entered.wait(5))
            app.send(sid, "cancel original", action="request_cancel", run_id=rid)
            self.assertEqual(app.wait(rid)["status"], "cancelled")
            peers.release.set()
            time.sleep(0.3)
            self.assertEqual(app.api("GET", "runs/" + rid)["status"], "cancelled")
            self.assertEqual(app.api("GET", f"runs/{rid}/tool-calls"), [])
            self.assertEqual(len(peers.models), 1)

    def test_r11_ack_loss_by_receiver_capability(self):
        with (
            Peers() as peers,
            deployment(peers, LOOM_WORK_LEASE_SECONDS=2) as (app, _, archive),
        ):
            actor = ci_agent(app, peers)
            schema = {
                "type": "object",
                "properties": {
                    "status": {"enum": ["passed", "failed"]},
                    "error": {"type": "string"},
                },
                "required": ["status", "error"],
            }

            def submit(cap):
                sid = app.api("POST", "sessions", {"agent_id": actor})["id"]
                run = app.send(
                    sid,
                    "R11-OUTCOME SUBMIT-CI",
                    delivery_contract={
                        "when": "failure",
                        "target": peers.url + "/notify/" + cap,
                        "capability": cap,
                    },
                    result_schema=schema,
                )["run_id"]
                app.wait(run, ("waiting",))
                call = next(
                    c["id"]
                    for c in job_call(app, run)
                    if c["tool_id"] == "testing.ci.submit"
                )
                return run, call

            peers.hold_ack = True
            quiet, cid = submit("unknown")
            app.api(
                "POST",
                f"tool-calls/{cid}/results",
                {"status": "completed", "result": {"status": "passed"}},
                token="source-token",
                key="passed",
            )
            self.assertEqual(app.wait(quiet)["output"]["status"], "passed")
            self.assertFalse(peers.requests)
            for cap in ("query_by_key", "idempotent", "unknown"):
                with self.subTest(cap=cap):
                    path = "/notify/" + cap
                    peers.drop.add(path)
                    peers.release_ack.clear()
                    peers.receiver_saved.clear()
                    before = len(peers.models)
                    before_ci = len(peers.submissions)
                    rid, cid = submit(cap)
                    result = report(app, cid)
                    eid = result["receipt_event_id"]
                    app.api(
                        "POST",
                        f"events/{eid}/deliveries",
                        {"action": "attach", "target": {"session_id": archive}},
                        key=cap,
                    )
                    self.assertTrue(peers.receiver_saved.wait(5))
                    self.assertEqual(
                        app.api("GET", "runs/" + rid)["status"], "waiting_delivery"
                    )
                    app.stop()
                    peers.release_ack.set()
                    app.start()
                    run = eventually(
                        lambda rid=rid: app.api("GET", "runs/" + rid),
                        lambda r: r.get("delivery_status") in {"confirmed", "unknown"},
                    )
                    self.assertEqual(
                        run["status"],
                        "waiting_delivery" if cap == "unknown" else "completed",
                    )
                    self.assertEqual(
                        peers.requests[path], 2 if cap == "idempotent" else 1
                    )
                    self.assertEqual(
                        peers.queries[path], 1 if cap == "query_by_key" else 0
                    )
                    self.assertEqual(sum(k[0] == path for k in peers.notifications), 1)
                    self.assertEqual(
                        peers.notifications[path, run["reply_id"]],
                        {"status": "failed", "error": "E421"},
                    )
                    self.assertEqual(len(peers.models) - before, 2)
                    self.assertEqual(len(peers.submissions) - before_ci, 1)
                    self.assertEqual(app.api("GET", f"sessions/{archive}/runs"), [])
                    self.assertEqual(len(app.api("GET", f"events/{eid}/deliveries")), 1)
                    trace(
                        "R11",
                        capability=cap,
                        run=rid,
                        reply=run["reply_id"],
                        report=eid,
                        posts=peers.requests[path],
                        queries=peers.queries[path],
                        status=run["status"],
                    )

    def test_r13_replace_builder_keep_provider_and_observer(self):
        from acceptance_bus import audit, install

        with Peers() as peers, deployment(peers) as (app, _, _):
            install(app, "observe", "loom", "model.completed")
            for idx, body in enumerate(
                ["return {input.messages[#input.messages]}", "return input.messages"]
            ):
                name = "builder" + str(idx)
                src = (
                    "return function(loom) loom.register_context(function(ctx,input) "
                    + body
                    + " end) end"
                )
                app.api(
                    "POST",
                    "plugins/" + name + "/versions",
                    {"version": "1", "code": src},
                    token="admin-token",
                )
                app.api(
                    "POST",
                    "plugins",
                    {"id": name, "package": name, "version": "1", "enabled": True},
                    token="admin-token",
                )
                agent = app.api(
                    "POST",
                    "agents",
                    {
                        "id": "review" + name,
                        "owner": "qa",
                        "model": "fixture",
                        "context_plugin": name,
                        "grants": {"plugins": [name]},
                    },
                    token="admin-token",
                )
                session = app.api("POST", "sessions", {"agent_id": agent["id"]})["id"]
                app.send(session, "CI-CTX-42", action="attach")
                run = app.send(session, "explain current CI")["run_id"]
                self.assertEqual(app.wait(run)["status"], "completed")
                calls = app.api("GET", f"runs/{run}/model-calls")
                self.assertEqual(len(calls), 1)
                audit_fields = {"source", "context_protected", "source_event_id"}
                sent_messages = [
                    {key: value for key, value in message.items() if key not in audit_fields}
                    for message in calls[0]["messages"]
                ]
                self.assertEqual(sent_messages, peers.models[-1]["messages"])
                self.assertIn("explain current CI", json.dumps(calls[0]["messages"]))
                self.assertEqual(
                    "CI-CTX-42" in json.dumps(calls[0]["messages"]), bool(idx)
                )
                events = app.api("GET", f"events?type=model.completed&run_id={run}")
                self.assertEqual(len(events), 1)
                observed = eventually(
                    lambda eid=events[0]["id"]: audit(app, "observe", eid), bool
                )
                self.assertEqual(observed["result"]["observed"], events[0]["id"])
                history = app.api("GET", f"sessions/{session}/context/items")
                self.assertEqual(sum(it["role"] == "assistant" for it in history), 1)
                trace(
                    "R13",
                    builder=name,
                    run=run,
                    model_call=calls[0]["id"],
                    observed=events[0]["id"],
                )
            self.assertEqual(len(peers.models), 2)


if __name__ == "__main__":
    unittest.main(verbosity=2)
