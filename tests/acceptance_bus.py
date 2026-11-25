"""Exercise the local bus over HTTP. Redis transport fault cases use LOOM_TEST_REDIS_URL."""

import io
import json
import os
import socket
import socketserver
import sqlite3
import threading
import unittest
import uuid
from contextlib import closing, contextmanager
from urllib.parse import urlsplit

from acceptance_catalog import Peers, create_peer
from acceptance_recovery import eventually, worker_processes
from harness import Instance


REDIS_SKIP_REASON = "set LOOM_TEST_REDIS_URL to an isolated Redis 6.2+"
requires_redis = unittest.skipUnless(os.environ.get("LOOM_TEST_REDIS_URL"), REDIS_SKIP_REASON)


def frame(stream):
    """Read one RESP frame, preserving its exact wire bytes for the forwarding proxy."""
    head = stream.readline()
    if not head:
        raise EOFError()
    kind, val = head[:1], head[1:-2]
    if kind in (b"$", b"!", b"=") and int(val) >= 0:
        body = stream.read(int(val) + 2)
        if len(body) != int(val) + 2:
            raise EOFError()
        return head + body, body[:-2]
    if kind in (b"*", b"%", b"~", b"|", b">"):
        n = max(0, int(val)) * (2 if kind in (b"%", b"|") else 1)
        children = [frame(stream) for _ in range(n)]
        return head + b"".join(raw for raw, _ in children), [v for _, v in children]
    return head, val


def encode(args):
    args = [str(s).encode() for s in args]
    return b"*%d\r\n" % len(args) + b"".join(
        b"$%d\r\n" % len(s) + s + b"\r\n" for s in args
    )


class RedisGate:
    """Forward actual Redis commands, refusing only the selected crash boundary."""

    def __init__(self, url, stream):
        self.url, self.stream = urlsplit(url), stream
        if self.url.scheme != "redis" or self.url.username or self.url.password:
            raise ValueError(
                "fixture requires a dedicated redis:// test instance without credentials"
            )
        self.upstream = (self.url.hostname, self.url.port or 6379)
        self.block = lambda cmd: False
        self.hit = threading.Event()
        self.lock = threading.Lock()
        gate = self

        class Handler(socketserver.StreamRequestHandler):
            def handle(self):
                try:
                    with (
                        socket.create_connection(gate.upstream, timeout=4) as upstream,
                        upstream.makefile("rb") as replies,
                    ):
                        while True:
                            raw, parts = frame(self.rfile)
                            cmd = [p.decode() for p in parts]
                            with gate.lock:
                                blocked = gate.block(cmd)
                            if blocked:
                                gate.hit.set()
                                self.wfile.write(
                                    b"-TRYAGAIN isolated QA transport gate\r\n"
                                )
                                continue
                            upstream.sendall(raw)
                            response, _ = frame(replies)
                            self.wfile.write(response)
                except (EOFError, OSError):
                    return

        self.proxy = socketserver.ThreadingTCPServer(("127.0.0.1", 0), Handler)
        self.proxy.daemon_threads = True
        self.thread = threading.Thread(target=self.proxy.serve_forever, daemon=True)

    def start(self):
        self.thread.start()
        return f"redis://127.0.0.1:{self.proxy.server_address[1]}{self.url.path}"

    def arm(self, predicate):
        with self.lock:
            self.hit.clear()
            self.block = predicate

    def redis(self, *args):
        with (
            socket.create_connection(self.upstream, timeout=4) as sock,
            sock.makefile("rb") as replies,
        ):
            dbnum = self.url.path.strip("/") or "0"
            for command in (("SELECT", dbnum), args):
                sock.sendall(encode(command))
                raw, result = frame(replies)
                if raw.startswith(b"-"):
                    raise RuntimeError(raw.decode().strip())
            return result

    def references(self, eid):
        rows = self.redis("XRANGE", self.stream, "-", "+")
        return [r[0].decode() for r in rows if r[1] == [b"event_id", eid.encode()]]

    def close(self):
        self.proxy.shutdown()
        self.proxy.server_close()
        self.thread.join()
        self.redis(
            "DEL", self.stream
        )  # Unique test key, never the deployment's normal stream.


class ReportPeer(Peers):
    def do_POST(self):
        if self.path == "/ci":
            body = self.rfile.read(int(self.headers["Content-Length"]))
            data = json.loads(body)
            if data["method"] == "tools/call":
                self.server.counts["ci.submit"] += 1
                return self.reply(
                    200,
                    {
                        "jsonrpc": "2.0",
                        "id": data["id"],
                        "result": {
                            "content": [],
                            "structuredContent": {
                                "status": "accepted",
                                "job_id": "T42",
                            },
                        },
                    },
                )
            self.rfile = io.BytesIO(body)
        return super().do_POST()


@contextmanager
def deployment(*, redis=False, role="all"):
    gate = None
    if redis:
        gate = RedisGate(os.environ["LOOM_TEST_REDIS_URL"], "loom:qa:bus:" + uuid.uuid4().hex)
    peer = create_peer(ReportPeer)
    peer.tools["/ci"] = ["submit"]
    thread = threading.Thread(target=peer.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{peer.server_port}"
    try:
        with Instance(
            env={
                "LOOM_ROLE": role,
                "LOOM_DEFAULT_MODEL": "test",
                "LOOM_MODEL_ROUTES": json.dumps(
                    {"test": {"base_url": url + "/v1", "model": "fixture"}}
                ),
            },
            start=False,
        ) as app:
            for name in ("LOOM_EVENT_BUS", "LOOM_REDIS_URL", "LOOM_REDIS_STREAM"):
                app.env.pop(name, None)
            if gate:
                app.env.update(LOOM_REDIS_STREAM=gate.stream, LOOM_REDIS_URL=gate.start())
            app.start()
            assert app.api("GET", "/health")["event_bus"] == ("redis" if gate else "local")
            yield app, gate, peer, url
    finally:
        peer.shutdown()
        peer.server_close()
        thread.join()
        if gate:
            gate.close()


def install(app, instance, source, typ):
    callback = (
        f"plugin:subscribe({{id='audit', source={json.dumps(source)}, types={{{json.dumps(typ)}}}}}, "
        "function(ctx,e) return {observed=e.id} end)"
    )
    app.api(
        "POST",
        "plugins/" + instance + "/versions",
        {"version": "1", "code": callback},
        token="admin-token",
    )
    app.api(
        "POST",
        "plugins",
        {
            "id": instance,
            "version": "1",
            "package": instance,
            "enabled": True,
            "owner": "qa",
        },
        token="admin-token",
    )


def audit(app, instance, eid):
    return app.api("GET", f"plugins/{instance}/subscriptions/audit/events/{eid}")


def query(app, sql, *args):
    with closing(
        sqlite3.connect(
            f"file:{(app.home / 'state' / 'loom.db').as_posix()}?mode=ro", uri=True
        )
    ) as db:
        return db.execute(sql, args).fetchall()


def waiting(app, peer, url, **connection):
    install(app, "ci-tools", "loom", "tool.result_received")
    app.api(
        "POST",
        "mcp/connections",
        {"instance": "ci-tools", "id": "ci", "url": url + "/ci", **connection},
        token="admin-token",
    )
    app.api("POST", "mcp/connections/ci-tools/ci/refresh", {}, token="admin-token")
    tool = "ci-tools.ci.submit"
    app.api(
        "POST",
        "agents",
        {"id": "review", "tools": [{"tool": tool}], "grants": {"tools": [tool]}},
        token="admin-token",
    )
    sid = app.api("POST", "sessions", {"agent_id": "review"})["id"]
    peer.steps.append(f"return tools.call('{tool}', {{}})")
    rid = app.send(sid, "Submit CI and analyze its final error code.")["run_id"]
    app.wait(rid, ("waiting",), timeout=8)
    call = next(
        c for c in app.api("GET", f"runs/{rid}/tool-calls") if c["tool_id"] == tool
    )
    assert call["job_id"] == "T42" and peer.counts["ci.submit"] == 1
    return rid, call["id"]


def report(app, call):
    app.api(
        "POST",
        f"tool-calls/{call}/results",
        {"status": "completed", "job_id": "T42", "result": "FINAL_REPORT E421"},
        token="source-token",
        key="final-t42",
    )
    rows = app.api("GET", "events?type=tool.result_received")
    return next(e["id"] for e in rows if e["tool_call_id"] == call)


def finish(test, app, peer, rid, cid, eid):
    run = app.wait(rid, ("completed",), timeout=65)
    eventually(lambda: audit(app, "ci-tools", eid), bool)
    test.assertEqual(audit(app, "ci-tools", eid)["result"], {"observed": eid})
    calls = app.api("GET", f"runs/{rid}/model-calls")
    test.assertEqual(len(calls), 2)
    test.assertEqual(len(peer.inputs), 2)
    test.assertIn("FINAL_REPORT E421", json.dumps(peer.inputs[1]["messages"]))
    test.assertEqual(peer.counts["ci.submit"], 1)
    facts = app.api("GET", "events?type=tool.result_received")
    test.assertEqual([e["id"] for e in facts if e["tool_call_id"] == cid], [eid])
    test.assertEqual(len(app.api("GET", "runs")), 1)
    test.assertEqual(run["id"], rid)


class BusAcceptance(unittest.TestCase):
    @requires_redis
    def test_switch_transports_on_the_same_home(self):
        stream = "loom:qa:switch:" + uuid.uuid4().hex
        gate = RedisGate(os.environ["LOOM_TEST_REDIS_URL"], stream)
        url = gate.start()
        try:
            with Instance(start=False) as app:
                app.env.update(LOOM_EVENT_BUS="local", LOOM_REDIS_URL=url, LOOM_REDIS_STREAM=stream)
                app.start()
                self.assertEqual(app.api("GET", "/health")["event_bus"], "local")
                install(app, "before-switch", "ci", "job.finished")

                def publish():
                    event = app.api(
                        "POST", "events", {"source": "ci", "type": "job.finished"}, token="source-token"
                    )
                    return event["id"]

                def observe(instance, event):
                    result = eventually(lambda: audit(app, instance, event), bool)
                    self.assertEqual(result["result"], {"observed": event})

                def original_result_row(event):
                    return query(
                        app,
                        """SELECT rowid FROM subscription_results WHERE event_id=?
                           AND json_extract(result, '$.plugin_instance')='before-switch'""",
                        event,
                    )[0][0]

                local_event = publish()
                observe("before-switch", local_event)
                local_row = original_result_row(local_event)
                self.assertEqual(gate.references(local_event), [])

                app.stop()
                del app.env["LOOM_EVENT_BUS"]  # Existing deployments select Redis by URL.
                app.start()
                self.assertEqual(app.api("GET", "/health")["event_bus"], "redis")
                eventually(lambda: gate.references(local_event), bool)
                redis_event = publish()
                observe("before-switch", redis_event)
                redis_row = original_result_row(redis_event)

                app.stop()
                app.env["LOOM_EVENT_BUS"] = "local"
                app.start()
                self.assertEqual(app.api("GET", "/health")["event_bus"], "local")
                install(app, "after-switch", "ci", "job.finished")
                after_switch = publish()
                for event in (local_event, redis_event, after_switch):
                    for instance in ("before-switch", "after-switch"):
                        observe(instance, event)
                self.assertEqual(original_result_row(local_event), local_row)
                self.assertEqual(original_result_row(redis_event), redis_row)
                self.assertEqual(gate.references(after_switch), [])
        finally:
            gate.close()

    def test_r05_named_receipt_source_is_bound_to_the_original_tool(self):
        from urllib.error import HTTPError
        with deployment() as (app, _, peer, url):
            rid,cid=waiting(app,peer,url,receipt_source="build-ci")
            payload={"job_id":"T42","result":"FINAL_REPORT E421"}
            for actor in ("source-token","other-source"):
                with self.assertRaises(HTTPError) as rejected:
                    app.api("POST",f"tool-calls/{cid}/results",payload,token=actor,key="final")
                self.assertEqual(rejected.exception.code,403)
                rejected.exception.close()
            self.assertEqual(app.api("GET",f"runs/{rid}")["status"],"waiting")
            for _ in range(2):
                app.api("POST",f"tool-calls/{cid}/results",payload,token="ci-source",key="final")
            self.assertEqual(app.wait(rid)["status"],"completed")
            self.assertEqual(len(app.api("GET",f"runs/{rid}/model-calls")),2)
            self.assertEqual(peer.counts["ci.submit"],1)

    def test_committed_receipt_survives_restart_and_two_new_workers(self):
        with deployment(role="api") as (app, _, peer, url):
            with worker_processes(app, 1):
                rid, cid = waiting(app, peer, url)
            # Refuse the first claim. The API can still commit the event and outbox
            # entry, but neither group can see it until we lift the fault.
            with closing(sqlite3.connect(app.home / "state" / "loom.db")) as db:
                db.execute("""
                    CREATE TRIGGER delay_ci_receipt BEFORE INSERT ON local_bus_pending
                    WHEN EXISTS (
                        SELECT 1 FROM outbox JOIN objects ON objects.id=outbox.event_id
                        WHERE outbox.seq=NEW.seq AND objects.kind='event'
                        AND json_extract(objects.doc,'$.type')='tool.result_received'
                    )
                    BEGIN SELECT RAISE(ABORT,'injected claim failure'); END
                """)
                eid = report(app, cid)
                self.assertEqual(report(app, cid), eid)
                self.assertEqual(
                    query(app, "SELECT 1 FROM outbox WHERE event_id=?", eid), [(1,)]
                )
                self.assertIsNone(audit(app, "ci-tools", eid))
                self.assertEqual(query(app, "SELECT 1 FROM consumed WHERE event_id=?", eid), [])
                self.assertEqual(app.api("GET", f"runs/{rid}")["status"], "waiting")
                self.assertEqual(len(peer.inputs), 1)
                app.stop()
                db.execute("DROP TRIGGER delay_ci_receipt")
            app.start()
            with worker_processes(app, 2):
                finish(self, app, peer, rid, cid, eid)
                self.assertEqual(
                    query(app, "SELECT COUNT(*) FROM consumed WHERE event_id=?", eid), [(2,)]
                )

    def test_redelivery_after_runtime_commit_and_lost_ack(self):
        with deployment() as (app, _, peer, url):
            rid, cid = waiting(app, peer, url)
            with closing(sqlite3.connect(app.home / "state" / "loom.db")) as db:
                db.execute("""
                    CREATE TRIGGER delay_ci_ack BEFORE DELETE ON local_bus_pending
                    WHEN OLD.consumer='runtime' AND EXISTS (
                        SELECT 1 FROM outbox JOIN objects ON objects.id=outbox.event_id
                        WHERE outbox.seq=OLD.seq AND objects.kind='event'
                        AND json_extract(objects.doc,'$.type')='tool.result_received'
                    )
                    BEGIN SELECT RAISE(ABORT,'injected acknowledgement failure'); END
                """)
                eid = report(app, cid)
                finish(self, app, peer, rid, cid, eid)
                seq, = query(app, "SELECT seq FROM outbox WHERE event_id=?", eid)[0]
                pending = "SELECT 1 FROM local_bus_pending WHERE consumer='runtime' AND seq=?"
                self.assertEqual(query(app, pending, seq), [(1,)])
                app.stop()
                db.execute("DROP TRIGGER delay_ci_ack")
                # Fast-forward this abandoned receipt's lease at the crash boundary.
                db.execute(
                    "UPDATE local_bus_pending SET expires_ms=0 WHERE consumer='runtime' AND seq=?",
                    (seq,),
                )
                db.commit()
            app.env["LOOM_ROLE"] = "api"
            app.start()
            with worker_processes(app, 2):
                eventually(lambda: query(app, pending, seq), lambda rows: not rows)
                finish(self, app, peer, rid, cid, eid)
                self.assertEqual(
                    query(app, "SELECT COUNT(*) FROM consumed WHERE event_id=?", eid), [(2,)]
                )


    @requires_redis
    def test_r06_stop_after_sqlite_commit_before_xadd(self):
        with deployment(redis=True) as (app, gate, peer, url):
            rid, cid = waiting(app, peer, url)

            def before_report_xadd(cmd):
                return (
                    cmd[0] == "XADD"
                    and cmd[1] == gate.stream
                    and bool(
                        query(
                            app,
                            "SELECT 1 FROM objects WHERE kind='event' AND id=? "
                            "AND json_extract(doc,'$.tool_call_id')=? "
                            "AND json_extract(doc,'$.type')='tool.result_received'",
                            cmd[-1],
                            cid,
                        )
                    )
                )

            gate.arm(before_report_xadd)
            eid = report(app, cid)
            self.assertTrue(
                gate.hit.wait(5), "did not intercept XADD for the isolated stream"
            )
            self.assertEqual(
                query(app, "SELECT published FROM outbox WHERE event_id=?", eid), [(0,)]
            )
            self.assertEqual(gate.references(eid), [])
            self.assertEqual(app.api("GET", f"runs/{rid}")["status"], "waiting")
            self.assertEqual(
                len(peer.inputs),
                1,
                "analysis started before the runtime subscriber received the report",
            )
            app.stop()
            gate.arm(lambda cmd: False)
            app.start()
            finish(self, app, peer, rid, cid, eid)
            self.assertEqual(len(gate.references(eid)), 1)
            print(
                json.dumps(
                    {
                        "case": "R06",
                        "run": rid,
                        "event": eid,
                        "CI_jobs": 1,
                        "analysis_calls": 1,
                        "audit": 1,
                    }
                )
            )

    @requires_redis
    def test_r07_stop_after_consumer_commit_before_xack(self):
        with deployment(redis=True) as (app, gate, peer, url):
            rid, cid = waiting(app, peer, url)
            group = json.dumps(
                [
                    "subscription",
                    "qa",
                    json.dumps(["ci-tools", "audit"], separators=(",", ":")),
                ],
                separators=(",", ":"),
            )
            reading = lambda cmd, g: (
                (cmd[0] == "XREADGROUP" and cmd[2] == g)
                or (cmd[0] == "XAUTOCLAIM" and cmd[2] == g)
            )
            gate.arm(
                lambda cmd: (
                    reading(cmd, group) or (cmd[0] == "XACK" and cmd[2] == "runtime")
                )
            )
            eid = report(app, cid)
            entry = eventually(lambda: gate.references(eid), bool)[0]
            eventually(
                lambda: query(
                    app,
                    "SELECT 1 FROM consumed WHERE consumer='runtime' AND event_id=?",
                    eid,
                ),
                bool,
            )
            pending = gate.redis("XPENDING", gate.stream, "runtime", entry, entry, 1)
            self.assertEqual([r[0].decode() for r in pending], [entry])
            self.assertIsNone(audit(app, "ci-tools", eid))
            # Keep the ACK gate closed while the admitted work finishes, so the crash
            # specifically exercises bus redelivery rather than an in-flight model call.
            self.assertEqual(
                app.wait(rid, ("completed",), timeout=10)["status"], "completed"
            )
            app.stop()
            # Audit may proceed immediately. Keep runtime's transport blocked until the audit commit is visible.
            gate.arm(lambda cmd: reading(cmd, "runtime"))
            app.start()
            eventually(lambda: audit(app, "ci-tools", eid), bool)
            duplicate = gate.redis("XADD", gate.stream, "*", "event_id", eid).decode()
            self.assertNotEqual(duplicate, entry)
            gate.arm(lambda cmd: False)
            finish(self, app, peer, rid, cid, eid)
            eventually(
                lambda: gate.redis("XPENDING", gate.stream, "runtime", entry, entry, 1),
                lambda v: not v,
                timeout=20,
            )

            def drained():
                info = gate.redis("XINFO", "GROUPS", gate.stream)
                rows = [dict(zip(r[::2], r[1::2])) for r in info]
                return all(
                    any(
                        r[b"name"] == g and r[b"pending"] == b"0" and r[b"lag"] == b"0"
                        for r in rows
                    )
                    for g in (b"runtime", group.encode())
                )

            eventually(drained, bool)
            self.assertEqual(
                query(app, "SELECT COUNT(*) FROM consumed WHERE event_id=?", eid),
                [(2,)],
            )
            self.assertEqual(gate.references(eid), [entry, duplicate])
            print(
                json.dumps(
                    {
                        "case": "R07",
                        "run": rid,
                        "event": eid,
                        "stream_entries": [entry, duplicate],
                        "group_commits": 2,
                        "CI_jobs": 1,
                        "analysis_calls": 1,
                        "audit": 1,
                    }
                )
            )

    @requires_redis
    def test_r12_plugin_scope_redelivery_and_disable(self):
        self.check_plugin_subscription(redis=True)

    def test_local_plugin_scope_and_disable(self):
        self.check_plugin_subscription(redis=False)

    def check_plugin_subscription(self, *, redis):
        with deployment(redis=redis) as (app, gate, _, _):
            install(app, "audit-ci", "ci", "job.finished")
            create = lambda source: app.api(
                "POST",
                "events",
                {"source": source, "type": "job.finished"},
                token="source-token",
            )["id"]
            excluded, eid = create("docs"), create("ci")
            eventually(lambda: audit(app, "audit-ci", eid), bool)
            group = json.dumps(
                [
                    "subscription",
                    "qa",
                    json.dumps(["audit-ci", "audit"], separators=(",", ":")),
                ],
                separators=(",", ":"),
            )

            def acked():
                rows = [
                    dict(zip(r[::2], r[1::2]))
                    for r in gate.redis("XINFO", "GROUPS", gate.stream)
                ]
                return any(
                    r[b"name"] == group.encode()
                    and r[b"pending"] == b"0"
                    and r[b"lag"] == b"0"
                    for r in rows
                )

            if gate:
                duplicate = gate.redis("XADD", gate.stream, "*", "event_id", eid).decode()
                self.assertIn(duplicate, gate.references(eid))
                eventually(acked, bool)
            self.assertEqual(audit(app, "audit-ci", eid)["result"], {"observed": eid})
            self.assertEqual(
                query(
                    app,
                    "SELECT COUNT(*) FROM consumed WHERE consumer=? AND event_id=?",
                    group,
                    eid,
                ),
                [(1,)],
            )
            self.assertIsNone(audit(app, "audit-ci", excluded))
            self.assertEqual(app.api("GET", f"events/{eid}/deliveries"), [])
            self.assertEqual(app.api("GET", "runs"), [])
            app.api(
                "PATCH", "plugins/audit-ci", {"enabled": False}, token="admin-token"
            )
            after = create("ci")
            eventually(
                lambda: query(
                    app,
                    "SELECT 1 FROM consumed WHERE consumer='runtime' AND event_id=?",
                    after,
                ),
                bool,
            )
            self.assertIsNone(audit(app, "audit-ci", after))
            print(
                json.dumps(
                    {
                        "case": "R12",
                        "event": eid,
                        "backend": "redis" if gate else "local",
                        **({"stream_entries": gate.references(eid)} if gate else {}),
                        "audit": 1,
                        "out_of_scope": 0,
                        "after_disable": 0,
                        "business_deliveries": 0,
                        "runs": 0,
                    }
                )
            )


if __name__ == "__main__":
    unittest.main(verbosity=2)
