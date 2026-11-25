"""Verify OI catalog sync, binding revocations and direct multimodal calls via HTTP."""

import base64
import json
import random
import struct
import threading
import time
import unittest
import zlib
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError

from harness import Instance


def source(name):
    return {"channel": "host-a", "app": "filesystem", "action": name}


def screenshot():
    # A real, poorly compressible PNG exercises the default textual context budget.
    def chunk(kind, data):
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data))
        )

    rng = random.Random(42)
    pixels = b"".join(b"\0" + rng.randbytes(960) for _ in range(260))
    header = struct.pack(">IIBBBBB", 320, 260, 8, 2, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(pixels))
        + chunk(b"IEND", b"")
    )


class OiPeer(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, obj, status=200):
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(obj).encode())

    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path.endswith("/chat/completions"):
            self.server.models.append(req)
            if req["messages"][-1].get("role") == "user" and isinstance(
                req["messages"][-1]["content"], str
            ):
                msg = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "type": "function",
                            "id": "tc1",
                            "function": {"name": "view_on_a", "arguments": "{}"},
                        }
                    ],
                }
                self.reply(
                    {"choices": [{"message": msg, "finish_reason": "tool_calls"}]}
                )
            else:
                self.reply(
                    {
                        "choices": [
                            {
                                "message": {"role": "assistant", "content": "seen"},
                                "finish_reason": "stop",
                            }
                        ]
                    }
                )
            return
        sid = self.path.split("/")[-1]
        token = self.server.required_tokens.get(sid, "consumer")
        if self.headers.get("Authorization") != "Bearer " + token:
            self.reply({}, 403)
            return
        method = req["method"]
        if method == "notifications/initialized":
            self.reply({}, 202)
            return
        if method == "initialize":
            result = {
                "protocolVersion": req["params"]["protocolVersion"],
                "capabilities": {"tools": {}},
            }
        elif method == "tools/list":
            if sid not in self.server.services:
                self.reply({}, 404)
                return
            if self.server.fail_list == sid and req["params"].get("cursor"):
                self.reply({}, 503)
                return
            result = {
                "tools": [
                    {"name": n, "inputSchema": {"type": "object"}}
                    for n in self.server.services[sid]
                ]
            }
            if self.server.fail_list == sid:
                result["tools"] = result["tools"][:1]
                result["nextCursor"] = "next"
        elif method == "tools/call" and sid == "catalog":
            if self.server.fail_directory:
                self.reply({}, 503)
                return
            items = [
                {
                    "id": s,
                    "publish": {"mcp": True},
                    "actions": [{"name": n, "source": source(n)} for n in names],
                }
                for s, names in self.server.services.items()
            ]
            result = {"content": [], "structuredContent": {"items": items}}
            if (
                self.server.hold_directory
                and not self.server.snapshot_captured.is_set()
            ):
                self.server.snapshot_captured.set()
                self.server.release_snapshot.wait(8)
        else:
            self.server.calls.append(req["params"])
            result = self.server.tool_error or {
                "content": [
                    {
                        "type": "image",
                        "mimeType": "image/png",
                        "data": self.server.image,
                    }
                ],
                "_meta": {"call_id": "oi-42"},
            }
        self.reply({"jsonrpc": "2.0", "id": req["id"], "result": result})


class InternalTools(unittest.TestCase):
    def test_service_grants_sync_direct_call_and_native_images(self):
        peer = ThreadingHTTPServer(("127.0.0.1", 0), OiPeer)
        self.addCleanup(peer.server_close)
        self.addCleanup(peer.shutdown)
        threading.Thread(target=peer.serve_forever, daemon=True).start()
        peer.services = {"workbench": ["view_on_a"], "other": ["other_tool"]}
        peer.required_tokens = {"catalog": "directory"}
        peer.models, peer.calls = [], []
        peer.fail_list, peer.fail_directory = None, False
        peer.tool_error = None
        peer.hold_directory = False
        peer.snapshot_captured, peer.release_snapshot = (
            threading.Event(),
            threading.Event(),
        )
        peer.image = base64.b64encode(screenshot()).decode()
        base = f"http://127.0.0.1:{peer.server_port}"
        env = {
            "LOOM_MCP_CREDENTIAL_DIRECTORY": "directory",
            "LOOM_MCP_CREDENTIAL_CONSUMER": "consumer",
            "LOOM_MCP_CREDENTIAL_PRIVATE": "other-consumer",
            "LOOM_MODEL_ROUTES": json.dumps(
                {"vision": {"base_url": base, "model": "fixture", "images": True}}
            ),
        }
        with Instance(env) as app:

            def admin(method, path, body=None):
                return app.api(method, path, body, token="admin-token")

            def refresh():
                return admin("POST", "plugins/office/refresh", {})

            def wait_until_ready():
                until = time.monotonic() + 10
                while not admin("GET", "plugins/office/discovery")["healthy"]:
                    if time.monotonic() > until:
                        self.fail("automatic Service discovery did not finish")
                    time.sleep(0.08)

            admin(
                "POST",
                "plugins",
                {
                    "id": "office",
                    "enabled": True,
                    "package": "internal-tools",
                    "version": "1.0.0",
                },
            )
            admin(
                "PATCH",
                "settings/office",
                {
                    "base_url": base,
                    "catalog_service": "catalog",
                    "catalog_credential_ref": "directory",
                    "consumer_credential_ref": "consumer",
                    "refresh_interval_secs": 300,
                },
            )
            wait_until_ready()
            peer.required_tokens["other"] = "other-consumer"
            restricted = {s["id"]: s for s in refresh()["services"]}
            self.assertFalse(restricted["office.other"]["ready"])
            self.assertIn("403", restricted["office.other"]["error"])
            self.assertTrue(restricted["office.workbench"]["ready"])
            admin(
                "PATCH",
                "settings/office",
                {"service_credentials": {"other": "private"}},
            )
            wait_until_ready()
            set_id = "office.workbench"
            binding = [{"toolset": set_id}]
            for agent, grants in [
                ("whole", {"toolsets": [set_id]}),
                ("single", {"tools": [set_id + ".view_on_a"]}),
                ("none", {}),
            ]:
                admin(
                    "POST", "agents", {"id": agent, "tools": binding, "grants": grants}
                )

            def names(agent):
                return [
                    t["model_name"]
                    for t in app.api("GET", "agents/" + agent + "/tools")
                ]

            self.assertEqual(names("none"), [])
            peer.services["workbench"].append("new_tool")
            refresh()
            self.assertEqual(names("single"), ["view_on_a"])
            self.assertEqual(names("whole"), ["new_tool", "view_on_a"])
            sid = app.api("POST", "sessions", {"agent_id": "whole"})["id"]
            rid = app.send(sid, "Read the image")["run_id"]
            run = app.wait(rid)
            self.assertEqual(
                run["status"], "completed", run.get("error") or run.get("stop_reason")
            )
            (call,) = app.api("GET", "runs/" + rid + "/tool-calls")
            self.assertEqual(call["tool_id"], set_id + ".view_on_a")
            self.assertEqual(call["oi_call_id"], "oi-42")
            self.assertEqual(call["binding"]["source"], source("view_on_a"))
            self.assertEqual(
                peer.calls[0]["_meta"]["loom.expected_source"], source("view_on_a")
            )
            self.assertEqual(call["result"]["content"][0]["data"], peer.image)
            first, second = peer.models
            self.assertIn("view_on_a", [t["function"]["name"] for t in first["tools"]])
            self.assertEqual(second["messages"][-2]["tool_call_id"], "tc1")
            self.assertNotIn(peer.image[:80], second["messages"][-2]["content"])
            self.assertEqual(
                second["messages"][-1]["content"][-1]["image_url"]["url"],
                "data:image/png;base64," + peer.image,
            )

            peer.tool_error = {
                "isError": True,
                "content": [{"type": "text", "text": "image file missing"}],
                "structuredContent": {"error": {"code": "FILE_NOT_FOUND"}},
                "_meta": {"call_id": "oi-43"},
            }
            sid = app.api("POST", "sessions", {"agent_id": "whole"})["id"]
            failed_run = app.send(sid, "Read the missing image")["run_id"]
            app.wait(failed_run)
            (failed,) = app.api("GET", "runs/" + failed_run + "/tool-calls")
            self.assertEqual(
                (failed["status"], failed["oi_call_id"]), ("failed", "oi-43")
            )
            self.assertEqual(failed["result"], peer.tool_error)

            peer.services["other"].append("view_on_a")
            refresh()
            with self.assertRaises(HTTPError) as conflict:
                admin(
                    "PATCH",
                    "agents/whole",
                    {
                        "tools": binding + [{"toolset": "office.other"}],
                        "grants": {"toolsets": [set_id, "office.other"]},
                    },
                )
            self.assertEqual(conflict.exception.code, 409)
            conflict.exception.close()
            peer.fail_list = "workbench"
            status = refresh()
            cached = next(s for s in status["services"] if s["id"] == set_id)
            self.assertFalse(cached["ready"])
            self.assertEqual(len(cached["members"]), 2)
            self.assertEqual(names("whole"), [])
            peer.fail_list, peer.fail_directory = None, True
            self.assertFalse(refresh()["healthy"])
            peer.fail_directory = False
            peer.hold_directory = True
            with ThreadPoolExecutor(max_workers=1) as executor:
                stale = executor.submit(refresh)
                try:
                    self.assertTrue(peer.snapshot_captured.wait(3))
                    del peer.services["workbench"]
                    self.assertEqual(
                        [s["id"] for s in refresh()["services"]], ["office.other"]
                    )
                finally:
                    peer.release_snapshot.set()
                with self.assertRaises(HTTPError) as superseded:
                    stale.result(timeout=5)
                self.assertEqual(superseded.exception.code, 409)
                superseded.exception.close()
            self.assertEqual(
                [s["id"] for s in admin("GET", "plugins/office/discovery")["services"]],
                ["office.other"],
            )
            self.assertEqual(names("whole"), [])
