"""Exercise ordinary user flows via a live Loom HTTP server."""
import json
import os
import unittest
from urllib.error import HTTPError
from harness import Instance


class Conversation(unittest.TestCase):
    @unittest.skipUnless(os.getenv("DEEPSEEK_API_KEY"), "live DeepSeek credential required")
    def test_a01_live_model_and_reconnect(self):
        with Instance() as app:
            templates = app.api("GET", "/api/agent-templates")
            template = next(t for t in templates if t["id"] == "standard")
            model = app.api("GET", "/api/models")[0]["id"]
            agent = app.api("POST", "/api/agents", {
                "id": "assistant", "model": {"default": model},
                "template": {k: template[k] for k in ("id", "source", "version")},
            })
            sid = app.api("POST", "/api/sessions", {"agent_id": agent["id"]})["id"]
            delivery = app.send(sid, "本次登记号是 QA-715。请只回复登记号。")
            run = app.wait(delivery["run_id"])
            self.assertEqual(run["status"], "completed", run.get("error"))
            self.assertIn("QA-715", run["output"])
            calls = app.api("GET", f"/api/runs/{run['id']}/model-calls")
            self.assertEqual(calls[-1]["provider_model"], "deepseek-flash")
            app.stop()
            app.start()
            self.assertEqual([r["id"] for r in app.api("GET",f"/api/sessions/{sid}/runs")],[run["id"]])
            history = app.api("GET", f"/api/sessions/{sid}/context/items")
            self.assertEqual(history[-1]["content"], run["output"])
            self.assertEqual(app.api("GET",f"/api/runs/{run['id']}/model-calls"),calls)
            with self.assertRaises(HTTPError) as denied:
                app.api("GET", f"/api/runs/{run['id']}", token="other-token")
            self.assertEqual(denied.exception.code,404)
            print(json.dumps({"case":"A01", "run":run["id"], "model":model,
                              "reply":run["output"],"model_calls":len(calls),
                              "tool_calls":len(app.api("GET", f"/api/runs/{run['id']}/tool-calls"))}))

if __name__=="__main__":
    unittest.main()
