"""Exercise the example composer across the actual HTTP send and sample boundaries."""

import json
import os
import subprocess
import unittest

from acceptance_context import Provider, answer
from harness import Instance, ROOT


class ComposerAcceptance(unittest.TestCase):
    def test_c09_c10_real_send_after_edit_and_after_a_delayed_response(self):
        model = Provider()
        self.addCleanup(model.stop)
        model.respond = lambda payload: answer(
            "推到远端"
            if "Suggest one" in payload["messages"][0]["content"]
            else "已提交到本地"
        )
        routes = {
            "test": {
                "base_url": f"http://127.0.0.1:{model.http.server_port}/v1",
                "model": "test",
            }
        }
        with Instance({"LOOM_MODEL_ROUTES": json.dumps(routes)}) as app:
            app.api("POST", "agents", {"id": "editor", "model": "test"})
            for action, text in [("accept", "先看差异"), ("late", "先别推")]:
                sid = app.api("POST", "sessions", {"agent_id": "editor"})["id"]
                first = app.wait(app.send(sid, "完成修改")["run_id"])
                version = app.api("GET", f"sessions/{sid}")["version"]
                cmd = [
                    "node",
                    "tests/composer.integration.mjs",
                    app.base,
                    sid,
                    first["id"],
                    str(version),
                    action,
                ]
                child_env = {
                    k: v for k, v in os.environ.items() if k != "DEEPSEEK_API_KEY"
                }
                sent = json.loads(
                    subprocess.check_output(cmd, cwd=ROOT, env=child_env, timeout=30)
                )
                self.assertEqual(app.wait(sent["run_id"])["status"], "completed")
                self.assertEqual(
                    app.api("GET", f"events/{sent['event_id']}")["content"], text
                )
                history = app.api("GET", f"sessions/{sid}/context/items")
                self.assertEqual(
                    [item["content"] for item in history if item["role"] == "user"],
                    ["完成修改", text],
                )
                self.assertEqual(len(app.api("GET", f"sessions/{sid}/runs")), 2)
                for rid in (first["id"], sent["run_id"]):
                    self.assertEqual(app.api("GET", f"runs/{rid}/tool-calls"), [])
