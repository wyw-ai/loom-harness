"""Verify user-visible text streaming through separate HTTP API and worker processes."""

import json
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from acceptance_recovery import worker_processes
from harness import Instance


class GatedModel(BaseHTTPRequestHandler):
    first_chunk = "hello "
    last_chunk = "world"
    release_timeout = 15

    def log_message(self, *args):
        pass

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if body.get("stream") is not True:
            self.send_error(400, "Expected streaming model request")
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def send(delta, finish=None):
            chunk = {"choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
            self.wfile.flush()

        send({"reasoning_content": "internal reasoning"})
        send({"content": self.first_chunk})
        # Only the HTTP client can release the second chunk. This detects buffering
        # until completion without relying on a machine-specific latency threshold.
        if self.server.release.wait(timeout=self.release_timeout):
            send({"content": self.last_chunk}, finish="stop")
            self.wfile.write(b"data: [DONE]\n\n")
            self.wfile.flush()


def events(response):
    fields = {}
    for line in response:
        line = line.decode().rstrip("\r\n")
        if not line:
            if "data" in fields:
                yield fields["id"], fields["event"], json.loads(fields["data"])
            fields = {}
        elif not line.startswith(":"):
            name, _, value = line.partition(":")
            fields[name] = value.removeprefix(" ")


class RunStream(unittest.TestCase):
    def test_live_first_text_and_resumed_stream_across_processes(self):
        server = ThreadingHTTPServer(("127.0.0.1", 0), GatedModel)
        server.release = threading.Event()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.addCleanup(server.release.set)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        route = {
            "model": "fixture",
            "base_url": f"http://127.0.0.1:{server.server_port}",
        }
        env = {"LOOM_ROLE": "api", "LOOM_MODEL_ROUTES": json.dumps({"fixture": route})}
        with Instance(env=env) as app, worker_processes(app, 1):
            agent = app.api(
                "POST", "agents", {"id": "reader", "model": {"default": "fixture"}}
            )
            sid = app.api("POST", "sessions", {"agent_id": agent["id"]})["id"]
            rid = app.send(sid, "Say hello")["run_id"]
            url = f"{app.base}/api/runs/{rid}/stream"

            def connect(token="user-token", cursor=None):
                headers = {"Authorization": f"Bearer {token}"}
                if cursor is not None:
                    headers["Last-Event-ID"] = cursor
                return urlopen(Request(url, headers=headers), timeout=12)

            with self.assertRaises(HTTPError) as forbidden:
                connect(token="other-token")
            self.assertEqual(forbidden.exception.code, 404)
            forbidden.exception.close()
            with connect() as response:
                self.assertEqual(
                    response.headers.get_content_type(), "text/event-stream"
                )
                feed = events(response)
                _, event, attempt = next(feed)
                self.assertEqual(event, "attempt_started")
                cursor, event, first = next(feed)
                self.assertEqual((event, first["text"]), ("text_delta", "hello "))
                self.assertTrue(first["provisional"])
                self.assertEqual(first["attempt_id"], attempt["attempt_id"])
                self.assertEqual(app.api("GET", f"runs/{rid}")["status"], "running")

            server.release.set()
            with connect(cursor=cursor) as response:
                tail = list(events(response))
            self.assertEqual(
                [event for _, event, _ in tail],
                ["text_delta", "attempt_finished", "run_end"],
            )
            self.assertEqual(tail[0][2]["text"], "world")
            self.assertEqual(tail[0][2]["model_call_id"], first["model_call_id"])
            self.assertEqual(tail[0][2]["attempt_id"], first["attempt_id"])
            self.assertEqual(tail[-1][2]["status"], "completed")
            self.assertNotIn("internal reasoning", json.dumps([attempt, first, tail]))
            with connect(cursor=tail[-1][0]) as response:
                self.assertEqual(response.status, 204)
            self.assertEqual(app.api("GET", f"runs/{rid}")["output"], "hello world")
